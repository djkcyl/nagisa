//! Handler 抽象：任意 `async fn(A0..An) -> HandlerResult`（每个 `Ai: FromContext`）
//! 由宏按 arity（参数个数）批量生成 blanket impl，擦除为 `Arc<dyn ErasedHandler>`。
use crate::ctx::Ctx;
use crate::extract::{FromContext, Reject};
use futures::future::BoxFuture;
use std::future::Future;
use std::sync::Arc;

/// handler 业务返回：`Ok(())` 处理成功，`Err(e)` 处理出错。
pub type HandlerResult = nagisa_types::error::Result<()>;

/// 把 handler 体的返回值规整成 [`HandlerResult`]，免去 handler 末尾的礼节性 `Ok(())`。
///
/// 为 `()` 与 [`HandlerResult`] 两种返回类型实现：返回 `()` 的 handler 自动视作成功
/// （`Ok(())`），返回 `Result<()>` 的 handler 原样传递其 `Err`。两个 impl 不重叠——
/// blanket `Handler` impl 约束在 `Fut::Output` 这一关联类型上，每个 `async fn` 的输出
/// 类型唯一，故无歧义。
///
/// 设计取舍：相较「为 `T: Into<HandlerResult>` 做泛型」，这里只覆盖恰好两种返回形态，
/// 更简单且不会把无关类型误当作合法返回。
pub trait IntoHandlerResult {
    fn into_handler_result(self) -> HandlerResult;
}

impl IntoHandlerResult for () {
    fn into_handler_result(self) -> HandlerResult {
        Ok(())
    }
}

impl IntoHandlerResult for HandlerResult {
    fn into_handler_result(self) -> HandlerResult {
        self
    }
}

/// dispatch 视角的单次 handler 执行结果。
pub enum HandlerOutcome {
    /// 命中并成功处理。
    Handled,
    /// 某提取器 `Skip` → 本 handler 不适用，dispatch 继续传播。
    Skipped,
    /// 提取出错或业务返回 `Err` → 记日志，dispatch 继续传播。
    Errored(nagisa_types::error::Error),
}

/// 类型擦除的 handler：dispatch 只面向它。
pub trait ErasedHandler: Send + Sync {
    fn call(&self, ctx: Arc<Ctx>) -> BoxFuture<'static, HandlerOutcome>;
}

/// 类型化 handler。由宏为多元 `async fn` 生成 blanket impl；用户一般不手写。
pub trait Handler<Args>: Clone + Send + Sync + 'static {
    fn call(&self, ctx: Arc<Ctx>) -> BoxFuture<'static, HandlerOutcome>;

    /// 擦除为 `Arc<dyn ErasedHandler>` 供注册。
    fn erased(self) -> Arc<dyn ErasedHandler>
    where
        Self: Sized,
        Args: 'static,
    {
        Arc::new(HandlerFn { f: self, _args: std::marker::PhantomData })
    }
}

/// `Handler<Args>` → `ErasedHandler` 的适配壳（携带 `Args` 幻型以选中正确的 impl）。
///
/// `PhantomData<fn() -> Args>` 自身恒为 `Send + Sync`，故壳的 auto `Send`/`Sync`
/// 只取决于 `F`——无需 `unsafe impl`（本 crate `#![forbid(unsafe_code)]`）。
struct HandlerFn<F, Args> {
    f: F,
    _args: std::marker::PhantomData<fn() -> Args>,
}

impl<F, Args> ErasedHandler for HandlerFn<F, Args>
where
    F: Handler<Args>,
    Args: 'static,
{
    fn call(&self, ctx: Arc<Ctx>) -> BoxFuture<'static, HandlerOutcome> {
        Handler::call(&self.f, ctx)
    }
}

/// 顺序提取一个参数：`Skip`→提前返回 `Skipped`，`Error`→提前返回 `Errored`。
///
/// dev 模式（`App::debug()` 注入 `DevMode`）下，`Skip` 不再静默：发一条 `[dev]` `WARN`
/// 点名是哪个提取器拒绝了本 handler，把「为何没触发」从黑盒变成可见诊断。
macro_rules! extract_or_bail {
    ($ty:ty, $ctx:expr) => {
        match <$ty as FromContext>::from_context(&$ctx).await {
            Ok(v) => v,
            Err(Reject::Skip) => {
                if $ctx.is_dev() {
                    tracing::warn!(
                        extractor = std::any::type_name::<$ty>(),
                        "[dev] skip: extractor rejected (Skip) — handler not applicable"
                    );
                }
                return HandlerOutcome::Skipped;
            }
            Err(Reject::Error(e)) => return HandlerOutcome::Errored(e),
        }
    };
}

/// 为 `F: Fn(A0..An) -> Fut` 生成 `Handler<(A0..An,)>` blanket impl。
macro_rules! impl_handler {
    ($($ty:ident),*) => {
        #[allow(non_snake_case, unused_variables)]
        impl<F, Fut, Out, $($ty,)*> Handler<($($ty,)*)> for F
        where
            F: Fn($($ty,)*) -> Fut + Clone + Send + Sync + 'static,
            Fut: Future<Output = Out> + Send + 'static,
            Out: IntoHandlerResult,
            $($ty: FromContext + Send + 'static,)*
        {
            fn call(&self, ctx: Arc<Ctx>) -> BoxFuture<'static, HandlerOutcome> {
                let f = self.clone();
                Box::pin(async move {
                    $(let $ty = extract_or_bail!($ty, *ctx);)*
                    // Handler 体可返回 `()`（自动包成 `Ok(())`）或 `HandlerResult`。
                    match f($($ty,)*).await.into_handler_result() {
                        Ok(()) => HandlerOutcome::Handled,
                        Err(e) => HandlerOutcome::Errored(e),
                    }
                })
            }
        }
    };
}

impl_handler!();
impl_handler!(A0);
impl_handler!(A0, A1);
impl_handler!(A0, A1, A2);
impl_handler!(A0, A1, A2, A3);
impl_handler!(A0, A1, A2, A3, A4);
impl_handler!(A0, A1, A2, A3, A4, A5);
impl_handler!(A0, A1, A2, A3, A4, A5, A6);
impl_handler!(A0, A1, A2, A3, A4, A5, A6, A7);
impl_handler!(A0, A1, A2, A3, A4, A5, A6, A7, A8);
impl_handler!(A0, A1, A2, A3, A4, A5, A6, A7, A8, A9);
impl_handler!(A0, A1, A2, A3, A4, A5, A6, A7, A8, A9, A10);
impl_handler!(A0, A1, A2, A3, A4, A5, A6, A7, A8, A9, A10, A11);
