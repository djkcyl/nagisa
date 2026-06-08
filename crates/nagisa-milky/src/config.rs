//! Milky 适配器配置：[`MilkyMode`]（出站事件信道）+ [`MilkyConfig`]（含链式 builder）。
//!
//! 实体定义于此私有模块，由 [`transport`](crate::transport) re-export，外部统一从
//! `nagisa_milky::{MilkyConfig, MilkyMode}` 或 `transport::{…}` 引用。

/// Milky **出站事件信道**模式：[`Ws`](MilkyMode::Ws) / [`Sse`](MilkyMode::Sse) 二选一
/// （nagisa 主动连协议端的 `/event`）。默认 `Ws`（向后兼容）。
///
/// WebHook（反向信道：nagisa 起服务端、协议端 POST 事件）**不在**此枚举里——它是独立的
/// 构造 / 运行路径，经 [`MilkyConfig::with_webhook`] 配置（见该方法），且优先于本 `mode`。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum MilkyMode {
    /// WebSocket：连 `/event`（HTTP Upgrade），逐文本帧 decode。
    #[default]
    Ws,
    /// SSE：`GET /event` 不带 Upgrade，读取 `text/event-stream`。
    Sse,
}

/// Milky adapter 配置。
#[derive(Debug, Clone)]
pub struct MilkyConfig {
    /// 事件 WebSocket 基址，如 `ws://127.0.0.1:3000` 或带 prefix `ws://h:p/foo`。
    pub ws_url: String,
    /// 可选访问令牌（`/api` Bearer header；`/event` header + query；WebHook 用作 Bearer 校验）。
    pub access_token: Option<String>,
    /// 出站事件信道模式（默认 `Ws`，向后兼容）。`webhook` 配置时此项被忽略。
    pub mode: MilkyMode,
    /// WebHook 接收端（反向信道）`(bind, path)`：配置后**优先**走 WebHook 独立运行路径，
    /// 不再用 `mode` 的出站 ws/sse 连接（见 [`Self::with_webhook`]）。
    pub webhook: Option<(String, String)>,
}

impl MilkyConfig {
    /// 由事件 WS 基址构造配置（无 token，`mode` 默认 [`Ws`](MilkyMode::Ws)，无 webhook）。
    /// 链式 builder 起点：再按需接 [`with_token`](Self::with_token) /
    /// [`with_mode`](Self::with_mode) / [`with_webhook`](Self::with_webhook)。
    pub fn new(ws_url: impl Into<String>) -> Self {
        MilkyConfig {
            ws_url: ws_url.into(),
            access_token: None,
            mode: MilkyMode::Ws,
            webhook: None,
        }
    }

    /// 设置访问令牌（链式）。
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.access_token = Some(token.into());
        self
    }

    /// 设置出站事件信道模式（链式，`Ws`/`Sse`）。
    pub fn with_mode(mut self, mode: MilkyMode) -> Self {
        self.mode = mode;
        self
    }

    /// 改用 **WebHook 接收端**（反向信道）：nagisa 在 `bind` 起 HTTP 服务端、于 `path` 接收
    /// 协议端 POST 的事件（带 `Authorization: Bearer <access_token>` 校验，若配了 token）。
    /// 配置后即优先走 WebHook 独立运行路径，[`mode`](Self::mode) 的出站 ws/sse 连接被忽略
    /// （反向信道与出站信道互斥，故独立于 `MilkyMode` 枚举，不混在其中）。
    pub fn with_webhook(mut self, bind: impl Into<String>, path: impl Into<String>) -> Self {
        self.webhook = Some((bind.into(), path.into()));
        self
    }
}
