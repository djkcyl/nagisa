//! dispatch 引擎：消费 `EventSource` 推来的事件流，逐个构造 `Ctx` 交给 `Router`，
//! 每个事件在独立的 `tokio::spawn` 任务中处理，实现真正的并发分发。
//! tokio 任务边界天然隔离 panic——单个 handler panic 不会拖垮分发循环。
//!
//! 任务被追踪在 `JoinSet` 中；主循环每轮顺便 reap 已完成的任务避免集合无界增长。
//! 收到 shutdown 信号或事件流关闭后，等待所有还在飞行中的 handler 任务完成后再返回。
use crate::bot::Bot;
use crate::ctx::Ctx;
use crate::router::Router;
use crate::ShutdownToken;
use nagisa_types::prelude::*;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tracing::Instrument;

/// 事件种类标签，用作 span / 日志字段。
fn kind_str(e: &Event) -> &'static str {
    match e {
        Event::Message(_) => "message",
        Event::Notice(_) => "notice",
        Event::Request(_) => "request",
        Event::Meta(_) => "meta",
        _ => "raw",
    }
}

/// 长生命周期分发任务：`select!`（关停 / 收事件 / reap 完成任务）。
/// `shutdown` 触发或事件流关闭即退出接收循环，之后等待所有 in-flight handler 完成。
///
/// 每个事件构造一个新 `Arc<Ctx>`（共享 `router.state()` 与 `bot`），在独立的
/// `tokio::spawn` 任务中并发处理；任务边界隔离 panic，慢 handler 不会阻塞循环。
pub async fn run_dispatch(
    router: Arc<Router>,
    bot: Bot,
    mut events: mpsc::Receiver<Event>,
    shutdown: ShutdownToken,
) {
    let state = router.state();
    let mut set: JoinSet<()> = JoinSet::new();

    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => {
                tracing::debug!("dispatch loop shutting down");
                break;
            }
            // 顺手 reap 已完成的 handler 任务,使集合有界——并把被任务边界隔离的 panic 暴露出来
            // (否则会被静默吞掉)。
            joined = set.join_next(), if !set.is_empty() => {
                if let Some(Err(e)) = joined {
                    tracing::error!(error = %e, "handler 任务 panic（已被任务边界隔离）");
                }
            }
            maybe = events.recv() => {
                let Some(event) = maybe else {
                    tracing::debug!("event channel closed; dispatch loop exiting");
                    break;
                };
                // 从每条入站消息学习 self_id,使 MentionMe/ToMe 始终正确——哪怕启动时 get_login_info
                // 失败了。
                if let Event::Message(m) = &event {
                    bot.set_self_id(m.self_id);
                }
                let ctx = Arc::new(Ctx::new(Arc::new(event), bot.clone(), Arc::clone(&state)));
                // 每事件一个 span：本事件分发期间框架发出的所有日志都挂它名下（带 kind/peer/sender），
                // 这样并发分发时同一事件的日志能串起来。span 仅 debug+ 生效，故 info 路径零成本。
                let ev = ctx.event();
                let span = tracing::debug_span!(
                    "event",
                    kind = kind_str(ev.as_ref()),
                    peer = ?ev.peer(),
                    sender = ?ev.sender(),
                );
                // 收到事件即记一条机制日志：心跳降到 trace 避免每 5s 刷屏；其余 debug。
                // 事件内容的可读渲染不在此处——那是 nagisa-log 的职责；nagisa-core 只留 kind 等机制字段。
                span.in_scope(|| match ev.as_ref() {
                    Event::Meta(Meta::Heartbeat { .. }) => tracing::trace!("heartbeat"),
                    _ => tracing::debug!("收到事件"),
                });
                // 每个事件在独立任务中处理：真正的并发，任务边界隔离 panic。
                let router = Arc::clone(&router);
                set.spawn(async move { router.dispatch(ctx).await }.instrument(span));
            }
        }
    }

    // 排空在飞 handler,免得关停时把还在跑的任务弃置。用循环 join_next 而非 shutdown(),
    // 让在飞任务跑完(shutdown() 会 abort 它们;任务内的 panic 已在任务边界被捕获)。
    while let Some(joined) = set.join_next().await {
        if let Err(e) = joined {
            tracing::error!(error = %e, "handler 任务 panic（关停 drain 期间）");
        }
    }
}
