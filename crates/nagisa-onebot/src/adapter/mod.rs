//! [`OneBotAdapter`] 本体:一个实例同时实现入站流 [`EventSource`] 与出站动作 API
//! [`ActionInvoker`],覆盖所有传输模式。
//!
//! 本文件持有**正向 WS** 路径,以及其余模式复用的机件:
//!
//! - [`EventSource::run`] 按 [`OneBotConfig::mode`] 分发:`Forward` 留在这里(连接 + 读循环 +
//!   带封顶指数退避与 jitter 的重连 + 客户端 Ping + idle 看门狗 + `select!` 监听 shutdown);
//!   `ReverseWs`、`Http`、`HttpApi`、`LLOneBotHttp` 委托给 [`crate::reverse_ws`] /
//!   [`crate::http_post`](它们再回调本文件里共享的解码 + pending-map 管线)。
//! - [`ActionInvoker`] / `OneBotActions` 动作面发动作帧并等响应。正向 / 反向 WS 在
//!   [`DashMap`] 里按生成的 `echo` 字符串存每次调用的 [`oneshot`],匹配响应帧到达时解决,
//!   10s 超时会移除该槽。HTTP 模式(`call_http`)逐动作 POST 并直接映射响应正文(无 echo)。
//!
//! `adapter` 的兄弟子模块:
//! - `config` — [`OneBotConfig`] / [`OneBotTransport`] 及其 builder。
//! - `api` — 响应封包映射 + typed 结构体提取器。
//! - `inbound` — 所有入站路径共用的一处入站契约(`prepare_inbound` / `dispatch_event`)。
//! - `onebot` — `OneBotActions` trait 实现(标准 + 厂商扩展)。
use crate::decode::{decode_event, decode_event_batch};
use crate::encode::encode_segments;
use crate::wire::{Inbound, RespJson};
use async_trait::async_trait;
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use nagisa_core::adapter::{ActionInvoker, MilkyActions, OneBotActions};
use nagisa_core::wire::Envelope;
use nagisa_core::{EventSource, ImplInfo, ShutdownToken};
use nagisa_types::event::{RawEvent, ReactionKind, RequestTokenInner};
use nagisa_types::prelude::*;
use serde_json::{json, Map, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message as WsMessage;

// `OneBotActions` trait 实现:标准动作 + 厂商扩展。
mod onebot;

mod api;
mod config;
mod inbound;

pub(crate) use api::*;
pub(crate) use config::encode_query;
pub use config::{LLOneBotEventMode, OneBotConfig, OneBotTransport, QuickOpFn};
pub(crate) use inbound::{dispatch_event, log_wire, prepare_inbound};

const ACTION_TIMEOUT: Duration = Duration::from_secs(10);
/// Reconnect watchdog (规范 §6.1):无任何入站帧(事件 / 心跳 / Pong)超过此时长即判定
/// 连接半开(NAT 静默断、对端假死),强制断开重连。
const IDLE_TIMEOUT: Duration = Duration::from_secs(90);
/// 客户端主动 ping 周期:健康对端回 Pong 会重置空闲看门狗,提前发现死连接。
const PING_INTERVAL: Duration = Duration::from_secs(30);
const PROTO: Protocol = Protocol::OneBot11;

type PendingMap = Arc<DashMap<String, oneshot::Sender<RespJson>>>;

/// OneBot v11 适配器。同时实现 [`EventSource`] 与 [`ActionInvoker`]。
pub struct OneBotAdapter {
    config: OneBotConfig,
    /// 出站动作帧(已序列化的 JSON 字符串)送往写任务。
    outbound: mpsc::Sender<String>,
    /// 接收端,只交给运行循环一次。
    outbound_rx: std::sync::Mutex<Option<mpsc::Receiver<String>>>,
    /// echo → 等待中的调用方。
    pending: PendingMap,
    /// 单调递增的 echo 序号。
    seq: AtomicU64,
    /// 首次连上后由 `get_version_info` 探得的实现信息。
    impl_info: OnceLock<ImplInfo>,
    /// 由 `app_name` 判定的厂商(OneBot 专用轴,供 per-vendor 动作名别名/能力探测)。
    vendor: OnceLock<nagisa_types::vendor::Vendor>,
    /// 非 forward 模式(反向 WS / HTTP)用:指向当前已连协议端的发送端。`call()` 把动作帧
    /// 路由到这里。无客户端连接时为 `None`。
    pub(crate) server_outbound: std::sync::Mutex<Option<mpsc::Sender<String>>>,
    /// HTTP 模式用:reqwest 客户端 + API 基址,HTTP 服务启动时装入。
    http_api: std::sync::Mutex<Option<(reqwest::Client, String)>>,
    /// 可选的快速操作 hook(仅 HTTP-POST 模式)。设了且 hook 返回 `Some(json)` 时,HTTP-POST
    /// 处理器回 `200` + 该 JSON 正文,而非默认的 `204 NO_CONTENT`。用 Mutex 守护以便在
    /// `Arc<Self>` 上构造后再设置。
    pub(crate) quick_op: std::sync::Mutex<Option<QuickOpFn>>,
}

impl OneBotAdapter {
    pub fn new(config: OneBotConfig) -> Arc<Self> {
        let (tx, rx) = mpsc::channel::<String>(256);
        Arc::new(OneBotAdapter {
            config,
            outbound: tx,
            outbound_rx: std::sync::Mutex::new(Some(rx)),
            pending: Arc::new(DashMap::new()),
            seq: AtomicU64::new(1),
            impl_info: OnceLock::new(),
            vendor: OnceLock::new(),
            server_outbound: std::sync::Mutex::new(None),
            http_api: std::sync::Mutex::new(None),
            quick_op: std::sync::Mutex::new(None),
        })
    }

    /// 返回服务端在连接时上报的实现信息(若已探得)。
    pub fn impl_info(&self) -> Option<&ImplInfo> {
        self.impl_info.get()
    }

    /// 设置快速操作应答 hook(仅 HTTP-POST 模式)。
    ///
    /// 设了之后,每条入站事件解码后都会带着该事件调用 hook。若返回 `Some(json)`,HTTP-POST
    /// 处理器回 `200 OK` + 该 JSON 正文(即被协议端消费的 quick-op 应答);若返回 `None`,
    /// 处理器回默认的 `204 NO_CONTENT`。
    ///
    /// 规范:[OneBot v11 §6.1 quick-op](https://github.com/botuniverse/onebot-11/blob/master/communication/http.md)。
    pub fn with_quick_op(
        self: Arc<Self>,
        f: impl Fn(&nagisa_types::event::Event) -> Option<Value> + Send + Sync + 'static,
    ) -> Arc<Self> {
        *self.quick_op.lock().unwrap() = Some(Arc::new(f));
        self
    }

    /// 取某条解码事件的 quick-op 应答(未设 hook / hook 返回 None 时为 None)。
    pub(crate) fn quick_op_response(&self, event: &nagisa_types::event::Event) -> Option<Value> {
        self.quick_op.lock().unwrap().as_ref().and_then(|f| f(event))
    }

    fn next_echo(&self) -> String {
        format!("nagisa-{}", self.seq.fetch_add(1, Ordering::Relaxed))
    }

    /// 发一个动作并等响应(10s 超时)。把响应封包映射成 `Result<Value>`(成功时为 `data` 字段)。
    async fn call(&self, action: &str, params: Value) -> Result<Value> {
        // HTTP 模式:POST {api_url}/<action> 后解封包(无 echo)。`Http`(webhook + API)、
        // `HttpApi`(仅 API)、`LLOneBotHttp`(API + SSE/长轮询事件)的动作都路由到这里。
        if matches!(
            self.config.mode,
            OneBotTransport::Http { .. } | OneBotTransport::HttpApi { .. } | OneBotTransport::LLOneBotHttp { .. }
        ) {
            return self.call_http(action, params).await;
        }
        let echo = self.next_echo();
        let frame = json!({ "action": action, "params": params, "echo": echo });
        let frame = serde_json::to_string(&frame).map_err(Error::Decode)?;
        log_wire("out", &frame); // 正/反向 ws 出站动作帧(带 echo,可与入站响应对照)。

        let (tx, rx) = oneshot::channel::<RespJson>();
        self.pending.insert(echo.clone(), tx);

        // Forward 模式走专用的 `outbound` 通道;反向 WS / HTTP 经当前已连的服务端客户端路由。
        let sent = match &self.config.mode {
            OneBotTransport::Forward { .. } => self.outbound.send(frame).await.is_ok(),
            _ => {
                let sender = self.server_outbound.lock().expect("server_outbound poisoned").clone();
                match sender {
                    Some(s) => s.send(frame).await.is_ok(),
                    None => false,
                }
            }
        };
        if !sent {
            self.pending.remove(&echo);
            return Err(Error::ConnectionClosed);
        }

        match tokio::time::timeout(ACTION_TIMEOUT, rx).await {
            Ok(Ok(resp)) => map_response(action, resp),
            Ok(Err(_)) => {
                // 发送端被 drop(调用中途 socket 关了)。
                self.pending.remove(&echo);
                Err(Error::ConnectionClosed)
            }
            Err(_) => {
                // 超时:移除该槽,免得 pending map 为永远等不到响应的调用积压条目。
                self.pending.remove(&echo);
                Err(Error::Timeout)
            }
        }
    }

    /// 调用 `primary`；若该端不认识此动作（Unsupported），改用 `alt` 名重试。
    /// 用于不同 OneBot 实现对同一语义动作命名不一致的情形(如 Lagrange 的
    /// `delete_group_file_folder` vs NapCat/LLOneBot 的 `delete_group_folder`)。
    async fn call_alias(&self, primary: &str, alt: &str, params: Value) -> Result<Value> {
        match self.call(primary, params.clone()).await {
            Err(e) if e.is_unsupported() => self.call(alt, params).await,
            other => other,
        }
    }

    /// HTTP-API 动作传输(`Http`、`HttpApi`、`LLOneBotHttp` 三种模式共用)。把 JSON POST 到
    /// `{api_url}/<action>` 并映射 `{status,retcode,data}` 封包。无 echo 关联:响应*就是* POST
    /// 的正文。
    async fn call_http(&self, action: &str, params: Value) -> Result<Value> {
        let (client, api_url) = {
            let guard = self.http_api.lock().expect("http_api poisoned");
            match guard.as_ref() {
                Some((c, u)) => (c.clone(), u.clone()),
                None => return Err(Error::ConnectionClosed),
            }
        };
        let url = format!("{}/{}", api_url.trim_end_matches('/'), action);
        // HTTP 出站:无 echo、action 在 URL,故单独记 action 字段 + params 作正文。
        tracing::debug!(target: "nagisa::wire", dir = "out", action = %action, "{params}");
        let mut req = client.post(&url).json(&params);
        if let Some(token) = &self.config.access_token {
            req = req.bearer_auth(token);
        }
        let resp = req.send().await.map_err(|e| Error::Transport(TransportError::Http(e.to_string())))?;
        let status = resp.status();
        // OneBot 专属的非 2xx 预筛(不进 core 骨架):除 404 外的非 2xx 都不是
        // `{status,retcode,data}` 封包(HTML 错误页 / 空 body / 鉴权挑战),在解封包之前就归为
        // `Error::Action`——否则一个解析失败的 401/5xx body 会被 `unwrap_or_default()` 吞成
        // retcode==0 的成功。404 与封包成功检查 / classify 是与 Milky 共形状的部分,交
        // `http_action_envelope` 收尾(它内部判 404→Unsupported)。
        if !status.is_success() && status != reqwest::StatusCode::NOT_FOUND {
            return Err(Error::Action {
                retcode: status.as_u16() as i64,
                message: format!("HTTP {status}"),
                kind: match status {
                    reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => ActionErrorKind::AuthFailed,
                    reqwest::StatusCode::TOO_MANY_REQUESTS => ActionErrorKind::RateLimited,
                    _ => ActionErrorKind::Other,
                },
            });
        }
        let body = resp.text().await.map_err(|e| Error::Transport(TransportError::Http(e.to_string())))?;
        log_wire("in", &body); // HTTP 入站:动作响应 + 长轮询 get_event 事件批(镜像 ws 的入站响应帧)。
                               // 公共骨架:404→Unsupported + 封包成功检查 + classify(与 Milky 适配器共形状)。OneBot
                               // 封包字段(`msg`/`wording` alias)、retcode 表(`classify_retcode`)与 ok 判定(status
                               // `ok`/`async` 或 retcode==0 均算成功,与 ws 路径的 `map_response` 同口径)是 OneBot 专属,
                               // 经 `parse` 闭包填进统一 `Envelope`。
                               //
                               // 解析失败原样透传为 `Error::Decode`(同 Milky 的 `?`):HTTP 路径无 echo 关联保证封包合法,
                               // 故畸形/空的 2xx body 绝不能被静默吞成 `Ok(Null)`。
                               //
                               // 与 `map_response` 的细微差异:retcode==404 的**失败**封包,ws 路径(`map_response`)报
                               // `Error::Unsupported(action)`,此处经骨架报 `Error::Action { kind: Unsupported }`——两者
                               // `is_unsupported()` 同真,`call_alias` 的别名回退行为一致(能力不丢)。
        nagisa_core::wire::http_action_envelope(action, status.as_u16(), &body, |body| {
            let resp: RespJson = serde_json::from_str(body)?;
            // OneBot ok 判定:`status == "ok"/"async"` 或 `retcode == 0` 均算成功(`async` =
            // 已异步受理,如 set_restart)。成功时归一为 `status:"ok"` + `retcode:0` 让骨架返回
            // `data`;失败时透传真实 retcode/message + 归类。
            let ok = resp.status == "ok" || resp.status == "async" || resp.retcode == 0;
            Ok(if ok {
                Envelope {
                    status: "ok".into(),
                    retcode: 0,
                    data: Some(resp.data),
                    message: None,
                    classify: ActionErrorKind::Other,
                }
            } else {
                let message = resp.message.unwrap_or_else(|| format!("retcode {}", resp.retcode));
                Envelope {
                    classify: classify_retcode(resp.retcode),
                    status: resp.status,
                    retcode: resp.retcode,
                    data: Some(resp.data),
                    message: Some(message),
                }
            })
        })
    }

    /// crate 内部:让反向 WS 服务端复用这条 demux+decode 管线。返回 `true` 表示下游 `sink`
    /// 已关闭(调用方应停止)。
    pub(crate) async fn handle_inbound_public(self: &Arc<Self>, txt: &str, sink: &mpsc::Sender<Event>) -> bool {
        self.handle_inbound(txt, sink).await
    }
    /// crate 内部:服务端连接断开时让所有挂起调用失败。
    pub(crate) fn clear_pending_public(&self) {
        self.pending.clear();
    }
    /// crate 内部:已配置的 access token(若有)。
    pub(crate) fn access_token(&self) -> Option<&str> {
        self.config.access_token.as_deref()
    }
    /// crate 内部:为 `call_http` 装入 HTTP-API 客户端 + 基址。
    pub(crate) fn install_http_api(&self, client: reqwest::Client, api_url: String) {
        *self.http_api.lock().expect("http_api poisoned") = Some((client, api_url));
    }
    /// 与 `dispatch_inbound_text` 类似,但额外返回解码出的事件(若有),供调用方施加 quick-op
    /// hook。由 HTTP-POST 处理器使用。
    pub(crate) async fn dispatch_and_decode(self: &Arc<Self>, txt: &str, sink: &mpsc::Sender<Event>) -> Option<Event> {
        match serde_json::from_str::<Inbound>(txt) {
            Ok(Inbound::Event(ev)) => {
                // HTTP-POST 的事件帧只到这里(不经 handle_inbound),故在此原样记一帧 dir=in。
                log_wire("in", txt);
                // 与 dispatch_event 共用 prepare_inbound 的「丢冗余帧 + 归一化」契约;冗余帧
                // (如 lifecycle.connect)→ None(不转发、也无 quick-op 可言)。这里克隆发送,
                // 把事件回传给 quick-op hook。
                let event = prepare_inbound(decode_event(*ev), self.vendor())?;
                if sink.send(event.clone()).await.is_err() {
                    tracing::warn!("event sink closed");
                }
                Some(event)
            }
            _ => {
                // 非事件帧(resp / 解不开):回到常规路径并返回 None(非事件帧无 quick-op 可言)。
                self.handle_inbound(txt, sink).await;
                None
            }
        }
    }

    /// crate 内部:把一条 SSE `/_events` `data:` 帧(单条事件 JSON)经共享管线解码并推入
    /// `sink`。解不开的帧在 `decode_event_value` 里降级为 `Event::Raw`——绝不丢弃、绝不 panic。
    /// 返回 `true` 表示下游 `sink` 已关闭(SSE 泵应停止)。
    pub(crate) async fn dispatch_sse_event(&self, payload: &str, sink: &mpsc::Sender<Event>) -> bool {
        let value = serde_json::from_str::<Value>(payload).unwrap_or(Value::Null);
        // 空 / 仅空白的 `data:`(心跳注释等)→ 静默跳过。
        if value.is_null() && payload.trim().is_empty() {
            return false;
        }
        log_wire("in", payload); // SSE 事件帧(不经 handle_inbound)。
        dispatch_event(sink, crate::decode::decode_event_value(value), self.vendor()).await
    }
}

// ===== 传输:运行循环 =====

impl OneBotAdapter {
    /// 构造 OneBot 合并转发的 `messages` 数组:
    /// `[{type:"node", data:{user_id, nickname, content[, time]}}, …]`。
    /// 由 `send(Forward::Nodes)` 与显式的 `send_*_forward` 动作共用。
    /// OFFICIAL/ENDPOINT 溯源标注见 onebot.rs(`OneBotActions::send_*_forward`)。
    /// 节点编码与消息段路径共用 `encode::encode_forward_node`(此处只是裸数组外层)。
    fn encode_forward_nodes(nodes: &[ForwardNode]) -> Value {
        Value::Array(nodes.iter().map(crate::encode::encode_forward_node).collect())
    }

    fn build_request(&self) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request> {
        // OneBot 鉴权:`Authorization: Bearer` 头 + 事件 socket 还应带 `?access_token=`
        // 查询参数(规范 §6.1)。部分实现/反向代理只读 query,故两者都带。
        // 仅 `Forward` 模式走本路径(其余传输模式在 `run` 里已分流,不会到这);故 url 取
        // `Forward { url }`,非 Forward 模式留空串。
        let base_url = match &self.config.mode {
            OneBotTransport::Forward { url } => url.clone(),
            _ => String::new(),
        };
        let url = match &self.config.access_token {
            Some(token) => {
                let sep = if base_url.contains('?') { '&' } else { '?' };
                format!("{}{}access_token={}", base_url, sep, encode_query(token))
            }
            None => base_url.clone(),
        };
        let mut req = url
            .as_str()
            .into_client_request()
            .map_err(|e| Error::Transport(TransportError::WebSocket(e.to_string())))?;
        if let Some(token) = &self.config.access_token {
            let val = format!("Bearer {token}");
            if let Ok(hv) = val.parse() {
                req.headers_mut().insert("Authorization", hv);
            }
        }
        Ok(req)
    }

    /// 一次连接+读取周期。socket 关闭或 shutdown 触发时返回。
    async fn run_once(
        self: &Arc<Self>,
        sink: &mpsc::Sender<Event>,
        outbound_rx: &mut mpsc::Receiver<String>,
        shutdown: &ShutdownToken,
    ) -> std::result::Result<(), TransportError> {
        let req = self.build_request().map_err(|e| TransportError::WebSocket(e.to_string()))?;
        let (ws, _resp) = connect_async(req).await.map_err(|e| TransportError::WebSocket(e.to_string()))?;
        // run_once 仅 Forward 模式可达（其余模式在 `run` 里已分流），故 url 取 Forward{url}。
        let url = match &self.config.mode {
            OneBotTransport::Forward { url } => url.as_str(),
            _ => "",
        };
        tracing::info!(%url, "onebot ws connected");
        // 传输层连上：发一条 Meta::Connect（框架统一信号；重连也会再发）。best-effort：
        // 通道满/关只丢这条 meta，不影响事件循环。
        let _ = sink.try_send(Event::Meta(nagisa_types::event::Meta::Connect));
        let (mut write, mut read) = ws.split();

        // 尽力而为的能力探测——发了不管,绝不阻塞事件循环。
        {
            let adapter = Arc::clone(self);
            tokio::spawn(async move {
                if let Ok(data) = adapter.call("get_version_info", serde_json::json!({})).await {
                    let name = data.get("app_name").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
                    let version = data.get("app_version").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
                    let vendor = nagisa_types::vendor::Vendor::from_app_name(&name);
                    tracing::info!(impl_name = %name, impl_version = %version, ?vendor, "onebot impl info");
                    let _ = adapter.vendor.set(vendor);
                    let _ = adapter.impl_info.set(ImplInfo {
                        name,
                        version,
                        qq_protocol_version: None,
                        qq_protocol_type: None,
                        milky_version: None,
                    });
                }
            });
        }

        // idle 看门狗 + 客户端保活 ping。任何入站帧都重置 `idle`;它一旦触发,即判定连接已死 → 重连。
        let idle = tokio::time::sleep(IDLE_TIMEOUT);
        tokio::pin!(idle);
        let mut ping_iv = tokio::time::interval(PING_INTERVAL);
        ping_iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ping_iv.tick().await; // 吃掉立即触发的第一个 tick

        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => {
                    let _ = write.close().await;
                    return Ok(());
                }
                // idle 看门狗:IDLE_TIMEOUT 内无任何入站流量 → 半开,重连。
                _ = &mut idle => {
                    tracing::warn!(timeout = ?IDLE_TIMEOUT, "onebot ws idle (no inbound); forcing reconnect");
                    return Err(TransportError::Closed);
                }
                // 客户端保活:健康对端回 Pong,会重置看门狗。
                _ = ping_iv.tick() => {
                    let _ = write.send(WsMessage::Ping(Default::default())).await;
                }
                // 出站动作帧 → socket。
                maybe_frame = outbound_rx.recv() => {
                    match maybe_frame {
                        Some(frame) => {
                            if let Err(e) = write.send(WsMessage::text(frame)).await {
                                return Err(TransportError::WebSocket(e.to_string()));
                            }
                        }
                        None => return Ok(()), // 适配器已 drop
                    }
                }
                // 入站帧。
                maybe_msg = read.next() => {
                    // 任何入站活动(事件 / 心跳 / Pong)都证明链路还活着。
                    idle.as_mut().reset(tokio::time::Instant::now() + IDLE_TIMEOUT);
                    match maybe_msg {
                        // `handle_inbound` 返回 true = 下游 sink 已关闭:消费方走了,干净收束
                        // (返回 Ok(())→Stop,不重连、不发 Disconnect)。
                        Some(Ok(WsMessage::Text(txt))) => {
                            if self.handle_inbound(txt.as_str(), sink).await {
                                return Ok(());
                            }
                        }
                        Some(Ok(WsMessage::Binary(bin))) => {
                            if let Ok(txt) = std::str::from_utf8(&bin) {
                                if self.handle_inbound(txt, sink).await {
                                    return Ok(());
                                }
                            }
                        }
                        Some(Ok(WsMessage::Ping(payload))) => {
                            let _ = write.send(WsMessage::Pong(payload)).await;
                        }
                        Some(Ok(WsMessage::Close(_))) | None => return Err(TransportError::Closed),
                        Some(Ok(_)) => {} // Pong / 其他帧:忽略
                        Some(Err(e)) => return Err(TransportError::WebSocket(e.to_string())),
                    }
                }
            }
        }
    }

    /// 对一条入站文本帧做 demux:带 `echo` 的响应解决一个挂起调用;其余皆解码成 `Event`
    /// (失败则 Raw)。
    ///
    /// 返回 `true` 表示下游 `sink` 已关闭(驱动它的泵应终止)。响应帧从不关闭 sink(它只解决
    /// 挂起调用),故那条路径返回 `false`。
    async fn handle_inbound(self: &Arc<Self>, txt: &str, sink: &mpsc::Sender<Event>) -> bool {
        // 收到即原样打出整帧(未解析、未过滤):正/反向 ws 的所有帧 + HTTP-POST 的非事件帧
        // (HTTP-POST 的事件帧走 dispatch_and_decode 单独记,故此处不会重复)。开:
        // `RUST_LOG=info,nagisa::wire=debug`。
        log_wire("in", txt);
        match serde_json::from_str::<Inbound>(txt) {
            Ok(Inbound::Resp(env)) => {
                let key = echo_key(&env.echo);
                if let Some((_, tx)) = self.pending.remove(&key) {
                    // 把完整正文重新按 RespJson 解析,取 data/message。
                    let resp: RespJson = serde_json::from_str(txt).unwrap_or_default();
                    let _ = tx.send(resp);
                } else {
                    tracing::debug!(echo = %key, "response for unknown/expired echo");
                }
                false
            }
            Ok(Inbound::Event(ev)) => dispatch_event(sink, decode_event(*ev), self.vendor()).await,
            Err(_) => {
                // 解不开的帧:尽量浮现为 Raw 而不是丢掉。
                let raw = serde_json::from_str::<Value>(txt).unwrap_or(Value::Null);
                let event = Event::Raw(RawEvent { protocol: PROTO, kind: "undecodable".to_string(), raw });
                sink.send(event).await.is_err()
            }
        }
    }
}

#[async_trait]
impl EventSource for OneBotAdapter {
    async fn run(self: Arc<Self>, sink: mpsc::Sender<Event>, shutdown: ShutdownToken) -> Result<()> {
        // 模式分发。正向 WS 跑下面的循环;其余每种模式都委托给各自的服务端/客户端驱动
        // (它们复用本适配器上共享的解码 + pending-map 管线)。
        match self.config.mode.clone() {
            OneBotTransport::Forward { .. } => {}
            OneBotTransport::ReverseWs { bind, path } => {
                return crate::reverse_ws::run_reverse_ws(self, bind, path, sink, shutdown).await;
            }
            OneBotTransport::Http { api_url, post_bind, post_path, secret } => {
                return crate::http_post::run_http_post(self, api_url, post_bind, post_path, secret, sink, shutdown)
                    .await;
            }
            OneBotTransport::HttpApi { api_url } => {
                // 纯动作客户端:装好 HTTP-API 客户端使 `call_http` 可用,然后空转(本传输不上报
                // 事件;事件经独立的 Forward/ReverseWs 适配器到达)。`sink` 故意不用。
                let _ = &sink;
                self.install_http_api(reqwest::Client::new(), api_url);
                shutdown.cancelled().await;
                return Ok(());
            }
            OneBotTransport::LLOneBotHttp { api_url, events } => {
                return crate::http_post::run_llonebot_http(self, api_url, events, sink, shutdown).await;
            }
        }
        // 一次性取出出站接收端(EventSource::run 只能调一次)。每轮连接由「连一次」闭包从
        // self.outbound_rx 短暂 take 出本地所有权、用毕放回——既满足 run_once 的 `&mut`,
        // 又不让借用越过 reconnect helper 的 FnMut 边界(借用检查约束)。
        {
            let guard = self.outbound_rx.lock().expect("outbound_rx mutex poisoned");
            assert!(guard.is_some(), "EventSource::run may only be called once");
        }

        // 重连退避：起点 500ms、封顶 30s、倍率 2；jitter 由 seq 计数器派生
        // (+ 至多 25%，无 rng 依赖)。断开副作用(清 pending + 发 Meta::Disconnect + 记日志)
        // 留在「连一次」闭包内,与退避骨架解耦(见 nagisa_core::reconnect)。
        let backoff = nagisa_core::reconnect::Backoff::new(Duration::from_millis(500), Duration::from_secs(30));
        let me = &self;
        let sink_ref = &sink;
        let shutdown_ref = &shutdown;
        nagisa_core::reconnect::run(
            &shutdown,
            backoff,
            // jitter:由 seq 计数器派生的至多 +25%(无 rng 依赖)。
            |backoff_ms| me.seq.load(Ordering::Relaxed) % (backoff_ms / 4 + 1),
            move || async move {
                // 本轮独占出站接收端:从 self 取出本地所有权(锁只在 take/put 时短暂持有,
                // 绝不跨 await),run_once 用毕放回供下一轮重连复用。
                let mut outbound_rx = me
                    .outbound_rx
                    .lock()
                    .expect("outbound_rx mutex poisoned")
                    .take()
                    .expect("outbound_rx taken concurrently");
                let result = me.run_once(sink_ref, &mut outbound_rx, shutdown_ref).await;
                *me.outbound_rx.lock().expect("outbound_rx mutex poisoned") = Some(outbound_rx);
                match result {
                    Ok(()) => nagisa_core::reconnect::Step::Stop, // 正常退出
                    Err(e) => {
                        // 让挂起调用失败,免得调用方等过 socket 生命期还在挂。
                        me.pending.clear();
                        if shutdown_ref.is_cancelled() {
                            return nagisa_core::reconnect::Step::Stop;
                        }
                        // 传输层断开：发一条 Meta::Disconnect（携带底层错误文案），重连前通知监听方。
                        let _ = sink_ref.try_send(Event::Meta(nagisa_types::event::Meta::Disconnect {
                            reason: Some(e.to_string()),
                        }));
                        tracing::warn!(error = %e, "onebot ws disconnected; reconnecting");
                        nagisa_core::reconnect::Step::Reconnect
                    }
                }
            },
        )
        .await
    }
}

// ===== ActionInvoker =====

#[async_trait]
impl ActionInvoker for OneBotAdapter {
    fn protocol(&self) -> Protocol {
        PROTO
    }

    fn vendor(&self) -> nagisa_types::vendor::Vendor {
        self.vendor.get().copied().unwrap_or_default()
    }

    fn supports(&self, cap: Capability) -> bool {
        // 所有 OneBot v11 端都普遍支持的能力。
        if matches!(
            cap,
            Capability::GroupMute
                | Capability::GroupAdmin
                | Capability::GroupKick
                | Capability::HandleRequest
                | Capability::Essence
                | Capability::Reaction
                | Capability::Forward
                | Capability::FileOps
                | Capability::Nudge
                | Capability::ProfileLike
                | Capability::SelfProfile
                | Capability::MessageHistory
                | Capability::Cookies
                | Capability::Ocr
        ) {
            return true;
        }
        // AI 语音合成:只有 Lagrange / NapCat / LLOneBot 支持。
        if cap == Capability::Ai {
            return matches!(
                self.vendor(),
                nagisa_types::vendor::Vendor::LagrangeOneBot
                    | nagisa_types::vendor::Vendor::NapCat
                    | nagisa_types::vendor::Vendor::LLOneBot
            );
        }
        false
    }

    async fn send(&self, peer: &Peer, message: &[Segment]) -> Result<MessageId> {
        // 若唯一的段是 Forward::Nodes,改用专用的转发动作——Lagrange 会忽略 send_*_msg 里的
        // `forward` 段。
        if let [Segment::Forward(Forward::Nodes { nodes, .. })] = message {
            let messages = Self::encode_forward_nodes(nodes);
            let (action, params) = match peer.scene {
                Scene::Group => ("send_group_forward_msg", json!({ "group_id": peer.id.0, "messages": messages })),
                Scene::Friend | Scene::Temp => {
                    ("send_private_forward_msg", json!({ "user_id": peer.id.0, "messages": messages }))
                }
            };
            let data = self.call(action, params).await?;
            let onebot_id = data_i64(&data, "message_id").map(|v| v as i32);
            return Ok(MessageId { peer: *peer, seq: 0, onebot_id });
        }

        let wire = encode_segments(message);
        let segs = serde_json::to_value(&wire).map_err(Error::Decode)?;
        let (action, params) = match peer.scene {
            Scene::Group => ("send_group_msg", json!({ "group_id": peer.id.0, "message": segs })),
            Scene::Friend | Scene::Temp => ("send_private_msg", json!({ "user_id": peer.id.0, "message": segs })),
        };
        let data = self.call(action, params).await?;
        let onebot_id = data_i64(&data, "message_id").map(|v| v as i32);
        Ok(MessageId { peer: *peer, seq: 0, onebot_id })
    }

    async fn recall(&self, id: &MessageId) -> Result<()> {
        let mid = id
            .onebot_id
            .ok_or_else(|| Error::action_kind(ActionErrorKind::BadParams, "message id has no onebot_id"))?;
        self.call("delete_msg", json!({ "message_id": mid })).await?;
        Ok(())
    }

    async fn get_message(&self, id: &MessageId) -> Result<MessageEvent> {
        // OneBot 用合成的整型 `message_id` 寻址一条存储消息。没有它就取不到消息 → Unsupported
        // (干净降级)。
        let mid = id.onebot_id.ok_or_else(|| Error::Unsupported("get_message".into()))?;
        let data = self.call("get_msg", json!({ "message_id": mid })).await?;
        // 复用事件解码路径:把响应打上 `message` post 标签,过一遍 `decode_event`,再取出
        // `MessageEvent`。
        let mut obj = match data {
            Value::Object(m) => m,
            other => {
                let mut m = Map::new();
                m.insert("message".into(), other);
                m
            }
        };
        obj.insert("post_type".into(), Value::String("message".into()));
        // `get_msg` 返回 `message_type`(group/private);缺省时默认按 private。
        obj.entry("message_type".to_string()).or_insert(Value::String("private".into()));
        let ev: crate::wire::RawEventJson = serde_json::from_value(Value::Object(obj)).map_err(Error::Decode)?;
        match decode_event(ev) {
            Event::Message(m) => Ok(*m),
            _ => Err(Error::action("get_msg response did not decode to a message")),
        }
    }

    async fn get_login_info(&self) -> Result<(Uin, String)> {
        let data = self.call("get_login_info", json!({})).await?;
        let uin = Uin(data_i64(&data, "user_id").unwrap_or(0));
        let nick = data_str(&data, "nickname").unwrap_or_default();
        Ok((uin, nick))
    }

    async fn get_group_info(&self, group: Uin, no_cache: bool) -> Result<GroupInfo> {
        let data = self.call("get_group_info", json!({ "group_id": group.0, "no_cache": no_cache })).await?;
        Ok(group_info_from(&data))
    }

    async fn get_group_list(&self, no_cache: bool) -> Result<Vec<GroupInfo>> {
        let data = self.call("get_group_list", json!({ "no_cache": no_cache })).await?;
        Ok(data.as_array().map(|a| a.iter().map(group_info_from).collect()).unwrap_or_default())
    }

    async fn get_group_member_info(&self, group: Uin, user: Uin, no_cache: bool) -> Result<MemberInfo> {
        let data = self
            .call("get_group_member_info", json!({ "group_id": group.0, "user_id": user.0, "no_cache": no_cache }))
            .await?;
        Ok(member_info_from(&data))
    }

    async fn get_group_member_list(&self, group: Uin, no_cache: bool) -> Result<Vec<MemberInfo>> {
        let data = self.call("get_group_member_list", json!({ "group_id": group.0, "no_cache": no_cache })).await?;
        Ok(data.as_array().map(|a| a.iter().map(member_info_from).collect()).unwrap_or_default())
    }

    async fn get_friend_list(&self, no_cache: bool) -> Result<Vec<FriendInfo>> {
        let data = self.call("get_friend_list", json!({ "no_cache": no_cache })).await?;
        Ok(data.as_array().map(|a| a.iter().map(friend_info_from).collect()).unwrap_or_default())
    }

    async fn set_group_member_mute(&self, group: Uin, user: Uin, duration: u32) -> Result<()> {
        self.call("set_group_ban", json!({ "group_id": group.0, "user_id": user.0, "duration": duration })).await?;
        Ok(())
    }

    async fn set_group_whole_mute(&self, group: Uin, enable: bool) -> Result<()> {
        self.call("set_group_whole_ban", json!({ "group_id": group.0, "enable": enable })).await?;
        Ok(())
    }

    async fn set_group_admin(&self, group: Uin, user: Uin, enable: bool) -> Result<()> {
        self.call("set_group_admin", json!({ "group_id": group.0, "user_id": user.0, "enable": enable })).await?;
        Ok(())
    }

    async fn set_group_member_card(&self, group: Uin, user: Uin, card: &str) -> Result<()> {
        self.call("set_group_card", json!({ "group_id": group.0, "user_id": user.0, "card": card })).await?;
        Ok(())
    }

    async fn set_group_name(&self, group: Uin, name: &str) -> Result<()> {
        self.call("set_group_name", json!({ "group_id": group.0, "group_name": name })).await?;
        Ok(())
    }

    async fn kick_group_member(&self, group: Uin, user: Uin, reject_add: bool) -> Result<()> {
        self.call(
            "set_group_kick",
            json!({ "group_id": group.0, "user_id": user.0, "reject_add_request": reject_add }),
        )
        .await?;
        Ok(())
    }

    async fn handle_request(&self, token: &RequestToken, approve: bool, reason: Option<&str>) -> Result<()> {
        // 拆开不透明的 OneBot flag。flag 本身分不出好友请求还是群请求;OneBot 两者端点不同。
        // 我们对裸 flag 先试好友端点,对带 Lagrange 群请求那种复合形态
        // "{seq}-{groupUin}-{eventType}" 的 flag 走群端点。
        let RequestTokenInner::OneBotFlag(flag) = &token.0 else {
            return Err(Error::action_kind(ActionErrorKind::BadParams, "request token is not an OneBot flag"));
        };
        // Lagrange 群 flag 形如 "{seq}-{groupUin}-{eventType}[-{isFiltered}]"。
        // eventType 1 = 加群请求(sub_type "add"),2 = 自身被邀请(sub_type "invite")。
        let looks_group = flag.matches('-').count() >= 2;
        if looks_group {
            // 解析以 `-` 分隔的第 3 段,辨别是邀请还是加群请求。
            let sub_type = flag
                .split('-')
                .nth(2)
                .and_then(|s| s.parse::<i64>().ok())
                .map(|t| if t == 2 { "invite" } else { "add" })
                .unwrap_or("add");
            let mut params = json!({
                "flag": flag,
                "sub_type": sub_type,
                "approve": approve,
            });
            if let (false, Some(r)) = (approve, reason) {
                params["reason"] = Value::String(r.to_string());
            }
            self.call("set_group_add_request", params).await?;
        } else {
            let mut params = json!({ "flag": flag, "approve": approve });
            if let Some(r) = reason {
                params["remark"] = Value::String(r.to_string());
            }
            self.call("set_friend_add_request", params).await?;
        }
        Ok(())
    }

    async fn send_reaction(
        &self,
        group: Uin,
        seq: i64,
        face_id: &str,
        _kind: ReactionKind,
        is_add: bool,
    ) -> Result<()> {
        self.call(
            "set_group_reaction",
            json!({
                "group_id": group.0,
                "message_id": seq,
                "code": face_id,
                "is_add": is_add,
            }),
        )
        .await?;
        Ok(())
    }

    async fn send_nudge(&self, peer: &Peer, target: Uin) -> Result<()> {
        // Lagrange.OneBot 只注册 `group_poke`/`friend_poke`(不认 NapCat 的 `send_poke`);
        // NapCat/LLOneBot 同时支持这两个名字,故按场景分发到通用名,两端都能命中。
        let (action, params) = match peer.scene {
            Scene::Group => ("group_poke", json!({ "group_id": peer.id.0, "user_id": target.0 })),
            _ => ("friend_poke", json!({ "user_id": target.0 })),
        };
        self.call(action, params).await?;
        Ok(())
    }

    async fn upload_group_file(
        &self,
        group: Uin,
        src: ResourceSource,
        name: &str,
        parent_folder_id: Option<&str>,
    ) -> Result<String> {
        let file = crate::encode::encode_source(&src);
        let mut params = serde_json::json!({ "group_id": group.0, "file": file, "name": name });
        // 部分 OneBot 端(Lagrange、NapCat)接受可选的 folder/folder_id 参数。给了就带,None 时
        // 省略以保持既有行为。
        if let Some(folder) = parent_folder_id {
            params["folder"] = serde_json::Value::String(folder.to_string());
            params["folder_id"] = serde_json::Value::String(folder.to_string());
        }
        let data = self.call("upload_group_file", params).await?;
        // 标准 OneBot v11 返回空 data;Lagrange/部分实现会回传 file_id。
        Ok(data_str(&data, "file_id").unwrap_or_default())
    }

    async fn upload_private_file(&self, user: Uin, src: ResourceSource, name: &str) -> Result<String> {
        let file = crate::encode::encode_source(&src);
        let data = self.call("upload_private_file", json!({ "user_id": user.0, "file": file, "name": name })).await?;
        Ok(data_str(&data, "file_id").unwrap_or_default())
    }

    async fn get_user_info(&self, user: Uin, no_cache: bool) -> Result<UserInfo> {
        let data = self.call("get_stranger_info", json!({ "user_id": user.0, "no_cache": no_cache })).await?;
        Ok(user_info_from(&data))
    }

    async fn get_message_history(
        &self,
        peer: &Peer,
        start: Option<&MessageId>,
        count: u32,
    ) -> Result<Vec<MessageEvent>> {
        // Lagrange 用合成的整型 `message_id` 寻址历史锚点;`0`(该字段默认值)表示「从最新消息起」。
        let anchor = start.and_then(|m| m.onebot_id).unwrap_or(0);
        let (action, params, is_group) = match peer.scene {
            Scene::Group => {
                ("get_group_msg_history", json!({ "group_id": peer.id.0, "message_id": anchor, "count": count }), true)
            }
            Scene::Friend | Scene::Temp => {
                ("get_friend_msg_history", json!({ "user_id": peer.id.0, "message_id": anchor, "count": count }), false)
            }
        };
        let data = self.call(action, params).await?;
        // 响应:`{ "messages": [ <完整 onebot 消息事件>, ... ] }`。
        let arr = data.get("messages").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let mut out = Vec::with_capacity(arr.len());
        for entry in arr {
            // 复用事件解码路径(同 `get_message`):给每条打上 `message` post 标签,过一遍
            // `decode_event`。
            let mut obj = match entry {
                Value::Object(m) => m,
                _ => continue,
            };
            obj.insert("post_type".into(), Value::String("message".into()));
            obj.entry("message_type".to_string()).or_insert(Value::String(if is_group {
                "group".into()
            } else {
                "private".into()
            }));
            let Ok(ev) = serde_json::from_value::<crate::wire::RawEventJson>(Value::Object(obj)) else {
                continue;
            };
            if let Event::Message(m) = decode_event(ev) {
                out.push(*m);
            }
        }
        Ok(out)
    }

    async fn leave_group(&self, group: Uin, dismiss: bool) -> Result<()> {
        self.call("set_group_leave", json!({ "group_id": group.0, "is_dismiss": dismiss })).await?;
        Ok(())
    }

    async fn set_group_member_special_title(&self, group: Uin, user: Uin, title: &str, duration: i64) -> Result<()> {
        self.call(
            "set_group_special_title",
            json!({ "group_id": group.0, "user_id": user.0, "special_title": title, "duration": duration }),
        )
        .await?;
        Ok(())
    }

    async fn mark_message_as_read(&self, _peer: &Peer, id: &MessageId) -> Result<()> {
        let mid = onebot_id_of(id)?;
        self.call("mark_msg_as_read", json!({ "message_id": mid })).await?;
        Ok(())
    }

    async fn set_essence(&self, _group: Uin, id: &MessageId, enable: bool) -> Result<()> {
        let mid = onebot_id_of(id)?;
        let action = if enable { "set_essence_msg" } else { "delete_essence_msg" };
        self.call(action, json!({ "message_id": mid })).await?;
        Ok(())
    }

    async fn get_group_file_download_url(&self, group: Uin, file_id: &str) -> Result<String> {
        let data = self.call("get_group_file_url", json!({ "group_id": group.0, "file_id": file_id })).await?;
        Ok(data_str(&data, "url").unwrap_or_default())
    }

    async fn delete_group_file(&self, group: Uin, file_id: &str) -> Result<()> {
        self.call("delete_group_file", json!({ "group_id": group.0, "file_id": file_id })).await?;
        Ok(())
    }

    async fn get_group_requests(&self) -> Result<Vec<Request>> {
        let data = self.call("get_group_requests", json!({})).await?;
        let arr = data.as_array().cloned().unwrap_or_default();
        let mut out = Vec::with_capacity(arr.len());
        for entry in &arr {
            let group = Uin(data_i64(entry, "group_id").unwrap_or(0));
            let initiator = Uin(data_i64(entry, "user_id").unwrap_or(0));
            let comment = data_str(entry, "comment").unwrap_or_default();
            let flag = data_str(entry, "flag").unwrap_or_default();
            let token = RequestToken::onebot_flag(flag);
            // Lagrange sub_type:"invite"(自身被邀请)vs "add"(加群请求)。
            let req = if entry.get("sub_type").and_then(|v| v.as_str()) == Some("invite") {
                Request::GroupInvite { group, initiator, comment, source_group: None, token }
            } else {
                Request::GroupJoin {
                    group,
                    initiator,
                    comment,
                    invitor: data_i64(entry, "invitor_id").map(Uin),
                    is_filtered: false,
                    token,
                }
            };
            out.push(req);
        }
        Ok(out)
    }

    async fn get_cookies(&self, domain: Option<&str>) -> Result<String> {
        let data = self.call("get_cookies", json!({ "domain": domain.unwrap_or("") })).await?;
        Ok(data_str(&data, "cookies").unwrap_or_default())
    }

    // ===== 转发 =====

    // OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/api/public.md
    // get_forward_msg:参数 `id`(官方/Lagrange);NapCat 也接受 `message_id`。
    // 响应 `message`(官方/Lagrange) / `messages`(NapCat):`node` 段数组
    // `{type:"node", data:{user_id, nickname, content:[...]}}`——NapCat 把节点正文放在
    // `message` 下,Lagrange/官方放在 `content` 下。
    async fn get_forward_messages(&self, forward_id: &str) -> Result<Vec<ForwardNode>> {
        let data = self.call("get_forward_msg", json!({ "id": forward_id, "message_id": forward_id })).await?;
        let arr = data
            .get("messages")
            .or_else(|| data.get("message"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut out = Vec::with_capacity(arr.len());
        for node in &arr {
            // 每条都是一个 `node` 段;节点正文在 `data` 下。go-cqhttp 用 `uin`/`name`,
            // 标准用 `user_id`/`nickname`。
            let body = node.get("data").unwrap_or(node);
            let user = Uin(data_i64(body, "user_id").or_else(|| data_i64(body, "uin")).unwrap_or(0));
            let name = data_str(body, "nickname").or_else(|| data_str(body, "name")).unwrap_or_default();
            let content_val = body.get("content").or_else(|| body.get("message"));
            // 独立展开的转发没有固有的会话对端(节点已脱离任何聊天);嵌套 `reply` 段用中性的
            // group(0) 对端——与 Milky 的 forward_node_from_value 对齐。
            let content =
                content_val.map(|c| crate::decode::decode_message_value(c, Peer::group(0))).unwrap_or_default();
            let time = body.get("time").and_then(Value::as_i64);
            out.push(ForwardNode { user, name, content, time });
        }
        Ok(out)
    }

    // ===== 好友 =====

    // ENDPOINT: Lagrange Lagrange.OneBot/Core/Operation/Generic/DeleteFriendOperation.cs
    //   (https://github.com/LagrangeDev/Lagrange.Core);亦见 NapCat/LLOneBot `delete_friend`。
    // 参数 {user_id}。Lagrange 还接受可选的 `block` 标志(此处省略)。
    async fn delete_friend(&self, user: Uin) -> Result<()> {
        self.call("delete_friend", json!({ "user_id": user.0 })).await?;
        Ok(())
    }

    // OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/api/public.md
    // send_like:参数 {user_id, times}。每个好友每天上限 10 次。
    async fn send_profile_like(&self, user: Uin, count: u32) -> Result<()> {
        self.call("send_like", json!({ "user_id": user.0, "times": count })).await?;
        Ok(())
    }

    // ===== 群公告 =====

    // ENDPOINT: Lagrange Lagrange.OneBot/Core/Operation/Group/SetGroupMemoOperation.cs
    //   (https://github.com/LagrangeDev/Lagrange.Core);亦见 NapCat
    //   go-cqhttp/SendGroupNotice.ts。参数 {group_id, content, image?}。
    // 响应:Lagrange 返回裸的公告 id(new_fid);NapCat 返回 void;裸字符串与 `{notice_id}`
    // 两种形态都兼容。
    async fn send_group_announcement(
        &self,
        group: Uin,
        content: &str,
        image: Option<ResourceSource>,
    ) -> Result<String> {
        let mut params = json!({ "group_id": group.0, "content": content });
        if let Some(src) = image {
            // 与图片发送 / upload_group_file 相同的 资源→`file/image` 编码。
            let file = crate::encode::encode_source(&src);
            params["image"] = Value::String(file);
        }
        let data = self.call("_send_group_notice", params).await?;
        let id = match &data {
            Value::String(s) => s.clone(),
            other => data_str(other, "notice_id").or_else(|| data_str(other, "new_fid")).unwrap_or_default(),
        };
        Ok(id)
    }

    // ENDPOINT: Lagrange Lagrange.OneBot/Core/Operation/Group/GetGroupMemoOperation.cs
    //   (https://github.com/LagrangeDev/Lagrange.Core);亦见 NapCat/LLOneBot
    //   `_get_group_notice`。参数 {group_id}。响应:数组,元素为
    //   {notice_id, sender_id, publish_time, message:{text, images:[{id,...}]}}。
    async fn get_group_announcements(&self, group: Uin) -> Result<Vec<Announcement>> {
        let data = self.call("_get_group_notice", json!({ "group_id": group.0 })).await?;
        Ok(data.as_array().map(|a| a.iter().map(|v| announcement_from(group, v)).collect()).unwrap_or_default())
    }

    // ENDPOINT: Lagrange Lagrange.OneBot/Core/Operation/Group/DeleteGroupMemoOperation.cs
    //   (https://github.com/LagrangeDev/Lagrange.Core, `_del_group_notice`);NapCat
    //   `_del_group_notice`;LLOneBot src/onebot11/types.ts 用 `_delete_group_notice`。
    //   参数 {group_id, notice_id}。用别名回退兼容 LLOneBot。
    async fn delete_group_announcement(&self, group: Uin, announcement_id: &str) -> Result<()> {
        self.call_alias(
            "_del_group_notice",
            "_delete_group_notice",
            json!({ "group_id": group.0, "notice_id": announcement_id }),
        )
        .await?;
        Ok(())
    }

    // ===== 精华 =====

    // ENDPOINT: Lagrange Lagrange.OneBot/Core/Operation/Message/GetEssenceMessageListOperation.cs
    //   (https://github.com/LagrangeDev/Lagrange.Core);亦见 NapCat
    //   group/GetGroupEssence.ts、LLOneBot go-cqhttp/GetGroupEssence.ts。
    // 参数 {group_id}。响应:数组,元素为 {sender_id, sender_nick, operator_id,
    // operator_time, message_id[, content[]]}。NapCat 带 `content`;LLOneBot/Lagrange 省略
    // (→ 空消息)。
    async fn get_essence_messages(&self, group: Uin) -> Result<Vec<EssenceMessage>> {
        let data = self.call("get_essence_msg_list", json!({ "group_id": group.0 })).await?;
        Ok(data.as_array().map(|a| a.iter().map(|v| essence_from(group, v)).collect()).unwrap_or_default())
    }

    // ===== 群文件 =====

    // ENDPOINT: Lagrange Lagrange.OneBot/Core/Operation/File/GetGroupFilesOperation.cs
    //   (https://github.com/LagrangeDev/Lagrange.Core);亦见 NapCat
    //   go-cqhttp/GetGroupRootFiles.ts 与 GetGroupFilesByFolder.ts。
    // folder None → get_group_root_files {group_id};Some → get_group_files_by_folder
    // {group_id, folder_id}。响应 {files:[{file_id,file_name,file_size,busid}],
    // folders:[{folder_id,folder_name,total_file_count,create_time}]}。
    async fn get_group_files(&self, group: Uin, folder_id: Option<&str>) -> Result<GroupFileList> {
        let (action, params) = match folder_id {
            None => ("get_group_root_files", json!({ "group_id": group.0 })),
            Some(fid) => ("get_group_files_by_folder", json!({ "group_id": group.0, "folder_id": fid })),
        };
        let data = self.call(action, params).await?;
        let files = data
            .get("files")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().map(group_file_from).collect())
            .unwrap_or_default();
        let folders = data
            .get("folders")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().map(group_folder_from).collect())
            .unwrap_or_default();
        Ok(GroupFileList { files, folders })
    }

    // ENDPOINT: Lagrange Lagrange.OneBot/Core/Operation/File/GetPrivateFileUrlOperation.cs
    //   (https://github.com/LagrangeDev/Lagrange.Core);亦见 NapCat
    //   file/GetPrivateFileUrl.ts。参数 {user_id, file_id[, file_hash]}。
    // 响应 {url}。
    async fn get_private_file_download_url(&self, user: Uin, file_id: &str, hash: Option<&str>) -> Result<String> {
        let mut params = json!({ "user_id": user.0, "file_id": file_id });
        if let Some(h) = hash {
            // Lagrange 用文件内容哈希(`file_hash`)寻址文件。
            params["file_hash"] = Value::String(h.to_string());
        }
        let data = self.call("get_private_file_url", params).await?;
        Ok(data_str(&data, "url").unwrap_or_default())
    }

    // ENDPOINT: Lagrange Lagrange.OneBot/Core/Operation/File/CreateGroupFileFolderOperation.cs
    //   (https://github.com/LagrangeDev/Lagrange.Core);亦见 NapCat/LLOneBot
    //   `create_group_file_folder`。参数 {group_id, name}。响应形态各异
    //   (Lagrange `{msg}`、NapCat `{result, groupItem}`);尽力提取文件夹 id,取不到则空。
    async fn create_group_folder(&self, group: Uin, name: &str) -> Result<String> {
        let data = self.call("create_group_file_folder", json!({ "group_id": group.0, "name": name })).await?;
        let id = data_str(&data, "folder_id")
            .or_else(|| data.get("groupItem").and_then(|gi| data_str(gi, "folderId")))
            .or_else(|| data.get("groupItem").and_then(|gi| data_str(gi, "folder_id")))
            .unwrap_or_default();
        Ok(id)
    }

    // ENDPOINT: Lagrange Lagrange.OneBot/Core/Operation/File/RenameGroupFileFolderOperation.cs
    //   (https://github.com/LagrangeDev/Lagrange.Core);亦见 LLOneBot
    //   llbot/file/RenameGroupFileFolder.ts。参数 {group_id, folder_id, new_folder_name}。
    async fn rename_group_folder(&self, group: Uin, folder_id: &str, new_name: &str) -> Result<()> {
        self.call(
            "rename_group_file_folder",
            json!({ "group_id": group.0, "folder_id": folder_id, "new_folder_name": new_name }),
        )
        .await?;
        Ok(())
    }

    // ENDPOINT: NapCat packages/napcat-onebot/action/router.ts (delete_group_folder)
    //   (https://github.com/NapNeko/NapCatQQ);亦见 LLOneBot `delete_group_folder`。
    //   Lagrange 把它挂在 `delete_group_file_folder` 下
    //   (Lagrange.OneBot/Core/Operation/File/GroupFSOperations.cs)——用别名回退兼容。
    //   参数 {group_id, folder_id}。
    async fn delete_group_folder(&self, group: Uin, folder_id: &str) -> Result<()> {
        self.call_alias(
            "delete_group_folder",
            "delete_group_file_folder",
            json!({ "group_id": group.0, "folder_id": folder_id }),
        )
        .await?;
        Ok(())
    }

    // ENDPOINT: Lagrange Lagrange.OneBot/Core/Operation/File/MoveGroupFileOperation.cs
    //   (https://github.com/LagrangeDev/Lagrange.Core);亦见 LLOneBot
    //   llbot/file/MoveGroupFile.ts、NapCat extends/MoveGroupFile.ts。
    // 参数:Lagrange/LLOneBot {parent_directory, target_directory};NapCat
    // {current_parent_directory, target_parent_directory}。trait 签名没有*当前*父目录,
    // 故默认取根("/"),并同发两种目标拼写;对根目录下的文件是正确的。
    async fn move_group_file(
        &self,
        group: Uin,
        file_id: &str,
        source_folder_id: Option<&str>,
        target_folder_id: Option<&str>,
    ) -> Result<()> {
        let source = source_folder_id.unwrap_or("/");
        let target = target_folder_id.unwrap_or("/");
        // Lagrange/LLOneBot 用 parent_directory/target_directory;
        // NapCat 用 current_parent_directory/target_parent_directory。
        // 两种拼写都发,让各端都能命中。
        self.call(
            "move_group_file",
            json!({
                "group_id": group.0,
                "file_id": file_id,
                "parent_directory": source,
                "target_directory": target,
                "current_parent_directory": source,
                "target_parent_directory": target,
            }),
        )
        .await?;
        Ok(())
    }

    // ENDPOINT: LLOneBot src/onebot11/action/llbot/file/RenameGroupFile.ts
    //   (https://github.com/LLOneBot/LLOneBot);亦见 NapCat extends/RenameGroupFile.ts。
    // 参数 {group_id, file_id, current_parent_directory, new_name}。
    // source_folder_id 映射到 current_parent_directory;None → 根 "/"。
    async fn rename_group_file(
        &self,
        group: Uin,
        file_id: &str,
        source_folder_id: Option<&str>,
        new_name: &str,
    ) -> Result<()> {
        let source = source_folder_id.unwrap_or("/");
        self.call(
            "rename_group_file",
            json!({
                "group_id": group.0,
                "file_id": file_id,
                "current_parent_directory": source,
                "new_name": new_name,
            }),
        )
        .await?;
        Ok(())
    }

    // ===== 头像 / 资料 =====

    // ENDPOINT: Lagrange Lagrange.OneBot/Core/Operation/Group/SetGroupAvatarOperation.cs
    //   (https://github.com/LagrangeDev/Lagrange.Core);亦见 NapCat/LLOneBot
    //   `set_group_portrait`。参数 {group_id, file}(资源→file 编码)。
    async fn set_group_avatar(&self, group: Uin, src: ResourceSource) -> Result<()> {
        let file = crate::encode::encode_source(&src);
        self.call("set_group_portrait", json!({ "group_id": group.0, "file": file })).await?;
        Ok(())
    }

    // ENDPOINT: Lagrange Lagrange.OneBot/Core/Operation/Generic/SetAvatarOperation.cs
    //   (https://github.com/LagrangeDev/Lagrange.Core);亦见 NapCat/LLOneBot
    //   `set_qq_avatar`。参数 {file}(资源→file 编码)。
    async fn set_self_avatar(&self, src: ResourceSource) -> Result<()> {
        let file = crate::encode::encode_source(&src);
        self.call("set_qq_avatar", json!({ "file": file })).await?;
        Ok(())
    }

    // ENDPOINT: NapCat packages/napcat-onebot/action/go-cqhttp/SetQQProfile.ts
    //   (https://github.com/NapNeko/NapCatQQ);亦见 LLOneBot SetQQProfile。
    // 参数 {nickname, personal_note?}。NapCat 要求 `nickname`;这里只有新昵称
    // (personal_note 在 LLOneBot 上保持原值)。
    async fn set_self_nickname(&self, name: &str) -> Result<()> {
        self.call("set_qq_profile", json!({ "nickname": name })).await?;
        Ok(())
    }

    // ENDPOINT: NapCat packages/napcat-onebot/action/go-cqhttp/SetQQProfile.ts
    //   (https://github.com/NapNeko/NapCatQQ);亦见 LLOneBot SetQQProfile。
    // 参数 {nickname, personal_note}。NapCat 校验 `nickname` 必填,故先用
    // `get_login_info` 取当前昵称一并回传(避免在 NapCat 上被拒);取不到时退化为
    // 只发 personal_note(LLOneBot 允许)。
    async fn set_self_bio(&self, bio: &str) -> Result<()> {
        let params = match self.get_login_info().await {
            Ok((_, nick)) => json!({ "nickname": nick, "personal_note": bio }),
            Err(_) => json!({ "personal_note": bio }),
        };
        self.call("set_qq_profile", params).await?;
        Ok(())
    }

    // ===== 系统 =====

    // OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/api/public.md
    // get_csrf_token:官方响应字段 `csrf_token`(int32);Lagrange/NapCat 用 `token`(int)。
    // OneBot 把它定为 int32,但各厂商有的返回 JSON 数字、有的返回字符串,故两者都兼容。统一
    // trait 返回 `String`(一份契约横跨 OneBot 的 int 与 Milky 的 string token)——这是刻意的
    // 类型层权衡,**不是**值损失:数字路径把整数完整字符串化、不截断,需要 int 的调用方自行
    // 再解析。先 `data_i64`(覆盖数字 + 数字字符串),再 `data_str`(非数字字符串)。
    async fn get_csrf_token(&self) -> Result<String> {
        let data = self.call("get_csrf_token", json!({})).await?;
        Ok(csrf_token_from_data(&data))
    }

    async fn call_raw(&self, action: &str, params: Value) -> Result<Value> {
        let params = if params.is_null() { Value::Object(Map::new()) } else { params };
        self.call(action, params).await
    }

    // OFFICIAL: api/public.md get_credentials {domain} → {cookies, csrf_token}。
    // 用 OneBot 的单端点变体覆盖默认的组合实现。
    async fn get_credentials(&self, domain: Option<&str>) -> Result<(String, String)> {
        let data = self.call("get_credentials", json!({ "domain": domain.unwrap_or("") })).await?;
        let cookies = data_str(&data, "cookies").unwrap_or_default();
        // 与 `get_csrf_token` 相同的 int/string 兼容、无损提取。
        let csrf = csrf_token_from_data(&data);
        Ok((cookies, csrf))
    }
}

impl MilkyActions for OneBotAdapter {}
