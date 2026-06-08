//! [`App`] 构建器 —— 面向使用者地组装一个 router 并把它跑在某个协议适配器之上。
//!
//! [`App::new`] 从 [`collect_into`] 起步,凡是经 `inventory` 可达的 `#[command]` /
//! `#[event]` handler 都会自动登记。构建器方法([`App::command`] / [`App::on`] /
//! [`App::data`] / [`App::service`])要么把显式登记委托给底层 [`Router`],要么累加额
//! 外的 [`Service`]。随后 `run_*` 方法把适配器的 `Arc` 接到分发循环 —— 并让所有用户服务
//! 跑在同一个 `JoinSet` 里 —— 直到 `shutdown` 触发或传输致命失败。
//!
//! ## `self_id` 怎么解析出来
//!
//! [`Bot`] 一开始就需要 bot 自己的 QQ 号:它会被克隆进每个 per-event
//! [`Ctx`](crate::Ctx),而 `MentionMe` / `ToMe` 拿首段的 mention 和 `bot.self_id()`
//! 比对。从首个事件里惰性推出它,就意味着要改一个已被共享的 `Bot`,而 `Bot` 没这个
//! API。所以我们**急切**解析:先 spawn `EventSource::run` 任务,再调
//! `adapter.get_login_info()`,短间隔重试几次直到 socket 起来。若在重试预算内始终拿不
//! 到登录信息,就退回 `Uin(0)` 并打一条 warning,而不是拒绝启动(非 mention 的 handler
//! 照常分发;mention 匹配只是要等将来某次重连解析出真的 `self_id` 才会生效)。
//!
//! ## Supervisor vs. 直接用 `JoinSet`
//!
//! 完整的 Supervisor 桥接(把适配器 + 分发都做成 `Service` 塞进 `Supervisor`)评估过但
//! 这里没用:`Bot` 必须在事件源启动**之后、**分发启动**之前**这个窗口里(经
//! `get_login_info`)构建出来,而 Supervisor 的契约是「先 `prepare` 每个服务,再 `run`
//! 每个服务」—— `get_login_info` 需要*正在运行*的事件源,而它只存在于 `run` 阶段。把这
//! 个握手穿过 `ServiceBus` 走会更多代码、更脆,不如下面这个双任务 `select!`。
//!
//! 这里用的方案保留了基于 `JoinSet` 的运行循环来跑适配器 + 分发,并加上 [`App::service`],
//! 让额外的用户 [`Service`](数据库连接、渲染循环等)跑在**同一个** `JoinSet` 里。它们的
//! `prepare`/`cleanup` 阶段也会按拓扑序在 run 阶段前后被执行 —— 所以这是个真正可组合的扩
//! 展点。
//!
//! **局限**:这些额外服务彼此共享一个 [`ServiceBus`](crate::ServiceBus),但不能直接
//! 发布/消费 `Bot` 句柄(`Bot` 由 `run_with` 在服务生命周期之外接好)。若某个服务需要
//! `Bot`,在构建 `Supervisor` 前把它塞进 [`ServiceBus`](crate::ServiceBus),或在实现服务
//! 时让它作为构造参数传入。
use nagisa_core::{
    collect_into, CooldownStore, EnabledOverrides, EnabledSet, FlightStore, Handler, KillSwitch,
    Matcher, Middleware, Rendezvous, Router, Rule, SleepState, Superusers, WaiterStore,
};
use nagisa_types::id::Uin;
use std::sync::Arc;

// 运行循环的接线(及其所需 import)仅在至少开启一个适配器 feature 时存在;否则就是死代码。
#[cfg(any(feature = "onebot", feature = "milky"))]
use {
    nagisa_core::{run_dispatch, Bot, Service, ShutdownToken, Supervisor},
    nagisa_types::error::Result,
    nagisa_types::event::Event,
    std::time::Duration,
    tokio::sync::mpsc,
    tokio::task::JoinSet,
};

/// 在事件 socket 起来期间,`get_login_info` 重试几次。
#[cfg(any(feature = "onebot", feature = "milky"))]
const LOGIN_RETRIES: usize = 20;
#[cfg(any(feature = "onebot", feature = "milky"))]
const LOGIN_RETRY_DELAY: Duration = Duration::from_millis(150);
/// 适配器 → 分发 的事件通道缓冲。
#[cfg(any(feature = "onebot", feature = "milky"))]
const EVENT_CHANNEL_CAP: usize = 256;

/// 应用构建器:把 handler 收集进一个 [`Router`],再让 router 跑在某个协议适配器之上,
/// 与所有用户登记的服务并行。
pub struct App {
    router: Router,
    /// app 的 `EnabledSet` 共享句柄。留着,好让 `restore_switches` 和 `on_switch_change` 在
    /// `Arc` 被塞进 router 的 state map 之后(`data_arc` 放进去的)仍能改它。
    enabled: Arc<EnabledSet>,
    /// app 的 `WaiterStore` 共享句柄(router 在 waiter-check 这一层查询的中断引擎注册表)。
    /// 留作内省访问器。
    waiters: Arc<WaiterStore>,
    /// app 自动提供的默认 `Rendezvous<String, Uin>` 共享句柄(token/bind 的规范存储)。留着,
    /// 好接 `restore`/`on_change`,也作内省访问器;`State<Rendezvous<String, Uin>>` 解析到的就
    /// 是同一个实例。需要别的类型可用 `App::data` 另注册一个。
    rendezvous: Arc<Rendezvous<String, Uin>>,
    /// app 的 `CooldownStore` 共享句柄(跨所有触发器共享的窗口式 `(count, first_ts)` 冷却注册表
    /// ——`UserGlobal` 语义)。声明式 `cooldown=` 门控的 `Check` 与命令式 `Cd` 注入
    /// 句柄都读它。留作内省访问器。
    cooldowns: Arc<CooldownStore>,
    /// app 的 `FlightStore` 共享句柄([`Session::single_flight`](nagisa_core::Session::single_flight)
    /// 据以获取的 single-flight 注册表 —— 那个让游戏/房间「重复开局」不可表达的 RAII guard)。
    /// 留作内省访问器。
    flight: Arc<FlightStore>,
    /// app 的全局总闸共享句柄(`switch()` 规则的紧急「全关」)。留着,好让管理代码在门控读的
    /// 同一个实例上翻它。
    kill_switch: Arc<KillSwitch>,
    /// app 的休眠标志共享句柄(`awake()` 门控的源)。留着,好让 `/sleep`
    /// 管理命令在门控读的活实例上翻它。
    sleep: Arc<SleepState>,
    /// 与适配器 + 分发循环并行运行的额外服务。由 [`Supervisor`] 管理(prepare → run →
    /// cleanup),跑在与适配器、分发任务并列的 `JoinSet` 里。
    #[cfg(any(feature = "onebot", feature = "milky"))]
    services: Vec<Arc<dyn Service>>,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// 新建一个 app。凡是被 `inventory` 收集、且能被链接器看到的 `#[command]` handler
    /// (见 [`nagisa_core::collect_into`])都会自动登记。用构建器方法补充显式的
    /// handler/state。
    ///
    /// ## DCE(死代码消除)警告
    ///
    /// `inventory` 靠**链接器**从二进制里的每个 crate 收集 `#[command]` 条目。如果某个
    /// 插件 crate 在二进制源码里没被别处引用,链接器可能悄悄丢掉整个编译单元 —— 连带它的
    /// `inventory::submit!` 条目一起丢。这些命令于是在运行期根本不出现,且没有任何报错。
    ///
    /// **你必须确保二进制引用了每个插件 crate** 来避免这点。两种惯用写法:
    ///
    /// ```ignore
    /// // 写法 1:在 main.rs 里 glob-use 插件 crate(无需真的 import 什么 —— 只要有
    /// // `use` 在,链接器就够了)。
    /// use my_plugins::*;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     App::new().run_onebot(cfg, shutdown).await.unwrap();
    /// }
    /// ```
    ///
    /// ```ignore
    /// // 写法 2:从插件 crate 暴露一个空操作的 `force_link()`,在 main 里调一次 ——
    /// // 比 glob 导入更直白。
    /// // 在 my_plugins/src/lib.rs:
    /// pub fn force_link() {}
    ///
    /// // 在 main.rs:
    /// fn main() {
    ///     my_plugins::force_link();
    ///     // ...
    /// }
    /// ```
    pub fn new() -> Self {
        // 自动注入：
        // - EnabledSet：业务可经 State<EnabledSet> 做 /enable、/disable；让
        //   #[command(default_enable=false)] 在运行期默认关闭。
        //   留一个 Arc 句柄,好让 restore_switches/on_switch_change 改动 router state map
        //   持有的同一个实例。
        // - WaiterStore（注入 state）：让 Session/Waiter 提取器无需额外配置；
        //   router 在 waiter-check 这一层从 state 里读取它（无 entry 时为 no-op）。
        // - Rendezvous<String, Uin>（注入 state）：token/bind 的跨会话默认存储，无需自己注册；
        //   否则缺失 State<Rendezvous> 只会让 handler 静默跳过（Reject::Error），难以排查。
        //   业务可用 App::data 覆盖（或注册其它 K/V 实例）。
        // - CooldownStore（注入 state）：让声明式 cooldown= 门控的 Check 与命令式 Cd 句柄
        //   无需手动配置，共用同一个进程级窗口存储（UserGlobal 跨触发器共享语义）。
        // - KillSwitch / SleepState（注入 state）：让 `switch()`（叠加全局总闸）与
        //   `awake()`（休眠门控）无需额外配置；管理命令经 handle 翻同一实例。
        let enabled = Arc::new(EnabledSet::new());
        let waiters = Arc::new(WaiterStore::new());
        let rendezvous = Arc::new(Rendezvous::<String, Uin>::default());
        let cooldowns = Arc::new(CooldownStore::new());
        let flight = Arc::new(FlightStore::new());
        let kill_switch = Arc::new(KillSwitch::new());
        let sleep = Arc::new(SleepState::new());
        Self {
            router: collect_into(Router::new())
                .data_arc(Arc::clone(&enabled))
                .data_arc(Arc::clone(&waiters))
                .data_arc(Arc::clone(&rendezvous))
                .data_arc(Arc::clone(&cooldowns))
                .data_arc(Arc::clone(&flight))
                .data_arc(Arc::clone(&kill_switch))
                .data_arc(Arc::clone(&sleep)),
            enabled,
            waiters,
            rendezvous,
            cooldowns,
            flight,
            kill_switch,
            sleep,
            #[cfg(any(feature = "onebot", feature = "milky"))]
            services: Vec::new(),
        }
    }

    /// 为这个 app 开启**开发/诊断模式**(往共享 state 里塞
    /// [`DevMode`](nagisa_core::DevMode) 标记)。
    ///
    /// 开发模式下,router 和 `Args<T>` 提取器不再*静默*跳过 handler:在每个跳过点
    /// (event-kind 不符、开关关闭、matcher 未命中、门控否决、提取器 `Skip`、`Args<T>`
    /// 解析失败)都打一条 `[dev]` `WARN`,说明 handler **为什么**没触发。对消息事件的
    /// `Args<T>` 解析失败,还会额外把命令的用法提示回给用户。跳过的*语义*不变 —— 这只加
    /// 诊断。在 setup 时调;生产环境别开。
    pub fn debug(mut self) -> Self {
        self.router = self.router.data(nagisa_core::DevMode);
        self
    }

    /// 登记一个 matcher 门控的命令 handler(委托给 [`Router::command`])。多数 bot 更推荐
    /// 用 `#[command]` 宏。
    pub fn command<H, Args>(mut self, matcher: Matcher, handler: H) -> Self
    where
        H: Handler<Args>,
        Args: 'static,
    {
        self.router = self.router.command(matcher, handler);
        self
    }

    /// 登记一个普通 handler:在每个事件上都跑,靠自身提取器的 `Skip` 自我过滤(委托给
    /// [`Router::on`])。
    pub fn on<H, Args>(mut self, handler: H) -> Self
    where
        H: Handler<Args>,
        Args: 'static,
    {
        self.router = self.router.on(handler);
        self
    }

    /// 登记一个一级 *top 观察者*(委托给 [`Router::on_top`]):它在 waiter-check 之前运行,
    /// 永不被消费型 waiter 拦住,所以即便在会话进行中也能看到每个事件。这是
    /// `#[command(top)]` / `#[event(.., top)]` 的命令式对应,让构建器风格的作者也能用上同一个
    /// top 分发层。适合那些必须看到全部流量的横切观察者(如发言计数器)。
    pub fn on_top<H, Args>(mut self, handler: H) -> Self
    where
        H: Handler<Args>,
        Args: 'static,
    {
        self.router = self.router.on_top(handler);
        self
    }

    /// 登记一个 matcher 门控的命令 handler,并额外用一个 [`Rule`](权限/场景)把守。规则在
    /// matcher 命中之后、handler 运行之前求值;不通过则跳过 handler。规则用 `&` / `|` / `!`
    /// 组合,例如 `nagisa::group_admin() | nagisa::superuser()`。
    pub fn command_with<H, Args>(mut self, matcher: Matcher, gate: Rule, handler: H) -> Self
    where
        H: Handler<Args>,
        Args: 'static,
    {
        self.router = self.router.command_with(matcher, gate, handler);
        self
    }

    /// 登记一个由 [`Rule`] 把守的普通 handler(在每个规则通过的事件上运行;handler 仍靠自身
    /// 提取器自我过滤)。
    pub fn on_with<H, Args>(mut self, gate: Rule, handler: H) -> Self
    where
        H: Handler<Args>,
        Args: 'static,
    {
        self.router = self.router.on_with(gate, handler);
        self
    }

    /// 加一层包住整条分发链的洋葱 [`Middleware`](委托给 [`Router::layer`])。越早加的层越靠
    /// 外。某个中间件不调 `next` 就返回 [`Flow::Stop`](nagisa_core::Flow) 会吞掉该事件。
    pub fn layer<M: Middleware>(mut self, m: M) -> Self {
        self.router = self.router.layer(m);
        self
    }

    /// 登记一份共享应用 state,handler 经 [`State<T>`](nagisa_core::State) 提取器取用(委托给
    /// [`Router::data`])。
    pub fn data<T: Send + Sync + 'static>(mut self, value: T) -> Self {
        self.router = self.router.data(value);
        self
    }

    /// 配置 [`superuser()`](nagisa_core::superuser) 规则查询的 superuser 集合。存为共享 state;
    /// 在 setup 时调一次。
    pub fn superusers(self, ids: impl IntoIterator<Item = impl Into<crate::Uin>>) -> Self {
        let set: std::collections::HashSet<crate::Uin> = ids.into_iter().map(Into::into).collect();
        self.data(Superusers(set))
    }

    /// 把持久化的开关覆盖加载进 app 的 `EnabledSet`(在 `App::new()` 之后、setup 时调)。用给
    /// 定的快照替换掉现有的全部覆盖。
    pub fn restore_switches(self, ov: EnabledOverrides) -> Self {
        self.enabled.restore(ov);
        self
    }

    /// app 的 `EnabledSet` 共享句柄。返回的 `Arc` 就是 router 据以门控的*同一个*实例,因此
    /// 经它做的改动(`set`/`reset`)会即时作用于运行中的分发 —— 适合从 handler 路径之外的管理
    /// 代码驱动开关。
    pub fn enabled_handle(&self) -> Arc<EnabledSet> {
        Arc::clone(&self.enabled)
    }

    /// app 的 `WaiterStore` 共享句柄(内省访问器)。
    pub fn waiter_store_handle(&self) -> Arc<WaiterStore> {
        Arc::clone(&self.waiters)
    }

    /// app 自动提供的默认 `Rendezvous<String, Uin>` 共享句柄。返回的 `Arc` 就是
    /// `State<Rendezvous<String, Uin>>` 解析到的*同一个*实例,可用来预置/查看 token,或接
    /// `restore`/`on_change`(持久化)。
    pub fn rendezvous_handle(&self) -> Arc<Rendezvous<String, Uin>> {
        Arc::clone(&self.rendezvous)
    }

    /// app 的 `CooldownStore` 共享句柄。返回的 `Arc` 就是 `State<CooldownStore>`(及 `Cd` 注入
    /// 句柄)解析到、且声明式 `cooldown=` 门控的 `Check` 打戳的*同一个*实例 —— 一个内省访问器。
    pub fn cooldown_store_handle(&self) -> Arc<CooldownStore> {
        Arc::clone(&self.cooldowns)
    }

    /// app 的 `FlightStore` 共享句柄。返回的 `Arc` 就是
    /// [`Session::single_flight`](nagisa_core::Session::single_flight) 据以获取的*同一个*实例 ——
    /// 一个用来查看当前哪些 scope 在 flight 中的内省访问器。
    pub fn flight_store_handle(&self) -> Arc<FlightStore> {
        Arc::clone(&self.flight)
    }

    /// app 的全局总闸共享句柄。返回的 `Arc` 就是 [`switch()`](nagisa_core::switch) 规则读的*同一个*
    /// 实例,翻它(`set(true)`)会否决每一个 `switch()` 门控的 handler —— 紧急「全关」+ 内省
    /// 访问器。
    pub fn kill_switch_handle(&self) -> Arc<KillSwitch> {
        Arc::clone(&self.kill_switch)
    }

    /// app 的休眠标志共享句柄。返回的 `Arc` 就是
    /// [`awake()`](nagisa_core::awake) 门控读的*同一个*
    /// 实例,因此 `/sleep` 管理命令是在门控查询的活实例上翻它。
    pub fn sleep_handle(&self) -> Arc<SleepState> {
        Arc::clone(&self.sleep)
    }

    /// 登记一个在每次开关改动(`EnabledSet::set`)时被调用的回调。用它把开关状态持久化到你的
    /// 存储(比如把快照写盘)。只能登记一个回调;再次调用会替换前一个。
    pub fn on_switch_change<F>(self, f: F) -> Self
    where
        F: Fn(&str, Option<nagisa_types::id::Peer>, bool) + Send + Sync + 'static,
    {
        self.enabled.on_change(f);
        self
    }

    /// 加一个额外的 [`Service`],与适配器 + 分发循环并行运行。
    ///
    /// 用户服务(数据库连接、渲染循环等)由 [`Supervisor`] 管理:它们的 `prepare` 阶段按依赖
    /// 拓扑序在主运行循环开始前执行,`run` 任务在同一个 `JoinSet` 里并发执行,`cleanup` 阶段在
    /// 停机时按逆拓扑序调用。
    ///
    /// **局限**:用户服务彼此共享一个 [`ServiceBus`](crate::ServiceBus),但不能经它拿到 [`Bot`]
    /// 句柄(`Bot` 在服务生命周期之外接好)。若某个服务需要 `Bot`,把它作为构造参数传入。
    #[cfg(any(feature = "onebot", feature = "milky"))]
    pub fn service(mut self, svc: Arc<dyn Service>) -> Self {
        self.services.push(svc);
        self
    }

    // 需要「账号可用、bot 就绪」的启动逻辑,表达为合成出的 `Meta::Ready` 事件的 handler ——
    // `#[event(Ready)] async fn(bot: Bot)`。`run_with` 在登录解析完成后注入一次
    // `Ready { self_id }`(仅当 `self_id != 0`),所以这类 handler 恰好运行一次,绝不会在账号
    // 可用之前跑,登录始终不成功时也绝不跑。(刻意不设 `on_ready` 回调:一种机制 —— 事件 ——
    // 就覆盖了主动式启动工作。)

    /// 让 app 跑在一个 OneBot v11 端点上。`shutdown` 触发或传输致命失败时返回。
    #[cfg(feature = "onebot")]
    pub async fn run_onebot(
        self,
        cfg: nagisa_onebot::OneBotConfig,
        shutdown: ShutdownToken,
    ) -> Result<()> {
        let adapter = nagisa_onebot::OneBotAdapter::new(cfg);
        self.run_with(adapter, shutdown).await
    }

    /// 让 app 跑在一个 Milky 端点上。`shutdown` 触发或传输致命失败时返回。
    #[cfg(feature = "milky")]
    pub async fn run_milky(
        self,
        cfg: nagisa_milky::MilkyConfig,
        shutdown: ShutdownToken,
    ) -> Result<()> {
        let adapter = Arc::new(nagisa_milky::MilkyAdapter::new(cfg)?);
        self.run_with(adapter, shutdown).await
    }

    /// 给任何「既是 `EventSource` 又是 `ActionInvoker`」的适配器共用的接线。对具体适配器类型
    /// 泛型,这样单个 `Arc<A>` 就同时满足两个 trait bound,不必再分配第二份。
    ///
    /// 经 [`App::service`] 登记的用户服务由 `Supervisor` 管理;它们的
    /// `prepare`/`run`/`cleanup` 生命周期与适配器 + 分发任务一并被执行。
    #[cfg(any(feature = "onebot", feature = "milky"))]
    async fn run_with<A>(self, adapter: Arc<A>, shutdown: ShutdownToken) -> Result<()>
    where
        A: nagisa_core::EventSource + nagisa_core::adapter::Actions,
    {
        let router = Arc::new(self.router);
        let (tx, rx) = mpsc::channel::<Event>(EVENT_CHANNEL_CAP);

        // —— 1. spawn 事件源(自己管 connect + reconnect)。 ——
        let mut tasks: JoinSet<Result<()>> = JoinSet::new();
        {
            let source = Arc::clone(&adapter);
            let sink = tx.clone();
            let source_shutdown = shutdown.clone();
            tasks.spawn(async move { source.run(sink, source_shutdown).await });
        }

        // —— 2. socket 起来后解析 self_id(+ nickname)(急切,带重试)。 ——
        let (self_id, nickname) = resolve_self_id(
            Arc::clone(&adapter) as Arc<dyn nagisa_core::adapter::ActionInvoker>,
            &shutdown,
        )
        .await;
        let bot = Bot::new(adapter, self_id);

        // —— 3. spawn 分发循环。 ——
        {
            let router = Arc::clone(&router);
            let dispatch_shutdown = shutdown.clone();
            tasks.spawn(async move {
                run_dispatch(router, bot, rx, dispatch_shutdown).await;
                Ok(())
            });
        }

        // —— 3b. 发出合成的 `Ready` 事件(登录已解析,句柄已就绪)。 ——
        // 仅当账号可用(`self_id != 0`)时发;始终解析不出的登录得到 `Uin(0)`,于是不发
        // Ready —— 所以没有可用账号时 `#[event(Ready)]` 的启动逻辑保持休眠,这是设计如此。
        // 它作为一个普通事件,注入喂给分发的同一个通道。放在分发循环 spawn *之后*做(此时消费者
        // 已在抽 `rx`),所以即便事件源在 `resolve_self_id` 等待期间填满了缓冲,这个
        // `send().await` 也绝不会死锁。随后我们 drop 自己的 sender,这样事件源退出后通道照样关闭。
        // 重连不会再发 Ready(每次 `run_with` resolve_self_id 只跑一次)。
        if self_id.0 != 0 {
            let _ = tx
                .send(Event::Meta(nagisa_types::event::Meta::Ready { self_id, nickname }))
                .await;
        }
        drop(tx);

        // —— 4. 经 Supervisor 运行用户服务(若有)。 ——
        //
        // Supervisor 负责用户服务的 prepare→run→cleanup。它们的 `run` 任务在 Supervisor::run
        // 内部被 spawn 到一个*独立的* JoinSet 里;我们把整个 Supervisor 生命周期包进单个任务,
        // 好让停机传播保持统一。
        if !self.services.is_empty() {
            let mut supervisor = Supervisor::new();
            for svc in self.services {
                supervisor = supervisor.add(svc);
            }
            let svc_shutdown = shutdown.clone();
            tasks.spawn(async move {
                supervisor.run(svc_shutdown).await
            });
        }

        // —— 5. 在一个尊重 shutdown 的 select 下 join 所有任务。 ——
        run_until_done(tasks, shutdown).await
    }
}

/// 经 `get_login_info` 急切取 `(self_id, _)`,在事件 socket 建立期间重试。预算内始终不成功
/// 则退回 `Uin(0)`(带一条 warning);停机时立即返回 `Uin(0)`。
#[cfg(any(feature = "onebot", feature = "milky"))]
async fn resolve_self_id(
    invoker: Arc<dyn nagisa_core::adapter::ActionInvoker>,
    shutdown: &ShutdownToken,
) -> (Uin, String) {
    for attempt in 0..LOGIN_RETRIES {
        if shutdown.is_cancelled() {
            return (Uin(0), String::new());
        }
        match invoker.get_login_info().await {
            Ok((uin, nickname)) => {
                tracing::info!(self_id = uin.0, nickname = %nickname, "resolved bot login info");
                return (uin, nickname);
            }
            Err(e) => {
                tracing::debug!(attempt, error = %e, "get_login_info not ready yet; retrying");
                tokio::select! {
                    _ = shutdown.cancelled() => return (Uin(0), String::new()),
                    _ = tokio::time::sleep(LOGIN_RETRY_DELAY) => {}
                }
            }
        }
    }
    tracing::warn!(
        "could not resolve bot self_id via get_login_info; mention matching will be inert until reconnect"
    );
    (Uin(0), String::new())
}

/// await 所有已 spawn 的任务。在 `shutdown` 触发、某个任务返回 `Err`(向上传播,并触发其余
/// 任务停机)、或全部完成时返回。
#[cfg(any(feature = "onebot", feature = "milky"))]
async fn run_until_done(mut tasks: JoinSet<Result<()>>, shutdown: ShutdownToken) -> Result<()> {
    let mut result: Result<()> = Ok(());
    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => break,
            joined = tasks.join_next() => {
                match joined {
                    None => break, // 所有任务已结束
                    Some(Ok(Ok(()))) => continue, // 某个任务干净退出;继续等
                    Some(Ok(Err(e))) => {
                        tracing::error!(error = %e, "app task returned error; shutting down");
                        result = Err(e);
                        break;
                    }
                    Some(Err(join_err)) => {
                        tracing::error!(error = %join_err, "app task panicked; shutting down");
                        // panic 的任务被隔离,不向上重抛。触发停机。
                        break;
                    }
                }
            }
        }
    }
    // 返回前确保一切收尾。
    shutdown.cancel();
    tasks.shutdown().await;
    result
}
