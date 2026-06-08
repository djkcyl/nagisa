//! 事件源：把协议入站事件流解码成统一 `Event` 推给上层。
use crate::ShutdownToken;
use async_trait::async_trait;
use nagisa_types::prelude::*;
use std::sync::Arc;
use tokio::sync::mpsc;

/// 入站事件源。OneBot = WS 读半边；Milky = `/event`（WS/SSE/webhook）。
///
/// 实现需在内部完成：连接、带退避的重连、心跳/看门狗、把 wire 事件解码为统一
/// `Event`（未知事件降级为 `Event::Raw`，绝不丢弃），并推入 `sink`。
#[async_trait]
pub trait EventSource: Send + Sync + 'static {
    /// 长生命周期任务：仅在 `shutdown` 触发或永久失败时返回。
    async fn run(
        self: Arc<Self>,
        sink: mpsc::Sender<Event>,
        shutdown: ShutdownToken,
    ) -> Result<()>;
}
