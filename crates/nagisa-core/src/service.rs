//! 服务生命周期 Supervisor + 依赖 DAG。
//!
//! 一个 [`Service`] 有三段生命周期：
//! 1. [`prepare`](Service::prepare)：按依赖 DAG 的**拓扑序**逐层 await（依赖先就绪）；
//! 2. [`run`](Service::run)：全部 spawn 进一个 [`JoinSet`]，各持一个由根派生的子
//!    [`ShutdownToken`]；任一 `run` 返回 `Err`、或 shutdown 触发即收束；
//! 3. [`cleanup`](Service::cleanup)：触发 shutdown 后，按**逆拓扑序**清理。
//!
//! `prepare` 中途失败 → 中止，并对已 `prepare` 成功者逆序 cleanup。
//!
//! 服务之间通过 [`ServiceBus`] 共享句柄（按类型存取）：例如 OneBot adapter 在
//! `prepare`/`run` 里把它建好的 [`Bot`](crate::bot::Bot) `insert` 进 bus，dispatch
//! 服务再 `get` 出来消费。这是门面 builder 据以搭线的脊柱。
use crate::ShutdownToken;
use async_trait::async_trait;
use nagisa_types::error::{Error, Result};
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::task::JoinSet;

/// 服务间共享句柄袋：按类型（`TypeId`）存取 `Arc<T>`。廉价克隆（内部 `Arc`）。
///
/// 用于服务之间发布/查找共享句柄，典型场景：adapter 发布 `Bot`，dispatch 取用。
#[derive(Clone, Default)]
pub struct ServiceBus {
    inner: Arc<RwLock<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>>,
}

impl ServiceBus {
    /// 空 bus。
    pub fn new() -> Self {
        Self::default()
    }

    /// 发布一个 `T`（同类型覆盖）。以 `Arc<T>` 存储，`get::<T>()` 取回同一份。
    /// 锁中毒时取用 guard 内数据，而非 panic（保持无 panic 合约）。
    pub fn insert<T: Send + Sync + 'static>(&self, value: T) {
        let mut map = self.inner.write().unwrap_or_else(|e| e.into_inner());
        map.insert(TypeId::of::<T>(), Arc::new(value));
    }

    /// 取回先前 `insert::<T>` 的共享句柄；从未发布过则 `None`。
    /// 锁中毒时取用 guard 内数据，而非 panic（保持无 panic 合约）。
    pub fn get<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        let map = self.inner.read().unwrap_or_else(|e| e.into_inner());
        map.get(&TypeId::of::<T>()).and_then(|v| Arc::clone(v).downcast::<T>().ok())
    }
}

/// 受 Supervisor 管理的服务。三段式生命周期 + 依赖声明。
///
/// 默认方法允许只实现关心的阶段（如纯 `run` 的传输服务无需 `prepare`/`cleanup`）。
///
/// 因为该 trait 用 `#[async_trait]` 定义，自定义实现的 `impl` 块也必须标注它——
/// 否则会撞上晦涩的 `E0195`。框架从门面 re-export 了它，直接用 `#[nagisa::async_trait]`：
///
/// ```ignore
/// use nagisa::prelude::*;
///
/// struct MyService;
///
/// #[nagisa::async_trait]
/// impl Service for MyService {
///     fn id(&self) -> &'static str { "my-service" }
///     async fn run(self: std::sync::Arc<Self>, _bus: ServiceBus, shutdown: ShutdownToken) -> Result<()> {
///         shutdown.cancelled().await;
///         Ok(())
///     }
/// }
/// ```
#[async_trait]
pub trait Service: Send + Sync + 'static {
    /// 全局唯一 id；`deps()` 用它引用其他服务。
    fn id(&self) -> &'static str;

    /// 本服务依赖的其他服务 id（须在它们 `prepare` 之后才能 `prepare`）。
    fn deps(&self) -> &'static [&'static str] {
        &[]
    }

    /// 准备阶段：按依赖拓扑序调用。发布共享句柄到 `bus`、建连接、读配置等。
    async fn prepare(&self, bus: &ServiceBus) -> Result<()> {
        let _ = bus;
        Ok(())
    }

    /// 运行阶段：长生命周期任务，spawn 进 `JoinSet`。`select!` 监听 `shutdown`。
    /// 正常应在 `shutdown` 触发后返回 `Ok(())`；返回 `Err` 会触发整体收束。
    async fn run(self: Arc<Self>, bus: ServiceBus, shutdown: ShutdownToken) -> Result<()> {
        let _ = (bus, shutdown);
        Ok(())
    }

    /// 清理阶段：按逆拓扑序调用。关闭连接、刷盘、释放资源。
    async fn cleanup(&self, bus: &ServiceBus) -> Result<()> {
        let _ = bus;
        Ok(())
    }
}

/// 服务生命周期 Supervisor：注册一批服务，按依赖 DAG 编排 prepare→run→cleanup。
#[derive(Default)]
pub struct Supervisor {
    services: Vec<Arc<dyn Service>>,
    bus: ServiceBus,
}

impl Supervisor {
    /// 空 supervisor（自带一个空 [`ServiceBus`]）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一个服务。
    ///
    /// 名为 `add` 是既定的 builder API（非 `std::ops::Add`）。
    #[allow(clippy::should_implement_trait)]
    pub fn add(mut self, service: Arc<dyn Service>) -> Self {
        self.services.push(service);
        self
    }

    /// 共享 [`ServiceBus`] 的克隆（廉价）。供外部预置句柄或事后查询。
    pub fn bus(&self) -> ServiceBus {
        self.bus.clone()
    }

    /// 编排整套生命周期。`shutdown` 是根关停信号。
    ///
    /// 1. 按依赖拓扑序逐层 `prepare`（任一失败 → 对已就绪者逆序 cleanup 后返回 Err）；
    /// 2. 全部 `run` spawn 进 `JoinSet`，各持根 `shutdown` 的子 token；
    ///    await 至 shutdown 触发或某 `run` 返回 Err；
    /// 3. 触发 shutdown，按逆拓扑序 `cleanup`。
    ///
    /// 返回：若某 `run` 提前以 `Err` 收束则透传该错误，否则 `Ok(())`。
    pub async fn run(self, shutdown: ShutdownToken) -> Result<()> {
        let Supervisor { services, bus } = self;

        // —— 拓扑排序（Kahn）：得到 prepare/run 顺序的下标序列。——
        let order = toposort(&services)?;

        // —— 1. prepare：按拓扑序逐个 await；失败则逆序 cleanup 已就绪者。——
        let mut prepared: Vec<usize> = Vec::with_capacity(order.len());
        for &idx in &order {
            match services[idx].prepare(&bus).await {
                Ok(()) => prepared.push(idx),
                Err(e) => {
                    tracing::error!(service = services[idx].id(), error = %e, "service prepare failed; cleaning up");
                    cleanup_reverse(&services, &prepared, &bus).await;
                    return Err(e);
                }
            }
        }

        // —— 2. run：全部 spawn，各持子 shutdown token。——
        let mut set: JoinSet<Result<()>> = JoinSet::new();
        for &idx in &order {
            let svc = Arc::clone(&services[idx]);
            let bus = bus.clone();
            let child = shutdown.child_token();
            set.spawn(async move { svc.run(bus, child).await });
        }

        // await 至：shutdown 触发，或某 run 任务返回（Err 透传、Ok 视该服务自然退出）。
        let mut run_result: Result<()> = Ok(());
        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                joined = set.join_next() => {
                    match joined {
                        // 所有 run 任务都已退出。
                        None => break,
                        Some(Ok(Ok(()))) => continue, // 某服务自然结束；继续等其余。
                        Some(Ok(Err(e))) => {
                            tracing::error!(error = %e, "service run task returned error; shutting down");
                            run_result = Err(e);
                            break;
                        }
                        Some(Err(join_err)) => {
                            // run 任务 panic：隔离为错误，触发整体收束，不向上 panic。
                            tracing::error!(error = %join_err, "service run task panicked; shutting down");
                            run_result = Err(internal_err(format!(
                                "service run task panicked: {join_err}"
                            )));
                            break;
                        }
                    }
                }
            }
        }

        // —— 3. cleanup：触发 shutdown，等剩余 run 任务收束，再逆拓扑序 cleanup。——
        shutdown.cancel();
        set.shutdown().await; // 等所有 run 任务结束（已 abort/或正常退出）。
        cleanup_reverse(&services, &order, &bus).await;

        run_result
    }
}

/// 对 `indices`（拓扑序的子序列）逆序调用 `cleanup`；逐个记录错误但不中断，
/// 确保每个已就绪服务都有机会清理。
async fn cleanup_reverse(services: &[Arc<dyn Service>], indices: &[usize], bus: &ServiceBus) {
    for &idx in indices.iter().rev() {
        if let Err(e) = services[idx].cleanup(bus).await {
            tracing::error!(service = services[idx].id(), error = %e, "service cleanup failed");
        }
    }
}

/// Kahn 拓扑排序：返回服务下标的拓扑序（依赖在前）。
///
/// 错误（经 [`internal_err`]）：重复 id、依赖了不存在的 id、或存在环。
fn toposort(services: &[Arc<dyn Service>]) -> Result<Vec<usize>> {
    // id → 下标。重复 id 视为配置错误。
    let mut index: HashMap<&'static str, usize> = HashMap::with_capacity(services.len());
    for (i, svc) in services.iter().enumerate() {
        if index.insert(svc.id(), i).is_some() {
            return Err(internal_err(format!("duplicate service id `{}`", svc.id())));
        }
    }

    // 邻接：dep → dependents（edge dep→node），并统计每个 node 的入度（= deps 数）。
    let n = services.len();
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut indegree: Vec<usize> = vec![0usize; n];
    for (i, svc) in services.iter().enumerate() {
        for dep in svc.deps() {
            let Some(&d) = index.get(dep) else {
                return Err(internal_err(format!(
                    "service `{}` depends on unknown service `{dep}`",
                    svc.id()
                )));
            };
            dependents[d].push(i);
            indegree[i] += 1;
        }
    }

    // 入度为 0 的入队（按注册下标顺序，给出确定性输出）。
    let mut queue: std::collections::VecDeque<usize> =
        (0..n).filter(|&i| indegree[i] == 0).collect();
    let mut order: Vec<usize> = Vec::with_capacity(n);
    while let Some(node) = queue.pop_front() {
        order.push(node);
        for &m in &dependents[node] {
            indegree[m] -= 1;
            if indegree[m] == 0 {
                queue.push_back(m);
            }
        }
    }

    if order.len() != n {
        // 未能排完 → 剩余节点构成至少一个环。
        let cyclic: Vec<&str> =
            (0..n).filter(|&i| indegree[i] > 0).map(|i| services[i].id()).collect();
        return Err(internal_err(format!("dependency cycle among services: {cyclic:?}")));
    }

    Ok(order)
}

/// 把 Supervisor 内部失败（依赖配置错误 / 环 / run 任务 panic）包成统一 `Error`。
///
/// `nagisa-types` 当前无「框架内部错误」专用变体；归入 `Action{kind: Internal}`，
/// 经 [`Error::action`]（retcode 取哨兵 `NON_PROTOCOL_RETCODE` = 非协议、属框架编排层）。
fn internal_err(msg: String) -> Error {
    Error::action(msg)
}
