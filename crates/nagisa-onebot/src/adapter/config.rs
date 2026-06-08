//! 适配器配置:传输模式 + `OneBotConfig` builder。
use std::net::SocketAddr;
use std::time::Duration;
use serde_json::Value;
use std::sync::Arc;

/// OneBot 收发传输模式。`Forward` 是默认(nagisa 作为 WS 客户端主动外连)。`ReverseWs` 让
/// nagisa 当 WS **服务端**,由协议端连进来。`Http` 把 HTTP-POST 事件上报与 HTTP API 耦在一起;
/// `HttpApi` 只是其中的动作半边(无 webhook)。
#[derive(Clone, Debug)]
pub enum OneBotTransport {
    /// nagisa 主动外连一个正向 WS 端点(既有默认)。
    Forward { url: String },
    /// nagisa 绑一个 TCP 监听器,在 `path` 上接受 WS 升级。
    ReverseWs { bind: SocketAddr, path: String },
    /// nagisa 经 `post_bind`+`post_path` 的 HTTP POST 收事件,并把动作发往 `api_url`。
    /// `secret`(若设)校验 `X-Signature: sha1=<hmac>`。
    Http { api_url: String, post_bind: SocketAddr, post_path: String, secret: Option<String> },
    /// 纯 HTTP-API **动作**客户端——把动作 POST 到 `api_url`,**不**附带 HTTP-POST webhook。
    /// 让「HTTP 动作 + WS 事件」可通过把本动作传输与独立的 `Forward`/`ReverseWs` 事件适配器
    /// 配对来表达(动作信道与事件信道可不同)。作为 [`EventSource`](nagisa_core::EventSource),
    /// 本模式不发任何事件;`run()` 空转直到 shutdown。
    HttpApi { api_url: String },
    /// LLOneBot 私有 HTTP 客户端：actions POST 到 `api_url`，事件经 `events`
    /// 选定的拉取方式获取（SSE `/_events` 推送流 或 `get_event` 长轮询）。无公网
    /// 回调 / 无 WS 时也能收事件，并与 webhook / forward-WS 走同一解码+分发路径。
    LLOneBotHttp { api_url: String, events: LLOneBotEventMode },
}

/// LLOneBot 纯 HTTP 客户端的事件拉取方式（[`OneBotTransport::LLOneBotHttp`]）。
#[derive(Clone, Debug)]
pub enum LLOneBotEventMode {
    /// 订阅 SSE `/_events` 流（推送式，优先）：服务端有事件即下发，断流自动重连。
    Sse,
    /// `get_event` 长轮询（回退式）：按 `interval` 周期反复排空后端事件队列。
    LongPoll { interval: Duration },
}

/// 适配器配置。端点 / 绑定完全落在 `mode` 里(如默认正向 WS 模式的
/// `OneBotTransport::Forward { url }`)。
#[derive(Clone, Debug)]
pub struct OneBotConfig {
    pub access_token: Option<String>,
    /// 传输模式。默认 `Forward { url }`。
    pub mode: OneBotTransport,
}

impl OneBotConfig {
    /// 正向 WS 配置(默认模式),如 `ws://127.0.0.1:8080/onebot/v11/ws`。
    pub fn new(url: impl Into<String>) -> Self {
        OneBotConfig {
            access_token: None,
            mode: OneBotTransport::Forward { url: url.into() },
        }
    }
    /// 反向 WS 配置:nagisa 绑 `bind`,在 `path` 上接受 WS 升级。
    pub fn reverse_ws(bind: SocketAddr, path: impl Into<String>) -> Self {
        OneBotConfig {
            access_token: None,
            mode: OneBotTransport::ReverseWs { bind, path: path.into() },
        }
    }
    /// HTTP-POST 配置:事件 POST 到 `post_bind`+`post_path`,动作发往 `api_url`。
    pub fn http(
        api_url: impl Into<String>,
        post_bind: SocketAddr,
        post_path: impl Into<String>,
    ) -> Self {
        OneBotConfig {
            access_token: None,
            mode: OneBotTransport::Http {
                api_url: api_url.into(),
                post_bind,
                post_path: post_path.into(),
                secret: None,
            },
        }
    }
    /// 纯 HTTP-API 动作配置:动作 POST 到 `api_url`,**不**起 webhook。与独立的纯事件适配器
    /// (`Forward`/`ReverseWs`)配对,即可实现「HTTP 动作 + WS 事件」的拆分。
    pub fn http_api(api_url: impl Into<String>) -> Self {
        OneBotConfig {
            access_token: None,
            mode: OneBotTransport::HttpApi { api_url: api_url.into() },
        }
    }
    /// LLOneBot HTTP 客户端 + SSE `/_events` 事件流：actions POST 到 `api_url`，
    /// 事件经 SSE 推送（首选）。无公网回调即可收事件。
    pub fn llonebot_http_sse(api_url: impl Into<String>) -> Self {
        OneBotConfig {
            access_token: None,
            mode: OneBotTransport::LLOneBotHttp {
                api_url: api_url.into(),
                events: LLOneBotEventMode::Sse,
            },
        }
    }
    /// LLOneBot HTTP 客户端 + `get_event` 长轮询事件源：actions POST 到 `api_url`，
    /// 每 `interval` 拉一批排队事件。SSE 不可用时的回退方案。
    pub fn llonebot_http_long_poll(api_url: impl Into<String>, interval: Duration) -> Self {
        OneBotConfig {
            access_token: None,
            mode: OneBotTransport::LLOneBotHttp {
                api_url: api_url.into(),
                events: LLOneBotEventMode::LongPoll { interval },
            },
        }
    }
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.access_token = Some(token.into());
        self
    }
    /// 设置 HTTP-POST 的 `X-Signature` 密钥(仅 `Http` 模式有意义)。
    pub fn with_secret(mut self, secret: impl Into<String>) -> Self {
        if let OneBotTransport::Http { secret: s, .. } = &mut self.mode {
            *s = Some(secret.into());
        }
        self
    }
}

/// 快速操作应答器:给定解码后的 [`Event`](nagisa_types::event::Event),可选地返回一个 JSON
/// 正文,回发给协议端以替代默认的 `204`。
/// 见 [`OneBotAdapter::with_quick_op`](super::OneBotAdapter::with_quick_op) 与 [OneBot v11 §6.1 quick-op](https://github.com/botuniverse/onebot-11/blob/master/communication/http.md)。
pub type QuickOpFn = Arc<dyn Fn(&nagisa_types::event::Event) -> Option<Value> + Send + Sync>;

/// 对 `access_token` 查询值做最小百分号编码:保留 RFC 3986 的非保留字符,其余一律 `%XX`
/// 转义。免得为罕见的非字母数字 token 引入一个 url crate。
pub(crate) fn encode_query(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
