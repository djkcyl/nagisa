//! 错误上下文适配：把任意「`Display` 错误的 `Result`」或「`Option`」归一到 nagisa 统一
//! [`Result`]，并附一句人读的上下文。形态对齐 `anyhow::Context`，但不依赖 anyhow——失败时
//! 落到 [`Error::action`]（哨兵 retcode + `Internal` 分类），与 [`bail!`](crate::bail) 同口径。
//!
//! 为什么是扩展 trait 而非 `impl<E: Display> From<E> for Error`：后者与既有
//! `#[from] serde_json::Error`（见 [`error`](crate::error)）以及 `Error: Display` 自身
//! （Self-from-Self）相干性冲突，编译不过。方法式适配是唯一相干安全的形态——这也是为什么裸
//! `?` 不足、业务侧得显式标注上下文的原因。
//!
//! ```text
//! use nagisa::prelude::*;
//! let rows = q.all(db).await.context("查回放历史")?;          // Result<_, sea_orm::DbErr>
//! let model = by_uin.remove(&u).context("用户取或建后仍缺失")?; // Option<_>
//! // 惰性：仅在出错时才格式化,省掉热路径上的 format 预分配
//! let m = map.get(&k).with_context(|| format!("缺少键 {k}"))?;
//! ```
use core::fmt::Display;

use crate::error::{Error, Result};

/// 给 `Result<T, E: Display>` 与 `Option<T>` 附上下文并归一到 nagisa [`Result`]。
///
/// 经 `nagisa::prelude::*` 导出后，业务一行 `.context("做某事")?` 即把外部错误
/// （sea-orm / reqwest / image / `JoinError` …）转成统一 [`Error`]，无需各自
/// 手写 `.map_err(|e| Error::action(format!("做某事: {e}")))`。
pub trait Context<T> {
    /// 附一句静态/已构造好的上下文。`Result` 失败时拼成 `"{ctx}: {错误}"`；
    /// `Option` 为 `None` 时即以 `ctx` 为消息。
    fn context(self, ctx: impl Display) -> Result<T>;

    /// 同 [`context`](Self::context)，但上下文**惰性**生成——闭包仅在出错路径执行，
    /// 适合上下文本身要 `format!` 的场景（成功路径零分配）。
    fn with_context<F, D>(self, f: F) -> Result<T>
    where
        F: FnOnce() -> D,
        D: Display;
}

impl<T, E: Display> Context<T> for core::result::Result<T, E> {
    fn context(self, ctx: impl Display) -> Result<T> {
        self.map_err(|e| Error::action(format!("{ctx}: {e}")))
    }

    fn with_context<F, D>(self, f: F) -> Result<T>
    where
        F: FnOnce() -> D,
        D: Display,
    {
        self.map_err(|e| Error::action(format!("{}: {e}", f())))
    }
}

impl<T> Context<T> for Option<T> {
    fn context(self, ctx: impl Display) -> Result<T> {
        self.ok_or_else(|| Error::action(ctx.to_string()))
    }

    fn with_context<F, D>(self, f: F) -> Result<T>
    where
        F: FnOnce() -> D,
        D: Display,
    {
        self.ok_or_else(|| Error::action(f().to_string()))
    }
}
