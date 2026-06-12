//! Milky 入站事件源：[`MilkyAdapter`] 的 [`EventSource`](nagisa_core::EventSource) 实现 +
//! ws/sse pump 骨架。
//!
//! `EventSource::run` 按配置三选一：配了 webhook 走 [`crate::webhook`] 反向信道（常驻服务端，
//! 不重连）；否则按 [`MilkyMode`] 连出站 ws/sse，并套 [`nagisa_core::reconnect`] 的指数退避重连
//! （起点 500ms、封顶 30s、倍率 2），每次断开发 `Meta::Disconnect`。
//!
//! ws/sse 连上后:发 `Meta::Connect`、best-effort 探测一次 `get_impl_info`,再交
//! [`nagisa_core::framesource::pump`] 跑 idle 看门狗循环——只把「取下一帧 / shutdown 收尾」的差异
//! 下放给 [`FrameSource`](nagisa_core::framesource::FrameSource) 的两个实现 [`WsSource`]
//! (自带 keepalive Ping + Ping→Pong)与 [`SseSource`](reqwest 增量 chunk →
//! [`nagisa_core::sse::SseParser`])。所有入站帧最终经 `pump` 的 dispatch 回调汇入
//! [`MilkyAdapter::dispatch_event`](与 webhook 共用),decode 失败降级为 `Event::Raw`。
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use nagisa_core::framesource::{Frame, FrameSource};
use nagisa_core::{EventSource, ImplInfo, ShutdownToken};
use nagisa_types::error::{Error, Result, TransportError};
use nagisa_types::event::Event;
use nagisa_types::prelude::*;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::decode::decode_event;
use crate::transport::{log_wire, MilkyAdapter, MilkyMode};

/// Idle watchdog (规范 §6.2):无入站帧超过此时长即判定半开连接,断开重连。ws/sse pump 共用。
const IDLE_TIMEOUT: Duration = Duration::from_secs(90);
/// ws 客户端主动心跳周期:周期性发 Ping 探活,使半开连接尽早暴露(对端回 Pong 即重置 idle
/// 看门狗),也防止中间代理/NAT 因长时间静默而回收连接。周期取 idle 的 ~1/3。
const PING_INTERVAL: Duration = Duration::from_secs(30);

/// `connect_async` 返回的 WS 流类型(本地命名,供 [`WsSource`] 持有)。
type WsStream = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

// ───────────────────────── EventSource ─────────────────────────

#[async_trait]
impl EventSource for MilkyAdapter {
    async fn run(self: Arc<Self>, sink: mpsc::Sender<Event>, shutdown: ShutdownToken) -> Result<()> {
        // WebHook 接收端是「nagisa 起服务端、协议端 POST 事件」的反向信道，并非可重连的
        // 出站连接：独立运行路径——直接起 axum 服务运行至 shutdown，不进退避重连循环。
        // 配了 webhook 即优先此路径，`mode`(出站 ws/sse)被忽略。
        if let Some((bind, path)) = &self.webhook {
            let (bind, path) = (bind.clone(), path.clone());
            return crate::webhook::run_webhook(self.clone(), &bind, &path, sink, shutdown).await;
        }

        // 重连退避：起点 500ms、封顶 30s、倍率 2、无 jitter(沿用现状)。断开副作用
        // (发 Meta::Disconnect + 记日志,ws/sse 共用)留在「连一次」闭包内(见 nagisa_core::reconnect)。
        let backoff = nagisa_core::reconnect::Backoff::new(Duration::from_millis(500), Duration::from_secs(30));
        let me = &self;
        let sink_ref = &sink;
        let shutdown_ref = &shutdown;
        nagisa_core::reconnect::run(
            &shutdown,
            backoff,
            |_| 0,
            move || async move {
                let connect = tokio::select! {
                    biased;
                    _ = shutdown_ref.cancelled() => return nagisa_core::reconnect::Step::Stop,
                    r = async {
                        // 出站信道只有 ws/sse 两种（webhook 在上方独立路径已 return）。
                        match me.mode {
                            MilkyMode::Sse => me.connect_and_pump_sse(sink_ref, shutdown_ref).await,
                            MilkyMode::Ws => me.connect_and_pump(sink_ref, shutdown_ref).await,
                        }
                    } => r,
                };
                match connect {
                    // 干净退出（收到 shutdown）。
                    Ok(true) => nagisa_core::reconnect::Step::Stop,
                    // 连接断开/失败 → 退避重连。
                    other => {
                        // 传输层断开：发 Meta::Disconnect（ws/sse 共用此重连分支，口径统一）。
                        // `Err(e)` 带错误文案，空闲/对端关闭（Ok(false)）则无 reason。
                        let reason = match &other {
                            Err(e) => Some(e.to_string()),
                            _ => None,
                        };
                        let _ = sink_ref.try_send(Event::Meta(nagisa_types::event::Meta::Disconnect { reason }));
                        tracing::warn!("milky event source disconnected; reconnecting");
                        nagisa_core::reconnect::Step::Reconnect
                    }
                }
            },
        )
        .await
    }
}

impl MilkyAdapter {
    /// 连接一次并 pump 事件。返回 `Ok(true)` = 收到 shutdown；`Ok(false)` = 连接断开。
    async fn connect_and_pump(&self, sink: &mpsc::Sender<Event>, shutdown: &ShutdownToken) -> Result<bool> {
        // 同时带 Bearer header 与 ?access_token= query（two-belt）。
        let mut request = self
            .event_url
            .as_str()
            .into_client_request()
            .map_err(|e| Error::Transport(TransportError::WebSocket(e.to_string())))?;
        if let Some(token) = &self.access_token {
            let value = format!("Bearer {token}")
                .parse()
                .map_err(|_| Error::Transport(TransportError::WebSocket("bad token".into())))?;
            request.headers_mut().insert(reqwest::header::AUTHORIZATION.as_str(), value);
        }

        let (ws, _resp) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|e| Error::Transport(TransportError::WebSocket(e.to_string())))?;
        tracing::info!("milky event ws connected: {}", self.event_url);
        // 连接已建立：ws 只提供「取下一帧」的差异实现 `WsSource`（含自身的 keepalive 心跳与
        // Ping→Pong 处理）；连接后的公共序（Meta::Connect + probe_impl_info + idle 看门狗循环）
        // 交 `run_pump` 统一驱动（套 `nagisa_core::framesource::pump` 骨架）。
        let mut keepalive = tokio::time::interval(PING_INTERVAL);
        // 首次 tick 立即就绪;跳过它,避免连接后立刻多发一帧 Ping。
        keepalive.tick().await;
        let mut source = WsSource { ws, keepalive };
        self.run_pump(&mut source, sink, shutdown).await
    }

    /// ws/sse 连接成功后的**站点前置序 + 公共泵**:先发 `Meta::Connect`、做一次 best-effort
    /// `probe_impl_info`(这两步是 milky 专属,故留在本地、不进 core 骨架),再把 idle 看门狗循环
    /// 交 [`nagisa_core::framesource::pump`]——ws/sse 只通过
    /// [`FrameSource`](nagisa_core::framesource::FrameSource) 提供「取下一帧」与「shutdown 收尾」的
    /// 差异实现,每个入站 payload 经 dispatch 回调汇入 [`Self::dispatch_event`]。返回
    /// `Ok(true)` = 收到 shutdown(或下游 sink 关闭);`Ok(false)` = 连接断开/空闲(触发退避重连)。
    async fn run_pump<S: FrameSource + ?Sized>(
        &self,
        source: &mut S,
        sink: &mpsc::Sender<Event>,
        shutdown: &ShutdownToken,
    ) -> Result<bool> {
        // 传输层连上：发 Meta::Connect（与 OneBot 口径一致；断开由外层重连分支统一发 Disconnect）。
        let _ = sink.try_send(Event::Meta(nagisa_types::event::Meta::Connect));
        // 尽力而为的能力探测——走 HTTP,在进读循环前 await(ws/sse 共用)。
        self.probe_impl_info().await;

        // idle 看门狗循环 + 入站帧 dispatch 交 core 骨架；IDLE_TIMEOUT 不变。
        nagisa_core::framesource::pump(source, IDLE_TIMEOUT, shutdown, |raw| async move {
            self.dispatch_event(&raw, sink).await
        })
        .await
    }

    /// 把 `get_impl_info` 响应 `data` 解析为 [`ImplInfo`]。
    ///
    /// `milky_version`（Milky spec 规范版本）独立 surface 到 `ImplInfo::milky_version`，
    /// 不再被塞进实现版本 `version`（后者优先取 `impl_version`，缺失时退回 `milky_version`
    /// 以保持向后兼容的非空展示）。所有 wire 字段缺失均降级（绝不 panic）。
    pub(crate) fn parse_impl_info(data: &Value) -> ImplInfo {
        let milky_version = data.get("milky_version").and_then(|v| v.as_str()).map(str::to_string);
        let name = data.get("impl_name").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
        let version = data
            .get("impl_version")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            // 实现版本缺失时退回 spec 版本，避免展示成 "unknown"。
            .or_else(|| milky_version.clone())
            .unwrap_or_else(|| "unknown".to_string());
        let qq_protocol_version = data.get("qq_protocol_version").and_then(|v| v.as_str()).map(str::to_string);
        let qq_protocol_type = data.get("qq_protocol_type").and_then(|v| v.as_str()).map(str::to_string);
        ImplInfo { name, version, qq_protocol_version, qq_protocol_type, milky_version }
    }

    /// 最佳努力的 `get_impl_info` 能力探测（走 HTTP，不触碰事件信道）。
    /// 失败仅记日志、不阻断事件流。ws/sse pump 共用（DRY）。
    async fn probe_impl_info(&self) {
        match self.call("get_impl_info", serde_json::json!({})).await {
            Ok(data) => {
                let info = Self::parse_impl_info(&data);
                tracing::info!(
                    impl_name = %info.name,
                    impl_version = %info.version,
                    milky_version = ?info.milky_version,
                    qq_protocol_version = ?info.qq_protocol_version,
                    qq_protocol_type = ?info.qq_protocol_type,
                    "milky impl info"
                );
                let _ = self.impl_info.set(info);
            }
            Err(e) => {
                tracing::debug!("milky get_impl_info failed (non-fatal): {e}");
            }
        }
    }

    /// decode 一条事件 JSON 文本并推入 sink；结构性破坏降级为 `Event::Raw`。
    /// 返回 `true` = 下游 sink 已关闭（应终止 pump）。ws/sse pump 与 webhook 接收端共用
    /// （`pub(crate)` 供 webhook.rs 直接调用，同一 decode→sink 路径，DRY）。
    pub(crate) async fn dispatch_event(&self, raw: &str, sink: &mpsc::Sender<Event>) -> bool {
        log_wire("in", raw); // ws / sse / webhook 三条入站路径共用的单一漏斗,一处即全覆盖。
        match decode_event(raw) {
            Ok(event) => sink.send(event).await.is_err(),
            Err(e) => {
                // 结构性破坏：不丢弃，降级为 Raw 推下去。
                tracing::warn!("milky event decode error: {e}; raw fallback");
                let raw_event = nagisa_types::event::Event::Raw(nagisa_types::event::RawEvent {
                    protocol: Protocol::Milky,
                    kind: "decode_error".into(),
                    raw: serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string())),
                });
                sink.send(raw_event).await.is_err()
            }
        }
    }

    /// SSE 事件源：GET `/event`（http/https，不带 Upgrade），读取 `text/event-stream`。
    /// 逐 chunk 缓冲行，遇空行即把累积的 `data:` 行拼成 JSON 交给 `dispatch_event`。
    /// 返回 `Ok(true)` = 收到 shutdown；`Ok(false)` = 流断开（触发退避重连）。
    async fn connect_and_pump_sse(&self, sink: &mpsc::Sender<Event>, shutdown: &ShutdownToken) -> Result<bool> {
        // 事件 URL 保持 http/https scheme（event_url 是 ws/wss，需换回 http/https）。
        let mut url = self.event_url.clone();
        let http_scheme = match url.scheme() {
            "ws" | "http" => "http",
            "wss" | "https" => "https",
            other => return Err(Error::Transport(TransportError::Http(format!("unsupported scheme: {other}")))),
        };
        url.set_scheme(http_scheme).map_err(|_| Error::Transport(TransportError::Http("set_scheme failed".into())))?;

        // GET /event with Accept: text/event-stream + Bearer/query auth（与 ws pump 同样 two-belt）。
        let mut req = self.http.get(url.clone()).header(reqwest::header::ACCEPT, "text/event-stream");
        if let Some(token) = &self.access_token {
            req = req.bearer_auth(token);
        }
        let resp = req.send().await.map_err(|e| Error::Transport(TransportError::Http(e.to_string())))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(Error::Transport(TransportError::Http(format!("milky sse GET /event: HTTP {status}"))));
        }
        tracing::info!("milky event sse connected: {url}");
        // 连接已建立：公共序（Meta::Connect + probe_impl_info + idle 看门狗循环骨架）交 `run_pump`
        // 统一驱动；sse 只提供「取下一帧」的差异实现 `SseSource`（reqwest chunk → 纯 SseParser）。
        let mut source = SseSource { resp, parser: nagisa_core::sse::SseParser::new() };
        self.run_pump(&mut source, sink, shutdown).await
    }
}

/// WS 帧源:自带 keepalive 心跳 + Ping→Pong 处理;文本帧作为入站 payload 上交。
struct WsSource {
    ws: WsStream,
    keepalive: tokio::time::Interval,
}

#[async_trait]
impl FrameSource for WsSource {
    async fn next_frame(&mut self) -> Frame {
        tokio::select! {
            biased;
            _ = self.keepalive.tick() => {
                // 主动 Ping(空 payload)探活;写失败说明链路已断,触发重连。
                if let Err(e) = self.ws.send(WsMessage::Ping(Vec::<u8>::new().into())).await {
                    tracing::warn!("milky event ws keepalive ping failed: {e}; reconnecting");
                    return Frame::Closed;
                }
                Frame::Tick
            }
            msg = self.ws.next() => match msg {
                // 文本帧:作为入站 payload 上交(pump 负责 dispatch + 重置 idle)。
                Some(Ok(WsMessage::Text(text))) => Frame::Inbound(vec![text.to_string()]),
                Some(Ok(WsMessage::Ping(payload))) => {
                    if self.ws.send(WsMessage::Pong(payload)).await.is_err() {
                        return Frame::Closed;
                    }
                    // 入站帧:无 payload 可 dispatch,但仍重置 idle(链路存活)。
                    Frame::Inbound(Vec::new())
                }
                Some(Ok(WsMessage::Close(_))) => Frame::Closed,
                // Binary/Pong/Frame:入站但忽略内容,仍重置 idle。
                Some(Ok(_)) => Frame::Inbound(Vec::new()),
                Some(Err(e)) => {
                    tracing::warn!("milky event ws error: {e}");
                    Frame::Closed
                }
                None => Frame::Closed,
            }
        }
    }

    async fn close(&mut self) {
        let _ = self.ws.close(None).await;
    }
}

/// SSE 帧源:reqwest 增量 chunk → 纯 [`SseParser`](nagisa_core::sse::SseParser) 凑齐事件 payload。
struct SseSource {
    resp: reqwest::Response,
    parser: nagisa_core::sse::SseParser,
}

#[async_trait]
impl FrameSource for SseSource {
    async fn next_frame(&mut self) -> Frame {
        // `chunk()` 是 reqwest 无需 `stream` feature 的增量读取入口（Deps: none）。
        match self.resp.chunk().await {
            // 入站数据:喂入解析器,凑齐的事件 payload 作为 Inbound 上交(空 Vec 也算入站,重置 idle)。
            Ok(Some(c)) => Frame::Inbound(self.parser.feed(&c)),
            // 流正常结束 → 断开重连。
            Ok(None) => Frame::Closed,
            Err(e) => {
                tracing::warn!("milky event sse error: {e}");
                Frame::Closed
            }
        }
    }
}
