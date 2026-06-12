//! Milky 适配器本体：[`MilkyAdapter`] 的构造、出站动作通道 `call`，以及共享辅助。
//!
//! 拆分后本模块只承载「与传输方式无关」的核心；事件源与动作映射分居 `sources` / `actions`
//! 私有模块：
//! - 构造 [`MilkyAdapter::new`]：从 `MilkyConfig::ws_url` 推导两个 URL——动作基址
//!   `ws://h:p{prefix}` → `http://h:p{prefix}api/`（`ws`→`http`、`wss`→`https`，末尾拼 `api/`），
//!   事件 URL 保持 ws scheme、末尾拼 `event`、带 `?access_token=`（若有 token）。
//! - 出站动作 `call`：`reqwest` `POST {api_base}{action}` + `Bearer` header + JSON body。
//!   Milky 无 echo、动作名在 URL 路径里，故无需 echo 关联。先按 HTTP 状态分支
//!   （`404`→`Unsupported`，`401`/`405`→`Action`），再解 `{status,retcode,data,message}` 封包；
//!   retcode 经 `classify_retcode` 启发式归类为 [`ActionErrorKind`]。`404`→`Unsupported` +
//!   封包成功检查走 [`nagisa_core::wire::http_action_envelope`] 公共骨架（与 OneBot 适配器共形状），
//!   Milky 专属的 `401`/`405` 预筛与封包字段/`classify_retcode` 留在本地。
//! - 共享辅助：`log_wire`（[`nagisa_core::wire::log_wire`] re-export，与 OneBot 适配器共用的
//!   `nagisa::wire` 日志漏斗）、`classify_retcode`、`join_path`。
//!
//! 动作 trait 的实现在 `actions` 模块（调用此处 `call`），[`EventSource`](nagisa_core::EventSource)
//! 的 ws/sse/webhook 入站路径在 `sources` / `webhook` 模块。`MilkyConfig`/`MilkyMode` 实体定义于
//! `config`，在此 re-export。
use std::sync::{Arc, OnceLock};

use nagisa_core::wire::{http_action_envelope, Envelope};
use nagisa_core::ImplInfo;
use nagisa_types::error::{ActionErrorKind, Error, Result, TransportError};
use nagisa_types::prelude::*;
use serde_json::Value;
use url::Url;

use crate::wire::ResponseEnvelope;

// 对外路径稳定：`MilkyConfig`/`MilkyMode` 实体在 config.rs，经此 re-export 保持
// `transport::{MilkyConfig, MilkyMode}` 可达（nagisa 门面 / nagisa-core 引用此路径）。
pub use crate::config::{MilkyConfig, MilkyMode};
// 协议帧日志漏斗与 OneBot 适配器共用 `nagisa::wire` target;经此 re-export 保持
// `transport::log_wire` 路径稳定（sources.rs / webhook.rs 引用）。
pub(crate) use nagisa_core::wire::log_wire;

/// Milky 1.2 协议适配器。
///
/// 单一实体同时是出站动作通道（[`ActionInvoker`](nagisa_core::adapter::ActionInvoker) +
/// [`MilkyActions`](nagisa_core::adapter::MilkyActions)，HTTP）与入站事件源
/// （[`EventSource`](nagisa_core::EventSource)，ws/sse/webhook 三选一）。由 [`MilkyConfig`] 构造，
/// 见 [`MilkyAdapter::new`]。
pub struct MilkyAdapter {
    pub(crate) http: reqwest::Client,
    /// 动作基址，确保以 `/` 结尾，便于 `join(action)`。
    api_base: Url,
    /// 事件 WS URL（已带 `?access_token=`，若有 token）。
    pub(crate) event_url: Url,
    /// 出站事件信道模式（默认 `Ws`，向后兼容）。`webhook` 配置时此项被忽略。
    pub(crate) mode: MilkyMode,
    /// WebHook 接收端 `(bind, path)`：配置后走 WebHook 独立运行路径(见 `run`)。
    pub(crate) webhook: Option<(String, String)>,
    /// 可选访问令牌（`pub(crate)` 供 webhook.rs 直接做 Bearer 校验）。
    pub(crate) access_token: Option<String>,
    /// 缓存机器人自身 uin（首次 `get_login_info` 成功后写入）。
    pub(crate) self_id: Arc<OnceLock<Uin>>,
    /// 首次连上后由 `get_impl_info` 探得的实现信息。
    pub(crate) impl_info: OnceLock<ImplInfo>,
}

impl MilkyAdapter {
    /// 由 [`MilkyConfig`] 构造适配器，从 `ws_url` 推导动作基址与事件 URL。
    ///
    /// `ws_url` 须为 `ws`/`wss`/`http`/`https` scheme（其余报 `Transport`）；其 path 前缀
    /// 会保留并分别拼上 `api/`（动作基址，scheme 改为 http/https）与 `event`（事件 URL，保持
    /// ws/wss 供 ws pump，sse pump 再换回 http/https）。token 仅追加到事件 URL 的 query；
    /// 动作调用与 webhook 校验各自带 `Bearer`。
    pub fn new(config: MilkyConfig) -> Result<Self> {
        let ws_url =
            Url::parse(&config.ws_url).map_err(|e| Error::Transport(TransportError::WebSocket(e.to_string())))?;

        // 动作基址：ws→http、wss→https，path 末尾拼 `api/`。
        let http_scheme = match ws_url.scheme() {
            "ws" | "http" => "http",
            "wss" | "https" => "https",
            other => return Err(Error::Transport(TransportError::Http(format!("unsupported scheme: {other}")))),
        };
        let mut api_base = ws_url.clone();
        api_base
            .set_scheme(http_scheme)
            .map_err(|_| Error::Transport(TransportError::Http("set_scheme failed".into())))?;
        let api_path = join_path(api_base.path(), "api/");
        api_base.set_path(&api_path);
        api_base.set_query(None);

        // 事件 URL：保持 ws scheme，path 末尾拼 `event`，带 access_token query。
        let mut event_url = ws_url.clone();
        let event_path = join_path(ws_url.path(), "event");
        event_url.set_path(&event_path);
        if let Some(token) = &config.access_token {
            event_url.query_pairs_mut().append_pair("access_token", token);
        }

        Ok(Self {
            http: reqwest::Client::new(),
            api_base,
            event_url,
            mode: config.mode,
            webhook: config.webhook,
            access_token: config.access_token,
            self_id: Arc::new(OnceLock::new()),
            impl_info: OnceLock::new(),
        })
    }

    /// 返回服务端在连接时上报的实现信息(若已探得)。
    pub fn impl_info(&self) -> Option<&ImplInfo> {
        self.impl_info.get()
    }

    /// 发送一个动作，返回 `data`。先按 HTTP 状态分支，再解封包。
    pub(crate) async fn call(&self, action: &str, params: Value) -> Result<Value> {
        let url = self.api_base.join(action).map_err(|e| Error::Transport(TransportError::Http(e.to_string())))?;

        // Milky 经 HTTP 调动作:无 echo、action 在 URL,故单独带 action 字段 + params 作正文。
        tracing::debug!(target: "nagisa::wire", dir = "out", action = %action, "{params}");
        let mut req = self.http.post(url).header(reqwest::header::CONTENT_TYPE, "application/json").json(&params);
        if let Some(token) = &self.access_token {
            req = req.bearer_auth(token);
        }

        let resp = req.send().await.map_err(|e| Error::Transport(TransportError::Http(e.to_string())))?;

        let status = resp.status();
        // Milky 专属的非 2xx 预筛(不进 core 骨架):401/405 不是 {status,retcode,data} 封包,
        // 在读 body 之前就归为 Action 错(401→鉴权失败、405→其他)。404 与封包成功检查 / classify
        // 是两适配器共形状的部分,交 `http_action_envelope` 收尾。
        if status == reqwest::StatusCode::METHOD_NOT_ALLOWED || status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(Error::Action {
                retcode: status.as_u16() as i64,
                message: format!("HTTP {status}"),
                kind: if status == reqwest::StatusCode::UNAUTHORIZED {
                    ActionErrorKind::AuthFailed
                } else {
                    ActionErrorKind::Other
                },
            });
        }

        let body = resp.text().await.map_err(|e| Error::Transport(TransportError::Http(e.to_string())))?;
        log_wire("in", &body); // HTTP 动作响应。
                               // 公共骨架:404→Unsupported + 封包成功检查 + classify。封包字段(milky 用 `message`)与
                               // retcode 语义(`classify_retcode`)是 milky 专属,经 `parse` 闭包填进统一 `Envelope`。
        http_action_envelope(action, status.as_u16(), &body, |body| {
            let env: ResponseEnvelope = serde_json::from_str(body)?;
            Ok(Envelope {
                classify: classify_retcode(env.retcode),
                status: env.status,
                retcode: env.retcode,
                data: env.data,
                message: env.message,
            })
        })
    }
}

/// 启发式归类 retcode（绝不按精确值匹配业务语义）。
fn classify_retcode(retcode: i64) -> ActionErrorKind {
    match retcode {
        // Milky ApiException -1 = 不支持/未找到。
        -1 => ActionErrorKind::Unsupported,
        // Milky ApiException -2 = 消息未找到。
        -2 => ActionErrorKind::NotFound,
        -400 => ActionErrorKind::BadParams,
        -403 | 401 | 403 => ActionErrorKind::AuthFailed,
        _ => ActionErrorKind::Other,
    }
}

/// 拼接 URL path：保证 base 以 `/` 结尾再接 segment。
fn join_path(base: &str, segment: &str) -> String {
    let trimmed = base.trim_end_matches('/');
    if trimmed.is_empty() {
        format!("/{segment}")
    } else {
        format!("{trimmed}/{segment}")
    }
}
