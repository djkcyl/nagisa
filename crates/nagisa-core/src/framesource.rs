//! ws/sse(以及未来 onebot Forward 单 socket 双向)连接成功后的**公共泵骨架**:
//! [`FrameSource`] trait + [`Frame`] 枚举 + [`pump`] 循环。
//!
//! 各传输方式连上后,把「取下一帧 / shutdown 收尾」的差异实现成一个 [`FrameSource`],其余
//! 通用流程(biased shutdown 优先、idle 看门狗、`Tick`/`Closed` 处理、入站帧回调)由 [`pump`]
//! 统一驱动。连接成功后的**站点专属前置序**(发 `Meta::Connect`、best-effort 能力探测)不在
//! 本骨架内——由调用方在 `pump` 之前自行完成,core 不感知。
//!
//! # 出站写完全封装在 [`FrameSource`] 内部
//!
//! [`pump`] 只「读」:每轮拿一帧 [`Frame`]。但 [`FrameSource::next_frame`] 的实现**可以**在其
//! `select!` 内部同时做出站写——例如:
//! - milky ws 的 keepalive:周期性自发 Ping(返回 [`Frame::Tick`]),收到对端 Ping 时回 Pong
//!   (返回 [`Frame::Inbound`] 的空 payload,仍重置 idle)。
//! - onebot Forward 的「单 socket 双向」:除读入站帧外,还从自己持有的 `outbound_rx` 取动作帧
//!   写同一 socket(含取出/放回语义)+ Ping/Pong。
//!
//! 这些出站写对 [`pump`] **不可见**:骨架不持有 socket、不感知出站信道,只按返回的 [`Frame`]
//! 推进 idle 看门狗与 dispatch。如此 onebot 的「next_frame 内部自带出站写」也能自然表达。

use crate::ShutdownToken;
use async_trait::async_trait;
use std::time::Duration;

/// 连接成功后,各传输方式提供「取下一帧」与「shutdown 收尾」的差异实现;其余通用流程交 [`pump`]。
#[async_trait]
pub trait FrameSource: Send {
    /// 取下一帧。实现可在内部 `select!` 同时做出站写(keepalive Ping、动作帧写 socket 等),
    /// 这些对 [`pump`] 不可见(见模块级文档)。返回:
    /// - [`Frame::Inbound`]:收到入站帧,携带可解码的事件 payload(s)。**任何**入站活动(含
    ///   Ping/Pong、忽略内容的二进制帧)都证明链路存活,故即便 payload 为空也应返回 `Inbound`
    ///   使 [`pump`] 重置 idle 看门狗。
    /// - [`Frame::Tick`]:非入站事件(典型是自发的 keepalive 心跳触发点)。**不**重置 idle——
    ///   心跳是自己发出的,不能证明对端存活;只有对端的回应(走 `Inbound`)才重置看门狗。
    /// - [`Frame::Closed`]:连接断开/失败,[`pump`] 据此终止(交外层重连)。
    async fn next_frame(&mut self) -> Frame;

    /// 收到 shutdown 时的收尾(如 ws 发 Close 帧);默认 no-op(如 sse 无需收尾)。
    async fn close(&mut self) {}
}

/// [`FrameSource::next_frame`] 的结果。
pub enum Frame {
    /// 收到入站帧:重置 idle 看门狗并 dispatch 这些 payload(可能为空,如 ws Ping/Pong/Binary——
    /// 空 payload 仍算入站活动,重置 idle 但无可 dispatch)。
    Inbound(Vec<String>),
    /// 非入站事件(自发 keepalive 心跳触发点等):不重置 idle,继续循环。
    Tick,
    /// 连接断开/失败:终止 [`pump`],交外层重连。
    Closed,
}

/// 连接成功后的**公共泵骨架**:跑 idle 看门狗循环,shutdown 优先,把每个入站 payload 交给
/// `dispatch` 回调。**不**做发 `Meta::Connect` / 能力探测等站点前置序(调用方在 `pump` 前自理)。
///
/// 参数:
/// - `source`:已连接的帧源,提供 `next_frame` / `close`。
/// - `idle_timeout`:idle 看门狗时长;超过此时长无任何入站帧即判定半开连接,返回 `Ok(false)`。
/// - `shutdown`:关停信号(biased 优先;触发时调 `source.close()` 收尾后返回 `Ok(true)`)。
/// - `dispatch`:对每个入站 payload 调用一次的异步回调;返回 `true` 表示下游 sink 已关闭,
///   [`pump`] 立即终止并返回 `Ok(true)`(站点专属的 decode→sink 逻辑封装在回调内)。
///
/// 返回 `Ok(true)` = 收到 shutdown **或** 下游 sink 关闭(均为「不必重连」的终止);
/// `Ok(false)` = 连接断开 / 空闲(交外层退避重连)。
pub async fn pump<S, D, Fut>(
    source: &mut S,
    idle_timeout: Duration,
    shutdown: &ShutdownToken,
    mut dispatch: D,
) -> nagisa_types::error::Result<bool>
where
    S: FrameSource + ?Sized,
    D: FnMut(String) -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    // Idle watchdog:无入站帧超过此时长即判定半开连接,断开重连。
    let idle = tokio::time::sleep(idle_timeout);
    tokio::pin!(idle);

    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => {
                source.close().await;
                return Ok(true);
            }
            _ = &mut idle => {
                tracing::warn!(timeout = ?idle_timeout, "event source idle (no inbound); reconnecting");
                return Ok(false);
            }
            frame = source.next_frame() => {
                match frame {
                    // 入站帧证明链路存活,重置空闲看门狗;逐条 dispatch payload(下游关闭即终止)。
                    Frame::Inbound(payloads) => {
                        idle.as_mut().reset(tokio::time::Instant::now() + idle_timeout);
                        for payload in payloads {
                            if dispatch(payload).await {
                                return Ok(true);
                            }
                        }
                    }
                    // keepalive tick 等非入站事件:不重置 idle,继续循环。
                    Frame::Tick => {}
                    // 连接断开/失败 → 重连。
                    Frame::Closed => return Ok(false),
                }
            }
        }
    }
}
