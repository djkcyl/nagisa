//! Router：handler 注册表（带优先级）+ app 共享状态表 + 洋葱中间件，按三层顺序分发单个事件。
//!
//! 注册口分两类：朴素观察者（[`on`](Router::on)/[`on_priority`](Router::on_priority)/
//! [`on_top`](Router::on_top)，每事件都跑、靠提取器 `Skip` 过滤）与命令 / 事件触发器
//! （[`command`](Router::command)/[`trigger_command`](Router::trigger_command)/
//! [`event_named`](Router::event_named)，带匹配器、启用门、可选门控 `Rule`）。`#[command]`/
//! `#[event]` 宏经 `trigger_command`/`event_named` 挂载，二者均以 `gate: Option<Rule>` 形参
//! 承载 `gate=`/`cooldown=`（无则传 `None`，与未门控零差异）。
//!
//! 分发（[`dispatch`](Router::dispatch)）先穿过中间件链，末端跑三层 handler 循环：
//! top observer → waiter 检查 → default handler。
//! 同层 handler 互不阻断——「消费即停」的传播阻断从未对外暴露、已作死设计移除；
//! 仅 waiter 命中（tier 2）且其 `block` 为真时才吞掉事件、跳过 tier 3。
use crate::ctx::{Ctx, StateMap};
use crate::handler::{ErasedHandler, Handler, HandlerOutcome};
use crate::matcher::Matcher;
use crate::middleware::{Middleware, Next};
use crate::rule::Rule;
use std::any::TypeId;
use std::sync::Arc;

/// 单个 handler 的启用控制元数据（插件总开关 + 触发器子开关）。
#[derive(Clone, Copy)]
struct TriggerControl {
    plugin_key: &'static str,
    trigger_key: &'static str,
    plugin_default: bool,
    plugin_can_disable: bool,
    trigger_default: bool,
    trigger_can_disable: bool,
}

/// 一条注册项：擦除后的 handler + 优先级 + 可选命令匹配器 + 可选门控规则。
/// 同层各 handler 互不阻断:事件按优先级依次喂给每个命中的 handler(「消费即停」的
/// 传播阻断从未被任何 API 暴露,已作为死设计移除;waiter 的 block(tier 2)是另一回事,仍在)。
struct Registered {
    handler: Arc<dyn ErasedHandler>,
    priority: i32,
    /// 命令型 handler 的门控匹配器；`None` 表示普通 handler（每事件都跑、靠提取器过滤）。
    matcher: Option<Matcher>,
    /// 可选门控规则（`Rule`，权限/场景等）；matcher 命中后、handler 前求值，不过则跳过。
    gate: Option<Rule>,
    /// 命令启用控制元数据（`#[command]` 经 `trigger_command` 携带）；`None` = 不参与 enable/disable。
    cmd: Option<TriggerControl>,
    /// 事件种类预筛（`#[event]` handler）：`Some(k)` 时本 handler 仅在当前事件的
    /// `EventKind` 等于 `k` 时触发。在启用门控之前检查。
    event_kind: Option<crate::event_trigger::EventKind>,
    /// 第一层「top observer」：在 waiter 检查之前跑，永不被 waiter 拦。
    top: bool,
}

/// handler 注册表 + app 共享状态 + 洋葱中间件。`dispatch` 先穿过中间件，
/// 末端按优先级（小者先）依次执行 handler。
/// handlers 始终保持按优先级稳定排序（注册时排序，分发时直接遍历）。
pub struct Router {
    handlers: Vec<Registered>,
    state: Arc<StateMap>,
    middleware: Vec<Arc<dyn Middleware>>,
}

impl Default for Router {
    fn default() -> Self {
        Self { handlers: Vec::new(), state: Arc::new(StateMap::new()), middleware: Vec::new() }
    }
}

impl Router {
    /// 空 router。
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一个 handler（默认优先级 0）。
    pub fn on<H, Args>(self, h: H) -> Self
    where
        H: Handler<Args>,
        Args: 'static,
    {
        self.register(h, 0, None, None)
    }

    /// 以指定优先级注册。
    pub fn on_priority<H, Args>(self, priority: i32, h: H) -> Self
    where
        H: Handler<Args>,
        Args: 'static,
    {
        self.register(h, priority, None, None)
    }

    /// 注册一个第一层 top observer：在 waiter 检查之前跑，不会被任何 waiter 拦。
    /// 适合那些必须看到每个事件的横切观察者（如发言计数器）——哪怕此刻某游戏的 waiter 正在
    /// 吞消息也照看不误。
    pub fn on_top<H, Args>(self, h: H) -> Self
    where
        H: Handler<Args>,
        Args: 'static,
    {
        self.register_full(h, 0, None, None, None, None, true)
    }

    /// 注册一个由 `matcher` 门控的命令型 handler（默认优先级 0）。
    /// dispatch 时先跑 `matcher.match_event(ctx)`：未命中则跳过本 handler；
    /// 命中则把 `ParsedCommand` 存入 `ctx` 扩展，再运行 handler（其内可用
    /// `Command`/`CommandArg`/`ArgText`/`Captures` 提取）。
    pub fn command<H, Args>(self, matcher: Matcher, h: H) -> Self
    where
        H: Handler<Args>,
        Args: 'static,
    {
        self.register(h, 0, Some(matcher), None)
    }

    /// 注册一个带门控 `Rule`（权限/场景等）的普通 handler。规则不过则跳过本 handler。
    pub fn on_with<H, Args>(self, gate: Rule, h: H) -> Self
    where
        H: Handler<Args>,
        Args: 'static,
    {
        self.register(h, 0, None, Some(gate))
    }

    /// 注册一个由 `matcher` 门控、且附带 `Rule`（权限等）的命令型 handler。
    /// 顺序：matcher 命中 → `Rule` 求值（可读取 `ParsedCommand`/`to_me`）→ handler。
    pub fn command_with<H, Args>(self, matcher: Matcher, gate: Rule, h: H) -> Self
    where
        H: Handler<Args>,
        Args: 'static,
    {
        self.register(h, 0, Some(matcher), Some(gate))
    }

    /// 注册一个由 matcher 门控、并携带插件 + 触发器开关元数据的命令型 handler。
    /// `#[command]` 宏经它挂载，故分层的 EnabledSet 门控得以套用。
    ///
    /// `gate: Option<Rule>` 承载 `#[command(gate=…/cooldown=…)]` 的可选门控
    /// （matcher 命中后求值，不过则跳过）；无 gate/cooldown 时传 `None`，与未门控命令零差异。
    #[allow(clippy::too_many_arguments)]
    pub fn trigger_command<H, Args>(
        self,
        plugin_key: &'static str,
        trigger_key: &'static str,
        plugin_default: bool,
        plugin_can_disable: bool,
        trigger_default: bool,
        trigger_can_disable: bool,
        top: bool,
        priority: i32,
        matcher: Matcher,
        gate: Option<Rule>,
        h: H,
    ) -> Self
    where
        H: Handler<Args>,
        Args: 'static,
    {
        let cmd = TriggerControl {
            plugin_key,
            trigger_key,
            plugin_default,
            plugin_can_disable,
            trigger_default,
            trigger_can_disable,
        };
        self.register_full(h, priority, Some(matcher), gate, Some(cmd), None, top)
    }

    /// 注册一个事件触发型 handler：无 matcher，由 `EventKind` + 分层开关（`TriggerControl`）门控。
    /// 由 `#[event]` 挂载。
    ///
    /// `gate: Option<Rule>` 承载 `#[event(Kind, gate=…/cooldown=…)]` 的可选
    /// 门控——故 `gate=`/`cooldown=` 在非消息 `#[event(Kind)]` 上语义与 `#[command]` 完全一致
    /// （共享 MetaArgs 解析）；无 gate/cooldown 时传 `None`。
    #[allow(clippy::too_many_arguments)]
    pub fn event_named<H, Args>(
        self,
        plugin_key: &'static str,
        trigger_key: &'static str,
        plugin_default: bool,
        plugin_can_disable: bool,
        trigger_default: bool,
        trigger_can_disable: bool,
        top: bool,
        priority: i32,
        kind: crate::event_trigger::EventKind,
        gate: Option<Rule>,
        h: H,
    ) -> Self
    where
        H: Handler<Args>,
        Args: 'static,
    {
        let cmd = TriggerControl {
            plugin_key,
            trigger_key,
            plugin_default,
            plugin_can_disable,
            trigger_default,
            trigger_can_disable,
        };
        self.register_full(h, priority, None, gate, Some(cmd), Some(kind), top)
    }

    fn register<H, Args>(self, h: H, priority: i32, matcher: Option<Matcher>, gate: Option<Rule>) -> Self
    where
        H: Handler<Args>,
        Args: 'static,
    {
        self.register_full(h, priority, matcher, gate, None, None, false)
    }

    #[allow(clippy::too_many_arguments)]
    fn register_full<H, Args>(
        mut self,
        h: H,
        priority: i32,
        matcher: Option<Matcher>,
        gate: Option<Rule>,
        cmd: Option<TriggerControl>,
        event_kind: Option<crate::event_trigger::EventKind>,
        top: bool,
    ) -> Self
    where
        H: Handler<Args>,
        Args: 'static,
    {
        self.handlers.push(Registered { handler: h.erased(), priority, matcher, gate, cmd, event_kind, top });
        // 注册时稳定排序（同优先级保持注册顺序），分发时直接遍历。
        self.handlers.sort_by_key(|r| r.priority);
        self
    }

    /// 注册一份 app 共享状态：以 `Arc<T>` 存入，`State<T>` 提取器可取。
    /// builder 阶段调用（尚未共享 Arc），直接获取唯一可变引用写入。
    pub fn data<T: Send + Sync + 'static>(mut self, value: T) -> Self {
        Arc::get_mut(&mut self.state)
            .expect("state Arc is shared during build — Router::data must be called before cloning the Router")
            .insert(TypeId::of::<T>(), Arc::new(value));
        self
    }

    /// 把一个已有的 `Arc<T>` 作为共享状态插入。当调用方需要保留对同一实例的句柄时有用
    /// （如 `App` 自己留着一份 `Arc<EnabledSet>`）。须在 `Router` 被克隆之前调用（即 builder 阶段）。
    pub fn data_arc<T: Send + Sync + 'static>(mut self, value: Arc<T>) -> Self {
        Arc::get_mut(&mut self.state)
            .expect("state Arc is shared during build — Router::data_arc must be called before cloning")
            .insert(TypeId::of::<T>(), value);
        self
    }

    /// 共享状态表的 `Arc`（dispatch 引擎用它构造每事件 `Ctx`）。
    /// 廉价克隆：只递增引用计数，无 HashMap 拷贝。
    pub fn state(&self) -> Arc<StateMap> {
        Arc::clone(&self.state)
    }

    /// 追加一层洋葱中间件：包裹整条事件分发。先注册的在更外层。
    pub fn layer<M: Middleware>(mut self, m: M) -> Self {
        self.middleware.push(Arc::new(m));
        self
    }

    /// 分发单个事件：先穿过中间件链，末端跑 handler 循环。
    /// 无中间件时走零开销直达分支。
    pub async fn dispatch(&self, ctx: Arc<Ctx>) {
        if self.middleware.is_empty() {
            self.run_handlers(ctx).await;
        } else {
            let next = Next { remaining: &self.middleware, terminal: self };
            let _ = next.run(ctx).await;
        }
    }

    /// 第一层（top）与第三层（default）共用的单 handler 主体。同层 handler 互不阻断，
    /// 调用方按优先级逐个调用本函数即可（不依赖返回值短路）。
    ///
    /// 语义与从前那个单一 `run_handlers` 循环逐字一致：
    /// 事件种类预筛 → 启用门 → matcher（`ParsedCommand` 仅本 handler 可见）→ `Rule` 门
    /// → handler 调用 → `ParsedCommand` 清理 → 结果。
    async fn try_run_one(&self, reg: &Registered, ctx: &Arc<Ctx>) {
        // 事件种类预筛:若本 handler 绑定到某个具体 kind,在任何启用门/matcher 求值之前就廉价跳过。
        if let Some(kind) = reg.event_kind {
            if crate::event_trigger::EventKind::of(ctx.event().as_ref()) != Some(kind) {
                // dev 模式：点名为何跳过（事件种类不符），把「静默跳过」变成可见诊断。
                if ctx.is_dev() {
                    tracing::warn!(
                        expected = ?kind,
                        actual = ?crate::event_trigger::EventKind::of(ctx.event().as_ref()),
                        trigger = reg.cmd.map(|c| c.trigger_key).unwrap_or("?"),
                        "[dev] skip: event-kind mismatch"
                    );
                }
                return;
            }
        }
        // 启用/禁用门控（在匹配前廉价短路）。
        if let Some(cmd) = &reg.cmd {
            if !command_enabled(ctx, cmd) {
                // dev 模式：点名是哪条命令/插件的开关把它关掉了。
                if ctx.is_dev() {
                    tracing::warn!(
                        plugin = cmd.plugin_key,
                        trigger = cmd.trigger_key,
                        "[dev] skip: switch off (plugin/trigger disabled for this peer)"
                    );
                }
                return;
            }
        }
        // 命令型 handler：先跑匹配器。未命中 → 跳过；命中 → 存 ParsedCommand 再运行。
        // 关键：matcher-gated handler 的 ParsedCommand 仅在本 handler 作用域内可见——
        // 运行后立即 remove，避免泄漏给同事件后续的 handler。
        let is_command = reg.matcher.is_some();
        if let Some(matcher) = &reg.matcher {
            match matcher.match_event(ctx) {
                Some(parsed) => ctx.insert_ext(parsed),
                None => {
                    // dev 模式：匹配器没中（命令头不匹配本条消息）。
                    if ctx.is_dev() {
                        tracing::warn!(
                            trigger = reg.cmd.map(|c| c.trigger_key).unwrap_or("?"),
                            "[dev] skip: matcher miss (command head did not match this message)"
                        );
                    }
                    return;
                }
            }
        }
        // 门控规则：matcher 命中后求值（规则可读 ParsedCommand/to_me）；不过 → 跳过，
        // 并清掉本轮可能插入的 ParsedCommand，避免泄漏给后续 handler。
        if let Some(gate) = &reg.gate {
            let passed = gate.eval(ctx).await;
            // 无论通过与否,都把本轮 eval 可能落下的 `GateReply` 取走清空。`GateReply` 是整个
            // 事件共享的单槽 ctx-ext:若只在否决分支清(原实现),那么 `replying(a,msg) | b` 这种
            // 组合里 a 否决(写 GateReply)、b 通过 → 整体通过、不进否决分支 → 这条陈旧 GateReply
            // 残留,被同事件后续某个静默否决的 handler 误读、把上一条命令的回复发给错的人。
            let gate_reply = ctx.remove_ext::<crate::rule::GateReply>();
            if !passed {
                // reply-on-veto：若某 `replying` 叶子在否决时记下了一条
                // `GateReply`（首写者胜），在此 inline 发送一次(保持「dispatch 返回即已发」的
                // 确定性顺序;gate-veto 自动回复本就少见,串行化代价可忽略)。纯布尔门控从不落
                // `GateReply`,故此分支零开销静默跳过。
                if let Some(crate::rule::GateReply(m)) = gate_reply {
                    if let Some(msg) = ctx.message() {
                        let _ = ctx.bot().send(&msg.peer, &m).await;
                    }
                }
                // dev 模式：门控规则（权限/场景）拒绝了本次触发。
                if ctx.is_dev() {
                    tracing::warn!(
                        trigger = reg.cmd.map(|c| c.trigger_key).unwrap_or("?"),
                        "[dev] skip: gate rejected (permission/scene rule returned false)"
                    );
                }
                if is_command {
                    let _ = ctx.remove_ext::<crate::matcher::ParsedCommand>();
                    let _ = ctx.remove_ext::<crate::matcher::CommandUsage>();
                }
                return;
            }
        }
        // 命中匹配器并通过门控，即将运行：命令型 / 事件型 handler 记一条 debug（"谁触发了什么"，
        // 带 sender/peer，便于排障时看清谁触发了哪个命令）。这是逐次调用的细节,归 debug——
        // info 级保留给更高信噪比的事件,免得每条命令都刷屏；普通观察者（on/on_top）同样不记。
        let invoked = reg.cmd.is_some() || reg.event_kind.is_some();
        if invoked {
            tracing::debug!(
                trigger = reg.cmd.map(|c| c.trigger_key).unwrap_or("?"),
                plugin = reg.cmd.map(|c| c.plugin_key).unwrap_or("?"),
                sender = ?ctx.event().sender(),
                peer = ?ctx.event().peer(),
                "触发 handler"
            );
        }
        let started = std::time::Instant::now();
        let outcome = reg.handler.call(Arc::clone(ctx)).await;
        let elapsed_ms = started.elapsed().as_millis() as u64;
        if is_command {
            // 限定 ParsedCommand 的可见性：仅本命令 handler 能读到自己的解析。
            let _ = ctx.remove_ext::<crate::matcher::ParsedCommand>();
            let _ = ctx.remove_ext::<crate::matcher::CommandUsage>();
        }
        match outcome {
            HandlerOutcome::Skipped => {}
            HandlerOutcome::Errored(e) => {
                tracing::warn!(
                    trigger = reg.cmd.map(|c| c.trigger_key).unwrap_or("?"),
                    error = %e,
                    priority = reg.priority,
                    elapsed_ms,
                    "handler 出错"
                );
            }
            HandlerOutcome::Handled => {
                if invoked {
                    tracing::debug!(elapsed_ms, "handler 完成");
                }
            }
        }
    }

    /// 三层分发：
    ///   1. top observer（`reg.top == true`），按优先级——总是跑。
    ///   2. waiter 检查：查 `ctx.state()` 里的 `WaiterStore` 并 `try_deliver`；
    ///      若投递成功**且**该 waiter 的 `block` 为真 → 直接返回（跳过第三层）。
    ///   3. default handler（`reg.top == false`），按优先级。
    pub(crate) async fn run_handlers(&self, ctx: Arc<Ctx>) {
        // 第一层:top observer。
        for reg in self.handlers.iter().filter(|r| r.top) {
            self.try_run_one(reg, &ctx).await;
        }
        // 第二层:waiter 检查。
        if let Some(store) = ctx
            .state()
            .get(&TypeId::of::<crate::session::WaiterStore>())
            .and_then(|a| a.downcast_ref::<crate::session::WaiterStore>())
        {
            let d = store.try_deliver(ctx.event());
            if d.delivered {
                tracing::debug!(blocked = d.block, "事件投递给中断 waiter");
            }
            if d.delivered && d.block {
                return;
            }
        }
        // 第三层:default handler。
        for reg in self.handlers.iter().filter(|r| !r.top) {
            self.try_run_one(reg, &ctx).await;
        }
    }
}

fn command_enabled(ctx: &Ctx, cmd: &TriggerControl) -> bool {
    let peer = ctx.event().peer();
    match ctx
        .state()
        .get(&TypeId::of::<crate::enabled::EnabledSet>())
        .and_then(|a| a.downcast_ref::<crate::enabled::EnabledSet>())
    {
        Some(es) => es.is_enabled_keyed(
            cmd.plugin_key,
            cmd.trigger_key,
            cmd.plugin_default,
            cmd.plugin_can_disable,
            cmd.trigger_default,
            cmd.trigger_can_disable,
            peer,
        ),
        None => {
            // 无 EnabledSet:退回「默认开,除非某个 default 为 false」。
            if !cmd.trigger_can_disable {
                return true;
            }
            cmd.plugin_default && cmd.trigger_default
        }
    }
}
