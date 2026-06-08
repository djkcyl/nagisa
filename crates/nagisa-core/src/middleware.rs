//! 洋葱式中间件：在「整条事件分发」外层包一圈，可在 handler 前后做事、或直接拦截。
//!
//! 洋葱续延模型(`next()`)：每个中间件做点事 → `await next.run(ctx)`
//! → 再做点事；返回 [`Flow::Stop`]（且不调用 `next`）即吞掉事件、终止传播。
//! 限流、审计、「插件禁用」门、交互式 prompt 拦截都挂在这一层。
use crate::ctx::Ctx;
use crate::router::Router;
use async_trait::async_trait;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// 中间件的传播控制。`Stop` = 吞掉本事件、不再下传。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Flow {
    Continue,
    Stop,
}

/// 一个洋葱层。`handle` 内 `next.run(ctx)` 把控制权交给内层（其余中间件 + handler 循环）。
#[async_trait]
pub trait Middleware: Send + Sync + 'static {
    async fn handle(&self, ctx: Arc<Ctx>, next: Next<'_>) -> Flow;
}

/// 指向「链条剩余部分」的续延：其余中间件 + 末端 handler 循环。
pub struct Next<'a> {
    pub(crate) remaining: &'a [Arc<dyn Middleware>],
    pub(crate) terminal: &'a Router,
}

impl<'a> Next<'a> {
    /// 把控制权交给内层。返回 boxed future 以打断异步递归的类型膨胀。
    pub fn run(self, ctx: Arc<Ctx>) -> Pin<Box<dyn Future<Output = Flow> + Send + 'a>> {
        Box::pin(async move {
            match self.remaining.split_first() {
                Some((mw, rest)) => {
                    let next = Next { remaining: rest, terminal: self.terminal };
                    mw.handle(ctx, next).await
                }
                None => {
                    // 末端：跑既有的优先级 handler 循环。
                    self.terminal.run_handlers(ctx).await;
                    Flow::Continue
                }
            }
        })
    }
}
