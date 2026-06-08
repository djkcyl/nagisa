//! OneBot 的 HTTP 传输:HTTP-POST webhook(`Http` 模式)与 LLOneBot 拉取式事件源
//! (`LLOneBotHttp` 模式)。两者的动作都经 HTTP API 出站(适配器私有的 `call_http`);没有常驻
//! socket,故动作路径上没有 idle 看门狗。
//!
//! - **HTTP-POST webhook**([`run_http_post`]):协议端把事件 POST 到 `post_bind`+`post_path`,
//!   带 `X-Signature: sha1=<hmac-sha1>`,配了密钥时校验。处理器经共享管线解码,若设了 quick-op
//!   hook 且返回了正文,则回 `200` + 该 JSON,而非默认的 `204`。
//! - **LLOneBot HTTP 事件源**([`run_llonebot_http`]):给纯 HTTP 客户端用的拉取式事件信道(无公网
//!   webhook 回调)。两种子模式,都灌入适配器的 decode→sink 管线:
//!   - SSE `/_events`——OneBot 事件的 `text/event-stream`,每帧 `data:` 一条事件;断流/空闲时带
//!     退避重连。
//!   - `get_event` 长轮询——经 `get_event` 动作周期性排空后端事件队列。
use crate::adapter::{LLOneBotEventMode, OneBotAdapter};
use nagisa_core::adapter::ActionInvoker; // 为了 `adapter.vendor()`(trait 方法)
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Router;
use hmac::{Hmac, KeyInit, Mac};
use nagisa_core::adapter::OneBotActions; // 为了 `adapter.get_event()`(trait 方法)
use nagisa_core::ShutdownToken;
use nagisa_types::error::{Error, Result, TransportError};
use nagisa_types::event::Event;
use sha1::Sha1;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

#[derive(Clone)]
struct HttpState {
    adapter: Arc<OneBotAdapter>,
    sink: mpsc::Sender<Event>,
    secret: Option<String>,
}

/// 跑 HTTP-POST 服务端直到 shutdown。
#[allow(clippy::too_many_arguments)]
pub async fn run_http_post(
    adapter: Arc<OneBotAdapter>,
    api_url: String,
    post_bind: SocketAddr,
    post_path: String,
    secret: Option<String>,
    sink: mpsc::Sender<Event>,
    shutdown: ShutdownToken,
) -> Result<()> {
    // 装好 HTTP-API 客户端,使 `call_http` 能把动作路由出去。
    adapter.install_http_api(reqwest::Client::new(), api_url);

    let state = HttpState { adapter, sink, secret };
    let app = Router::new()
        .route(&post_path, post(handle_post))
        .with_state(state);

    let listener = TcpListener::bind(post_bind)
        .await
        .map_err(|e| Error::Transport(TransportError::Http(e.to_string())))?;
    tracing::info!(%post_bind, %post_path, "onebot http-post webhook listening");

    let sd = shutdown.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move { sd.cancelled().await })
        .await
        .map_err(|e| Error::Transport(TransportError::Http(e.to_string())))?;
    Ok(())
}

/// `POST <post_path>`:校验签名(若配置),decode → Event,推送。若适配器有 quick-op hook 且对
/// 该解码事件返回 `Some(json)`,则回 `200 OK` + 该 JSON;否则回 `204 NO_CONTENT`。
async fn handle_post(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // X-Signature 校验(仅在配置了密钥时)。
    if let Some(secret) = &state.secret {
        let provided = headers
            .get("X-Signature")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !verify_signature(secret, &body, provided) {
            tracing::warn!("http-post: X-Signature mismatch; rejecting");
            return (StatusCode::UNAUTHORIZED, axum::body::Body::empty()).into_response();
        }
    }
    // Decode → Event(绝不 panic;解不开的经 decode 管线降级为 Raw)。
    let txt = match std::str::from_utf8(&body) {
        Ok(t) => t,
        Err(_) => return (StatusCode::BAD_REQUEST, axum::body::Body::empty()).into_response(),
    };
    // 分发入站并取回解码出的事件(供 quick-op hook)。
    let maybe_event = state.adapter.dispatch_and_decode(txt, &state.sink).await;
    // quick-op:若设了 hook 且返回 Some(json),回 200 + 正文。
    if let Some(event) = maybe_event {
        if let Some(quick_resp) = state.adapter.quick_op_response(&event) {
            let json_body = match serde_json::to_vec(&quick_resp) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!("quick-op: failed to serialize response: {e}");
                    return (StatusCode::INTERNAL_SERVER_ERROR, axum::body::Body::empty()).into_response();
                }
            };
            return (
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                json_body,
            ).into_response();
        }
    }
    (StatusCode::NO_CONTENT, axum::body::Body::empty()).into_response()
}

/// 近似常数时间地校验 `sha1=<hex>` 的 HMAC-SHA1。
fn verify_signature(secret: &str, body: &[u8], provided: &str) -> bool {
    let Some(hex) = provided.strip_prefix("sha1=") else { return false };
    let Ok(mut mac) = Hmac::<Sha1>::new_from_slice(secret.as_bytes()) else { return false };
    mac.update(body);
    let expected = mac.finalize().into_bytes();
    let expected_hex: String = expected.iter().map(|b| format!("{b:02x}")).collect();
    // 用 `hmac` 的 verify_slice 本是最佳,但我们只有 hex;故大小写不敏感、长度校验后做常数时间
    // 比较。
    constant_time_eq(expected_hex.as_bytes(), hex.to_ascii_lowercase().as_bytes())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// ───────────────────────── LLOneBot HTTP 事件源 ─────────────────────────

/// SSE 重连退避上限（与 forward-WS 看门狗节奏一致）。
const SSE_MAX_BACKOFF_MS: u64 = 30_000;
/// SSE 空闲看门狗：超过此时长无任何入站数据即判定半开连接，断开重连。
const SSE_IDLE_TIMEOUT: Duration = Duration::from_secs(90);

/// LLOneBot 私有 HTTP 事件源入口（`OneBotTransport::LLOneBotHttp`）。先装好 HTTP-API
/// 客户端使 `call_http` / `get_event` 可用，再按 `events` 选定方式拉事件灌入 `sink`，
/// 与 webhook / forward-WS 走同一解码+分发路径。仅在 `shutdown` 触发时返回。
pub async fn run_llonebot_http(
    adapter: Arc<OneBotAdapter>,
    api_url: String,
    events: LLOneBotEventMode,
    sink: mpsc::Sender<Event>,
    shutdown: ShutdownToken,
) -> Result<()> {
    // 装好 HTTP-API 客户端,使 `call_http`/`get_event` 能把动作路由出去。
    adapter.install_http_api(reqwest::Client::new(), api_url.clone());
    match events {
        LLOneBotEventMode::Sse => run_llonebot_sse(adapter, api_url, sink, shutdown).await,
        LLOneBotEventMode::LongPoll { interval } => {
            run_llonebot_long_poll(adapter, interval, sink, shutdown).await
        }
    }
}

/// SSE `/_events` 订阅循环：GET `{api_url}/_events`（`Accept: text/event-stream`），
/// 逐 chunk 缓冲行，遇空行即把累积的 `data:` 行拼成单条事件 JSON 交 adapter 解码并推
/// `sink`。断流 / 空闲超时即带退避重连。仅在 `shutdown` 触发时返回 `Ok`。
async fn run_llonebot_sse(
    adapter: Arc<OneBotAdapter>,
    api_url: String,
    sink: mpsc::Sender<Event>,
    shutdown: ShutdownToken,
) -> Result<()> {
    // `/_events` 是 LLOneBot 在 HTTP-server 上专有的事件订阅端点（与 HTTP-API 同基址）。
    let url = format!("{}/_events", api_url.trim_end_matches('/'));
    let client = reqwest::Client::new();

    // 重连退避：起点 500ms、封顶 SSE_MAX_BACKOFF_MS、倍率 2、无 jitter。
    // 关键约定:**不**在 clean end 重置退避——否则一个立刻断流的异常端会被以 ~2 req/s
    // 热重连(每次 end 都回 500ms),永远到不了封顶；故退避统一只倍增、封顶,由 reconnect
    // helper 的「Step::Reconnect 不区分 Ok(false)/Err」保证。SSE 路径不发 Meta::Disconnect。
    let backoff = nagisa_core::reconnect::Backoff::new(
        Duration::from_millis(500),
        Duration::from_millis(SSE_MAX_BACKOFF_MS),
    );
    nagisa_core::reconnect::run(&shutdown, backoff, |_| 0, || async {
        match sse_connect_and_pump(&adapter, &client, &url, &sink, &shutdown).await {
            // 流中途观察到 shutdown。
            Ok(true) => nagisa_core::reconnect::Step::Stop,
            // 流结束 / 空闲 → 带退避重连。
            Ok(false) => nagisa_core::reconnect::Step::Reconnect,
            Err(e) => {
                tracing::warn!(error = %e, "llonebot sse /_events disconnected; reconnecting");
                nagisa_core::reconnect::Step::Reconnect
            }
        }
    })
    .await
}

/// 一次 SSE 连接 + 读取周期。`Ok(true)` = 观察到 shutdown / sink 关闭;`Ok(false)` = 流结束 /
/// 空闲(重连);`Err` = 连接/HTTP 错误(带退避重连)。
///
/// 连上后交 [`nagisa_core::framesource::pump`] 跑 idle 看门狗循环(`SSE_IDLE_TIMEOUT` 不变),
/// 仅把「取下一帧」的差异下放给 [`LLOneBotSseSource`](reqwest 增量 chunk →
/// [`nagisa_core::sse::SseParser`]),每个凑齐的事件 payload 经 dispatch 回调汇入
/// [`OneBotAdapter::dispatch_sse_event`](沿用其 null 跳过 + `dir=in` 记帧 + 解码降级)。
/// **本路径刻意不发 `Meta::Disconnect`**(见 `run_llonebot_sse` 注释):pump 的终止只回到外层
/// reconnect 站点,该站点不发 Disconnect,故语义保持原样。
async fn sse_connect_and_pump(
    adapter: &Arc<OneBotAdapter>,
    client: &reqwest::Client,
    url: &str,
    sink: &mpsc::Sender<Event>,
    shutdown: &ShutdownToken,
) -> Result<bool> {
    let mut req = client.get(url).header(reqwest::header::ACCEPT, "text/event-stream");
    if let Some(token) = adapter.access_token() {
        req = req.bearer_auth(token);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| Error::Transport(TransportError::Http(e.to_string())))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(Error::Transport(TransportError::Http(format!(
            "llonebot sse GET /_events: HTTP {status}"
        ))));
    }
    tracing::info!("llonebot sse /_events connected: {url}");

    // 连接已建立:idle 看门狗循环交 core 骨架;SSE 只提供「取下一帧」的差异实现
    // `LLOneBotSseSource`(reqwest chunk → 纯 SseParser)。SSE 无站点前置序(不发 Meta::Connect、
    // 无能力探测),故连上后直接进 pump。
    let mut source = LLOneBotSseSource { resp, parser: nagisa_core::sse::SseParser::new() };
    nagisa_core::framesource::pump(&mut source, SSE_IDLE_TIMEOUT, shutdown, |payload| async move {
        adapter.dispatch_sse_event(&payload, sink).await
    })
    .await
}

/// LLOneBot SSE 帧源:reqwest 增量 chunk → 纯 [`SseParser`](nagisa_core::sse::SseParser) 凑齐事件
/// payload。与 Milky 的 `SseSource` 同形(零出站、纯读),`close` 用默认 no-op(SSE 无需收尾)。
struct LLOneBotSseSource {
    resp: reqwest::Response,
    parser: nagisa_core::sse::SseParser,
}

#[async_trait::async_trait]
impl nagisa_core::framesource::FrameSource for LLOneBotSseSource {
    async fn next_frame(&mut self) -> nagisa_core::framesource::Frame {
        use nagisa_core::framesource::Frame;
        // `chunk()` 是 reqwest 无需 `stream` feature 的增量读取入口（Deps: none）。
        match self.resp.chunk().await {
            // 入站数据:喂入解析器,凑齐的事件 payload 作为 Inbound 上交(空 Vec 也算入站,重置 idle)。
            Ok(Some(c)) => Frame::Inbound(self.parser.feed(&c)),
            // 流正常结束 → 断开重连。
            Ok(None) => Frame::Closed,
            Err(e) => {
                tracing::warn!("llonebot sse /_events error: {e}");
                Frame::Closed
            }
        }
    }
}

/// `get_event` 长轮询循环：按 `interval` 周期调用 `get_event` 排空后端事件队列，逐条
/// 推入 `sink`。SSE 不可用时的回退事件源。仅在 `shutdown` 触发时返回 `Ok`。错误（后端
/// 暂时不可达 / 不支持）只记日志并按 `interval` 重试，不终止循环。
async fn run_llonebot_long_poll(
    adapter: Arc<OneBotAdapter>,
    interval: Duration,
    sink: mpsc::Sender<Event>,
    shutdown: ShutdownToken,
) -> Result<()> {
    tracing::info!(?interval, "llonebot get_event long-poll started");
    loop {
        if shutdown.is_cancelled() {
            return Ok(());
        }
        match adapter.get_event().await {
            Ok(events) => {
                // 与所有入站路径共用 `dispatch_event` 的「丢冗余帧 + 按厂商归一化 at 段 name」
                // 入站契约（见 adapter::prepare_inbound），不再在此内联重写。
                let vendor = adapter.vendor();
                for event in events {
                    // `dispatch_event` 返回 true = 下游 sink 已关闭：消费方走了,长轮询无意义,收束。
                    if crate::adapter::dispatch_event(&sink, event, vendor).await {
                        return Ok(());
                    }
                }
            }
            // 后端的瞬时错误(不可达 / 尚未起来 / 此构建不支持)不该弄死轮询器——记日志,下一拍重试。
            Err(e) => {
                tracing::warn!(error = %e, "llonebot get_event poll failed; retrying");
            }
        }
        tokio::select! {
            _ = shutdown.cancelled() => return Ok(()),
            _ = tokio::time::sleep(interval) => {}
        }
    }
}
