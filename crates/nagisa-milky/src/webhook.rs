//! Milky WebHook 事件接收端：经 [`MilkyConfig::with_webhook`](crate::MilkyConfig::with_webhook)
//! 配置的独立反向信道（规范「通信」§ WebHook）。
//!
//! 协议端以 `POST {bind}{path}` 推送事件，body 为 `{time, self_id, event_type, data}` JSON，
//! 带 `Authorization: Bearer <access_token>`、`Content-Type: application/json`，无签名头
//! （区别于 OneBot HTTP-POST 的 `X-Signature` HMAC）。
//!
//! 实现：`run_webhook` 起一个 axum 服务端常驻至 shutdown（不重连，区别于 ws/sse 出站信道）。
//! 每条请求校验 Bearer（若配置了 token），把 body 经与 ws/sse 共享的 `dispatch_event` 解码并
//! 推入 sink，回 `200 OK`。Token 不匹配 → `401`；body 非 UTF-8 → `400`。
use crate::transport::MilkyAdapter;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::Router;
use nagisa_core::ShutdownToken;
use nagisa_types::error::{Error, Result, TransportError};
use nagisa_types::event::Event;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

#[derive(Clone)]
struct WebHookState {
    adapter: Arc<MilkyAdapter>,
    sink: mpsc::Sender<Event>,
    /// 可选访问令牌；配置后逐请求校验 `Authorization: Bearer <token>`。
    access_token: Option<String>,
}

/// 起 WebHook 接收服务端，运行至 shutdown。
pub(crate) async fn run_webhook(
    adapter: Arc<MilkyAdapter>,
    bind: &str,
    path: &str,
    sink: mpsc::Sender<Event>,
    shutdown: ShutdownToken,
) -> Result<()> {
    let access_token = adapter.access_token.clone();
    let state = WebHookState { adapter, sink, access_token };
    let app = Router::new().route(path, post(handle_post)).with_state(state);

    let listener = TcpListener::bind(bind).await.map_err(|e| Error::Transport(TransportError::Http(e.to_string())))?;
    tracing::info!(%bind, %path, "milky webhook receiver listening");

    let sd = shutdown.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move { sd.cancelled().await })
        .await
        .map_err(|e| Error::Transport(TransportError::Http(e.to_string())))?;
    Ok(())
}

/// `POST <path>`：校验 Bearer（若配置），decode → Event，推入 sink，回 `200`。
async fn handle_post(State(state): State<WebHookState>, headers: HeaderMap, body: Bytes) -> StatusCode {
    // Bearer 校验（仅当配置了 access_token）。Milky 用 Bearer，无 HMAC 签名头。
    if let Some(token) = &state.access_token {
        let provided = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or("");
        if provided != token {
            tracing::warn!("milky webhook: Bearer mismatch; rejecting");
            return StatusCode::UNAUTHORIZED;
        }
    }
    // decode → Event（绝不 panic；结构性破坏经 dispatch_event 降级为 Raw）。
    match std::str::from_utf8(&body) {
        Ok(txt) => {
            // 下游 sink 已关闭时 dispatch_event 返回 true；接收端仍回 200（无回压语义）。
            // 复用 ws/sse/webhook 同一条 decode→sink 路径（dispatch_event 为 pub(crate)）。
            let _ = state.adapter.dispatch_event(txt, &state.sink).await;
        }
        Err(_) => return StatusCode::BAD_REQUEST,
    }
    StatusCode::OK
}
