//! OneBot 反向 WebSocket 服务端模式:nagisa 当 WS **服务端**,协议端(Lagrange/NapCat 默认)连进来。
//! 事件跑与正向 WS 相同的解码管线;动作经适配器的 `server_outbound` 槽沿 accept 的 socket 回送。
//! echo 关联 + pending map 复用。
use crate::adapter::OneBotAdapter;
use futures_util::{SinkExt, StreamExt};
use nagisa_core::ShutdownToken;
use nagisa_types::error::Result;
use nagisa_types::event::Event;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// 反向 WS 客户端声明的角色(`X-Client-Role`)。一个客户端只以其中之一连入;`Universal`
/// (以及缺失该头,为向后兼容按 `Universal` 处理)在一个 socket 上同时承载事件与动作调用,
/// `Event` 只承载事件,`Api` 只承载动作调用。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClientRole {
    /// 一个 socket 上既有事件又有动作(默认 / 缺头)。
    Universal,
    /// 只有事件——此 socket 不可用作动作路由目标。
    Event,
    /// 只有动作——此 socket 不承载事件。
    Api,
}

impl ClientRole {
    /// 解析 `X-Client-Role` 头值(大小写不敏感)。`None` → `Universal`。无法识别的非空值产出
    /// `None`(由调用方拒绝握手)。
    fn parse(raw: Option<&str>) -> Option<Self> {
        match raw {
            None => Some(ClientRole::Universal),
            Some(v) => match v.trim().to_ascii_lowercase().as_str() {
                "universal" => Some(ClientRole::Universal),
                "event" => Some(ClientRole::Event),
                "api" => Some(ClientRole::Api),
                _ => None,
            },
        }
    }
    /// 此角色的 socket 是否可往外路由动作帧。
    fn carries_actions(self) -> bool {
        matches!(self, ClientRole::Universal | ClientRole::Api)
    }
    /// 此角色的 socket 上是否预期有入站事件帧。
    fn carries_events(self) -> bool {
        matches!(self, ClientRole::Universal | ClientRole::Event)
    }
}

/// 跑反向 WS 服务端直到 shutdown。
pub async fn run_reverse_ws(
    adapter: Arc<OneBotAdapter>,
    bind: SocketAddr,
    path: String,
    sink: mpsc::Sender<Event>,
    shutdown: ShutdownToken,
) -> Result<()> {
    let listener = TcpListener::bind(bind).await.map_err(|e| {
        nagisa_types::error::Error::Transport(nagisa_types::error::TransportError::WebSocket(
            e.to_string(),
        ))
    })?;
    tracing::info!(%bind, %path, "onebot reverse-ws listening");

    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => return Ok(()),
            accepted = listener.accept() => {
                let (stream, peer) = match accepted {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(error = %e, "reverse-ws accept failed");
                        continue;
                    }
                };
                let adapter = Arc::clone(&adapter);
                let sink = sink.clone();
                let shutdown = shutdown.clone();
                let want_path = path.clone();
                tokio::spawn(async move {
                    if let Err(e) = serve_conn(adapter, stream, peer, want_path, sink, shutdown).await {
                        tracing::debug!(error = %e, "reverse-ws connection ended");
                    }
                });
            }
        }
    }
}

// 握手回调的 `Err` 变体类型由 tokio-tungstenite 的 `Callback` trait 固定(一个完整的
// `http::Response`),故 large-err 体积无法避免。
#[allow(clippy::result_large_err)]
async fn serve_conn(
    adapter: Arc<OneBotAdapter>,
    stream: tokio::net::TcpStream,
    peer: SocketAddr,
    want_path: String,
    sink: mpsc::Sender<Event>,
    shutdown: ShutdownToken,
) -> Result<()> {
    // 握手期间校验 path + 鉴权。已配置的 token(若有)必须匹配 `Authorization: Bearer <t>` 或
    // `?access_token=<t>` 之一。
    let want_token = adapter_token(&adapter);
    // 握手回调还读 `X-Client-Role` / `X-Self-ID`;解析出的值暂存这里供连接循环查阅。用 `Mutex`
    // (而非 `Cell`/`RefCell`),使捕获的句柄保持 `Sync`——future 会 `tokio::spawn` 到多线程运行时。
    // 回调在握手期间同步执行,故锁绝不跨 `.await`。
    let handshake_meta = std::sync::Mutex::new((ClientRole::Universal, None::<String>));
    let callback = |req: &Request, resp: Response| -> std::result::Result<Response, ErrorResponse> {
        // 路径检查。
        if req.uri().path() != want_path {
            return Err(err_response(StatusCode::NOT_FOUND));
        }
        // 鉴权检查(仅在配置了 token 时)。
        if let Some(want) = &want_token {
            let header_ok = req
                .headers()
                .get("Authorization")
                .and_then(|v| v.to_str().ok())
                .map(|v| v == format!("Bearer {want}"))
                .unwrap_or(false);
            let query_ok = req
                .uri()
                .query()
                .map(|q| q.split('&').any(|kv| kv == format!("access_token={want}")))
                .unwrap_or(false);
            if !header_ok && !query_ok {
                return Err(err_response(StatusCode::UNAUTHORIZED));
            }
        }
        // `X-Client-Role`:API / Event / Universal。缺头按 Universal 兼容;无法识别的值是协议违规
        // → 回 400 拒绝,而非默默错处理。
        let role_raw = req.headers().get("X-Client-Role").and_then(|v| v.to_str().ok());
        let role = match ClientRole::parse(role_raw) {
            Some(role) => role,
            None => return Err(err_response(StatusCode::BAD_REQUEST)),
        };
        // `X-Self-ID`:连入 bot 的 QQ uin(信息性;用于日志 / 未来多账号路由)。原样读取,绝不强制。
        let self_id = req
            .headers()
            .get("X-Self-ID")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        *handshake_meta.lock().expect("handshake_meta poisoned") = (role, self_id);
        Ok(resp)
    };

    let ws = tokio_tungstenite::accept_hdr_async(stream, callback)
        .await
        .map_err(|e| {
            nagisa_types::error::Error::Transport(nagisa_types::error::TransportError::WebSocket(
                e.to_string(),
            ))
        })?;
    let (role, self_id) = handshake_meta.into_inner().expect("handshake_meta poisoned");
    tracing::info!(
        %peer,
        ?role,
        carries_events = role.carries_events(),
        self_id = self_id.as_deref().unwrap_or("-"),
        "reverse-ws client connected"
    );
    // 传输层连上：仅对**承载事件**的 socket 发 Meta::Connect（反向 WS 可能有多个角色客户端
    // Universal/Event/Api，只有事件流那条算「协议端连接」，否则会为同一逻辑连接重复发）。
    let carries_events = role.carries_events();
    if carries_events {
        let _ = sink.try_send(nagisa_types::event::Event::Meta(nagisa_types::event::Meta::Connect));
    }
    let (mut write, mut read) = ws.split();

    // 只对承载动作的角色(Universal / Api)安装动作路由发送端。只发事件的 socket 不可成为动作
    // 目标,故其槽保持不动(动作继续路由到任一已连的 Universal/Api socket——若有)。
    let installed_action_route = role.carries_actions();
    // 在只发事件的路径上,发送端必须比循环活得久:若在此 drop,`out_rx.recv()` 会立刻产出 `None`
    // 并过早关掉(仍然有用的)事件 socket。故整条连接期间都把它绑住;它从不被发送,使那条
    // `select!` 分支永久挂起。对承载动作的角色,发送端则存于 `server_outbound`。
    let (out_tx, mut out_rx) = mpsc::channel::<String>(256);
    let _idle_out_tx_guard = if installed_action_route {
        *adapter.server_outbound.lock().expect("server_outbound poisoned") = Some(out_tx);
        None
    } else {
        Some(out_tx)
    };

    // 刻意不套 core 的 framesource::pump:本路径是被动服务端,本无 idle 看门狗/断线重连语义,
    // 套泵会凭空注入 90s 空闲断连(行为变更)。故走自有 `select!` 循环。
    let res = loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => {
                let _ = write.close().await;
                break Ok(());
            }
            maybe_frame = out_rx.recv() => {
                match maybe_frame {
                    Some(frame) => {
                        if write.send(WsMessage::text(frame)).await.is_err() {
                            break Ok(());
                        }
                    }
                    None => break Ok(()),
                }
            }
            maybe_msg = read.next() => {
                match maybe_msg {
                    // `handle_inbound_public` 返回 true = 下游 sink 已关闭(消费方走了):
                    // 收束本连接(break Ok),不再读。
                    Some(Ok(WsMessage::Text(txt))) => {
                        if adapter.handle_inbound_public(txt.as_str(), &sink).await {
                            break Ok(());
                        }
                    }
                    Some(Ok(WsMessage::Binary(bin))) => {
                        if let Ok(txt) = std::str::from_utf8(&bin) {
                            if adapter.handle_inbound_public(txt, &sink).await {
                                break Ok(());
                            }
                        }
                    }
                    Some(Ok(WsMessage::Ping(p))) => { let _ = write.send(WsMessage::Pong(p)).await; }
                    Some(Ok(WsMessage::Close(_))) | None => break Ok(()),
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        tracing::warn!(error = %e, "reverse-ws read error");
                        break Ok(());
                    }
                }
            }
        }
    };

    // 收尾:仅当**本** socket 安装过动作路由槽时才清它(只发事件的 socket 从未拥有它;无条件清
    // 会抹掉兄弟 Universal/Api socket 的槽)。让绑在它上的挂起调用失败。
    if installed_action_route {
        *adapter.server_outbound.lock().expect("server_outbound poisoned") = None;
        adapter.clear_pending_public();
    }
    // 传输层断开：与连接对称，仅对承载事件的 socket 发 Meta::Disconnect（反向 WS 这里以
    // Ok 收束、无独立错误文案，故 reason 为 None）。
    if carries_events {
        let _ = sink.try_send(nagisa_types::event::Event::Meta(
            nagisa_types::event::Meta::Disconnect { reason: None },
        ));
    }
    res
}

fn adapter_token(adapter: &OneBotAdapter) -> Option<String> {
    adapter.access_token().map(str::to_string)
}

fn err_response(code: StatusCode) -> ErrorResponse {
    let mut resp = ErrorResponse::new(None);
    *resp.status_mut() = code;
    resp
}
