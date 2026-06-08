//! 统一错误层：业务只见 [`Error`](enum@Error) / [`Result`]，看不到 retcode / HTTP status /
//! 协议结构。配套 [`bail!`](crate::bail) 宏用于 handler 提前失败；[`context`](crate::context)
//! 模块提供把外部错误归一到这里的 `.context()`。
use thiserror::Error;

/// 传输层错误。适配器把 tungstenite/reqwest 等错误映射成这里的字符串变体；本 crate 不依赖它们。
#[derive(Error, Debug)]
pub enum TransportError {
    #[error("websocket error: {0}")]
    WebSocket(String),
    #[error("http error: {0}")]
    Http(String),
    #[error("connection closed")]
    Closed,
}

/// 动作错误的粗分类（启发式，绝不按精确 retcode 值匹配——三实现 retcode 不一致）。
#[non_exhaustive]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ActionErrorKind {
    Unsupported,
    BadParams,
    AuthFailed,
    RateLimited,
    Internal,
    /// 资源未找到（如 Milky 的 "message not found"）。
    NotFound,
    Other,
}

/// Nagisa 统一错误。
#[derive(Error, Debug)]
pub enum Error {
    /// 业务动作失败：retcode != 0 / status != "ok"。retcode 不透明，分类看 `kind`。
    #[error("action failed: retcode={retcode} {message}")]
    Action { retcode: i64, message: String, kind: ActionErrorKind },
    /// 该实现不支持此动作。
    #[error("action `{0}` not supported by this implementation")]
    Unsupported(String),
    #[error("action timed out")]
    Timeout,
    #[error("connection closed mid-call")]
    ConnectionClosed,
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// 仅动作响应才硬失败；入站事件解码失败应降级为 Event::Raw，不产生此错误。
    #[error(transparent)]
    Decode(#[from] serde_json::Error),
}

/// 库内统一 Result。
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// 哨兵 retcode：表示「非协议失败」——业务 handler / 框架编排层主动构造的错误。
    ///
    /// 协议适配器永远用真实 retcode；只有不源自某个具体协议响应的错误才取此值。
    pub const NON_PROTOCOL_RETCODE: i64 = -1;

    /// 友好构造器：业务 handler 主动失败的常用入口。
    ///
    /// retcode 取哨兵 [`Error::NON_PROTOCOL_RETCODE`]、kind 取
    /// [`ActionErrorKind::Internal`]——这正是「不是某个协议响应、而是业务逻辑/框架编排
    /// 自己判定失败」的语义。需要别的 kind 时用 [`Error::action_kind`]，需要透传真实
    /// retcode（仅适配器）时仍可直接构造 `Error::Action {..}` 结构体变体。
    ///
    /// 设计取舍：只暴露 `action`(默认) + `action_kind`(可定制 kind) 两个构造器，而非为每个
    /// 字段做 builder——handler 绝大多数只想喊一句「我失败了，原因是 msg」。
    pub fn action(message: impl Into<String>) -> Self {
        Error::action_kind(ActionErrorKind::Internal, message)
    }

    /// 同 [`Error::action`]，但显式指定粗分类 [`ActionErrorKind`]（如 `BadParams`）。
    pub fn action_kind(kind: ActionErrorKind, message: impl Into<String>) -> Self {
        Error::Action { retcode: Self::NON_PROTOCOL_RETCODE, message: message.into(), kind }
    }

    /// 是否为「该实现不支持此动作」。业务可据此降级（配合 `Bot::supports`）。
    pub fn is_unsupported(&self) -> bool {
        matches!(self, Error::Unsupported(_))
            || matches!(self, Error::Action { kind: ActionErrorKind::Unsupported, .. })
    }
}

/// 在 handler（或任何返回 `nagisa` `Result` 的函数）里提前失败：`bail!("...")` 即
/// `return Err(Error::action(format!("...")))`。
///
/// 形态对齐 `anyhow::bail!`：
/// - `bail!("plain message")` / `bail!("{x} too big", x = n)`——格式化串，kind 默认 `Internal`；
/// - `bail!(kind, "...")`——首参是 [`ActionErrorKind`]，其余照常格式化，便于喊出
///   `BadParams` 之类的分类。
///
/// 设计取舍：做成 `#[macro_export]` 的声明宏（而非 proc-macro），零额外编译开销，且
/// `$crate` 让它在任何消费 crate 内都正确解析到 `nagisa_types`（经 `nagisa` 预导出后即
/// `nagisa::bail!`）。
#[macro_export]
macro_rules! bail {
    ($kind:expr, $fmt:literal $(, $($arg:tt)*)?) => {
        return ::core::result::Result::Err(
            $crate::error::Error::action_kind($kind, ::std::format!($fmt $(, $($arg)*)?))
        )
    };
    ($fmt:literal $(, $($arg:tt)*)?) => {
        return ::core::result::Result::Err(
            $crate::error::Error::action(::std::format!($fmt $(, $($arg)*)?))
        )
    };
}
