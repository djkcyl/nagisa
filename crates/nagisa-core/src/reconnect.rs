//! 带上限指数退避的**重连驱动** helper:把「连接断开 → 退避 → 重连」这套外层循环
//! 骨架抽出来,各 adapter 站点只提供「连一次」闭包与退避参数(起点 / 上限 / 可选 jitter)。
//!
//! 每个站点的现有语义都逐点保留:
//! - 退避起点 / 上限 / 倍率由 [`Backoff`] 各自配置(均为「sleep 当前值 → 再倍增封顶」)。
//! - **shutdown 优先**:每轮连接前、以及退避 sleep 期间都 `select!` 关停信号,立即干净返回。
//! - 「连一次」闭包返回 [`Step::Stop`] 即终止(干净退出);返回 [`Step::Reconnect`] 即退避重连。
//!   断开时的副作用(发 `Meta::Disconnect`、清 pending、按错误记日志、「干净结束不重置退避」)
//!   全部留在闭包内由站点自理——helper 只管循环骨架与退避节奏,不碰这些站点专属语义。
use crate::ShutdownToken;
use std::time::Duration;

/// 「连一次」闭包的结果:停止(干净退出)或退避后重连。
pub enum Step {
    /// 连接干净结束(通常因收到 shutdown)——终止重连循环。
    Stop,
    /// 连接断开/失败——退避后重连。断开副作用由闭包自理(见模块级文档)。
    Reconnect,
}

/// 指数退避状态:`sleep` 取当前值(可叠加 jitter),随后 `*2` 封顶到 `max`。
#[derive(Debug, Clone)]
pub struct Backoff {
    current: Duration,
    max: Duration,
}

impl Backoff {
    /// 由起点 / 上限构造。倍率固定为 2(沿用各站点现状)。
    pub fn new(initial: Duration, max: Duration) -> Self {
        Self { current: initial, max }
    }

    /// 当前退避基值(未叠加 jitter)。
    pub fn current(&self) -> Duration {
        self.current
    }

    /// 倍增并封顶到 `max`(下次 `current()` 生效)。
    pub fn advance(&mut self) {
        self.current = (self.current * 2).min(self.max);
    }
}

/// 重连驱动:循环「连一次 → 按结果停/退避重连」,shutdown 优先。
///
/// `connect_once` 每轮被调用一次执行一次完整的连接 + pump;它**自己**完成断开时的全部副作用
/// (发 `Meta::Disconnect`、清 pending、记日志等),只把控制流意图作为 [`Step`] 返回。
/// `jitter` 给定当前退避基值的毫秒数、返回要叠加的毫秒抖动(无抖动站点传 `|_| 0`)。
///
/// 仅在收到 shutdown(或 `connect_once` 返回 [`Step::Stop`])时返回 `Ok(())`。
pub async fn run<C, Fut, J>(
    shutdown: &ShutdownToken,
    mut backoff: Backoff,
    mut jitter: J,
    mut connect_once: C,
) -> nagisa_types::error::Result<()>
where
    C: FnMut() -> Fut,
    Fut: std::future::Future<Output = Step>,
    J: FnMut(u64) -> u64,
{
    loop {
        if shutdown.is_cancelled() {
            return Ok(());
        }
        match connect_once().await {
            Step::Stop => return Ok(()),
            Step::Reconnect => {
                if shutdown.is_cancelled() {
                    return Ok(());
                }
                // sleep 当前退避基值 + 站点 jitter;sleep 期间 shutdown 优先。
                let base = backoff.current();
                let extra = jitter(base.as_millis() as u64);
                let sleep = base + Duration::from_millis(extra);
                tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => return Ok(()),
                    _ = tokio::time::sleep(sleep) => {}
                }
                // 与各站点现状一致:统一只倍增、封顶(「干净结束不重置退避」由闭包侧的
                // 返回值语义保证——不论 Ok(false) 还是 Err 都走同一退避,helper 从不重置)。
                backoff.advance();
            }
        }
    }
}
