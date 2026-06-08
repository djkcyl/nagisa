//! 入站契约:丢弃被取代的帧 + 按厂商归一化 `at` 段 name + 转发。
use nagisa_types::prelude::*;
use tokio::sync::mpsc;

/// 这个解码后的事件是否已被框架的统一信号取代,因而不应转发进事件流。
///
/// 目前唯一一类:OneBot `lifecycle.connect`(解码时打了 `kind == "lifecycle_connect"` 标签)。
/// 框架事件源在 socket 连上时已统一发 `Meta::Connect`(`[元] 协议端连接`)表示「协议端连接」,
/// 这条协议帧只是同一件事的重复,丢弃零信息损失。其余 `Raw`(含未知 meta)照常转发、不丢。
/// 只此模块内 `prepare_inbound` 调用(所有入站路径都经 `dispatch_event`/`dispatch_*` 共用它)。
fn is_superseded_frame(event: &Event) -> bool {
    matches!(event, Event::Raw(r) if r.kind == "lifecycle_connect")
}

// 原始网络帧 debug 日志(target `nagisa::wire`,`dir`="in"/"out")。收发两端同 target + debug 级,
// 一条 `RUST_LOG=info,nagisa::wire=debug` 即抓本适配器收发的所有协议帧(未解析、未过滤);与
// Milky 适配器共用同一 `nagisa::wire` 漏斗(在 nagisa-core),故跨适配器口径一致。经此 re-export
// 保持 `crate::adapter::log_wire` 路径稳定(各入站/出站路径引用不变)。
pub(crate) use nagisa_core::wire::log_wire;

/// 按**厂商**把解码后事件里的 at 段 `name` 归一化到 OneBot 约定（裸昵称，消费方自行补 @）。
///
/// `name` 是否带前导 @ 取决于厂商,故只能在知道 `vendor` 的适配器层处理,且**只对会带 @ 的厂商**
/// 动手——否则会误伤真名带 @ 的人:
/// - **Lagrange**:原样透传 QQ 的 mention 文本 `"@昵称"`(含 @)→ 剥**一个**前导 @。真名叫 `@x`
///   的人在 Lagrange 上收到 `"@@x"`,剥一个得 `@x` 仍正确。
/// - **NapCat**(不发 name)/ **LLOneBot**(已 `replace('@','')` 去掉所有 @)/ **Other**:本就不带
///   前导 @(或无 name),**不动**——避免把真名带 @ 的人误伤。
///
/// 只此模块内 `prepare_inbound` 调用(所有入站路径都经它统一归一化)。
fn normalize_at_names(event: &mut Event, vendor: nagisa_types::vendor::Vendor) {
    if vendor != nagisa_types::vendor::Vendor::LagrangeOneBot {
        return;
    }
    if let Event::Message(m) = event {
        for seg in &mut m.content {
            if let Segment::Mention { name: Some(n), .. } = seg {
                if let Some(rest) = n.strip_prefix('@') {
                    *n = rest.to_string();
                }
            }
        }
    }
}

/// 入站事件的统一**预处理**:先丢弃被框架统一信号取代的冗余帧(见 [`is_superseded_frame`]
/// → `None`),否则按厂商归一化 at 段 name(见 [`normalize_at_names`])后返回。这条「丢冗余帧 +
/// 归一化」的入站契约**只此一处定义**,由下面 [`dispatch_event`](移动发送)与 HTTP-POST 的
/// `dispatch_and_decode`(克隆发送 + 回传供 quick-op hook)共用,避免两处各写一份而漂移。
pub(crate) fn prepare_inbound(mut event: Event, vendor: nagisa_types::vendor::Vendor) -> Option<Event> {
    if is_superseded_frame(&event) {
        return None;
    }
    normalize_at_names(&mut event, vendor);
    Some(event)
}

/// 把解码后的事件转发到 `sink`:经 [`prepare_inbound`] 丢冗余帧 + 归一化后发送。所有 OneBot
/// 入站路径(正向/反向 WS、HTTP-POST、SSE、long-poll)都经此/同款处理,口径一致。
///
/// 返回 `true` 表示下游 `sink` 已关闭(发送失败)——调用方据此向上终止泵循环
/// (`nagisa_core::framesource::pump` 的 dispatch 回调约定)。冗余帧(被 [`prepare_inbound`]
/// 丢弃)与发送成功均返回 `false`(无需终止)。
pub(crate) async fn dispatch_event(
    sink: &mpsc::Sender<Event>,
    event: Event,
    vendor: nagisa_types::vendor::Vendor,
) -> bool {
    if let Some(event) = prepare_inbound(event, vendor) {
        if sink.send(event).await.is_err() {
            tracing::warn!("event sink closed");
            return true;
        }
    }
    false
}
