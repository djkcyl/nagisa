//! Nagisa —— 统一在 OneBot v11 与 Milky 之上的 Rust 聊天机器人框架(门面 crate)。
//!
//! 这是面向使用者的唯一入口。bot 作者只依赖 `nagisa`(按需开 `onebot` / `milky`
//! feature),写 `use nagisa::prelude::*;`,从不直接点名底层的 engine / types /
//! macros / adapter 这些 crate:
//!
//! ```rust,ignore
//! use nagisa::prelude::*;
//!
//! #[command("ping", mention_me)]
//! async fn ping(reply: Reply) -> HandlerResult {
//!     reply.text("pong").await?;
//!     Ok(())
//! }
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     // ctrl_c_shutdown() 起一个后台任务,收到 SIGINT/Ctrl-C 时取消 shutdown
//!     // token,让 bot 停机。
//!     let shutdown = ctrl_c_shutdown();
//!     App::new()
//!         .run_onebot(OneBotConfig::new("ws://127.0.0.1:8080/onebot/v11/ws"), shutdown)
//!         .await
//! }
//! ```
//!
//! # 门面做了什么
//!
//! - 把 engine([`nagisa_core`])、统一领域模型([`nagisa_types`])、宏
//!   ([`nagisa_macros`])在 crate 根重新导出,业务代码只点 `nagisa::…`。
//! - 精选一个 [`prelude`] —— `use nagisa::prelude::*;` 这一行 glob 导入,带进宏、
//!   [`App`] 构建器、[`Bot`] 句柄、常用 `FromContext` 提取器、统一的
//!   `Segment` / `Message` / `Peer` / `Uin` / `Result` / `Error`,以及门控/规则
//!   组合子。
//! - 协议适配器(`OneBotConfig` / `MilkyConfig`)挂在 `onebot` / `milky` feature
//!   后面,日志工具箱(`nagisa::log`)挂在 `log` feature 后面,排版引擎
//!   (`nagisa::render` —— 文档排版成图片)挂在 `render` feature 后面。
//! - 提供 [`App`] 构建器([`app`]),把适配器、分发循环、自动登记的 handler 和一个
//!   [`ShutdownToken`] 接到一起。
//!
//! 适配器作者用的管线(新协议适配器要实现的出站动作 trait)在 [`adapter`];
//! engine 内部的对象安全机制在 [`engine`]。业务代码两者都不点名。
#![forbid(unsafe_code)]

pub mod app;

pub use app::App;

// ── Engine 层(nagisa-core) ────────────────────────────────────────────────────
// 分发引擎、`Bot` 句柄、两个传输 trait、`FromContext` 提取器、matcher DSL、注册表
// 和服务生命周期。在 crate 根重新导出,业务代码不直接点 `nagisa_core` 路径。
pub use nagisa_core::{
    // 出站消息日志器:每条 bot 发出的消息都会喂给已注册的日志器(多订阅)。
    add_outgoing_logger,
    awake,
    awake_silent,
    // 注册表(inventory)+ 插件元数据。`registered_triggers` 报的是已解析到插件的点分 key
    // (宏先发空 key,解析期按 module_path 关联补上)。
    collect_into,
    from_user,
    group_admin,
    group_only,
    group_owner,
    in_group,
    keyword,
    private,
    registered_plugins,
    registered_triggers_resolved as registered_triggers,
    replying,
    // 分发引擎入口。
    run_dispatch,
    superuser,
    // 总闸 / 休眠的叶子构造器 + 否决时的回复通道。
    switch,
    to_me,
    // 事件触发器(`#[event(Kind)]`):EventKind 选择器 + 细粒度的带类型事件提取器(只有
    // Nudge 做自身事件过滤 —— Recall 原样投递,让 handler 自己决定防撤回策略)。
    AdminChange,
    // 带类型的命令参数解析(Args<T> 类型;#[derive(Args)] 是宏)。
    ArgError,
    // 参数元数据(#[derive(Args)] 生成,供 help 自动生成用法)。
    ArgKind,
    ArgSpec,
    ArgText,
    ArgToken,
    Args,
    ArgsMeta,
    // 在匹配链上取元素的提取器,与 waiter 共用同一套类型:同一个 `Image` 既读内联图片槽,
    // 也是 `recv::<Image>` 的返回(「内联或追问是同一类型」)。`Option<At>`/`Option<Image>`
    // 即可选。
    At,
    // `Bot` 句柄。它的固有方法就是业务调用的动作面;支撑这些方法、由适配器实现的 trait
    // 在 [`adapter`]。
    Bot,
    BotOffline,
    BotOnline,
    Captures,
    Category,
    // 冷却门控(默认 UserGlobal、窗口内 max_exec)+ 命令式的 Cd 注入句柄。
    Cd,
    CdKey,
    Command,
    CommandArg,
    // 生命周期事件提取器(`#[event(Connect/Disconnect/Ready/BotOnline/BotOffline/Heartbeat)]`)。
    Connect,
    Cooldown,
    CooldownScope,
    CooldownStore,
    // 每事件上下文 + 提取器(FromContext)。
    Ctx,
    // 开发/诊断标记(配 `App::debug()`):把静默跳过变成 `[dev]` WARN,并对 `Args<T>`
    // 解析失败自动回一条用法提示。
    DevMode,
    Disconnect,
    // 运行期命令开关。
    EnabledOverrides,
    EnabledSet,
    EventKind,
    EventPeer,
    EventSource,
    Extracted,
    // 带类型的具名槽 matcher:HEAD 侧,与 TAIL 侧的 Args<T> 对称(#[derive(Slots)] 是宏)。
    Flank,
    // 中断引擎(Session/Waiter/Scope)。
    FlightGuard,
    // 洋葱中间件(链路)。
    Flow,
    FriendAdd,
    // 带类型的目标选择器(bot.group(g).member(u).mute(..))。
    FriendRef,
    FriendRequest,
    FromArg,
    FromContext,
    FromSlots,
    FromTailText,
    GateReply,
    GroupCardChange,
    GroupJoinRequest,
    GroupMessage,
    GroupRef,
    // Handler 抽象。
    Handler,
    HandlerOutcome,
    HandlerResult,
    Heartbeat,
    Honor,
    Image,
    KillSwitch,
    LuckyKing,
    // Matcher(一个 regex 支撑的触发器;command([..]) 是字面量糖,slots(..) 是带类型的头部)。
    Matcher,
    MemberJoin,
    MemberLeave,
    MemberRef,
    Middleware,
    Mute,
    NamedCaptures,
    Next,
    Nudge,
    ParseArgs,
    ParsedCommand,
    PluginMeta,
    PrivateMessage,
    // 内置限流器(经 App::layer 选用)。
    RateLimit,
    RateLimitScope,
    Ready,
    Recall,
    Reject,
    // 跨会话 rendezvous(token/bind:此处签发,彼处认领)。
    Rendezvous,
    RendezvousSnapshot,
    Reply,
    ReplyMsg,
    Router,
    // 规则/权限组合代数(& | !)+ 内置规则。
    Rule,
    Scope,
    Sender,
    // 服务生命周期(Supervisor + DAG)。
    Service,
    ServiceBus,
    Session,
    // 根取消/停机信号。
    ShutdownToken,
    SleepState,
    SlotSpec,
    SlotValue,
    Slots,
    State,
    Superusers,
    Supervisor,
    SwitchKey,
    Tail,
    ToMe,
    TriggerId,
    TriggerKind,
    TriggerMeta,
    TriggerSpec,
    WaitFlow,
    Waiter,
    WaiterDepth,
    WaiterStore,
};

/// 适配器作者用的管线:新协议适配器要实现的出站动作 trait —— `ActionInvoker`
/// (两协议通用的根)、`OneBotActions`(OneBot v11 加上揉进来的 NapCat / LLOneBot /
/// Lagrange 厂商扩展)、`MilkyActions`,由 `Actions` marker 合在一起。业务代码从不
/// 点名这些 —— 它调 [`Bot`] 的固有方法,厂商专属动作走 `bot.actions().<m>()` ——
/// 所以放在这个子模块而非精选的 crate 根。转自 [`nagisa_core::adapter`]。
pub mod adapter {
    pub use nagisa_core::adapter::{ActionInvoker, Actions, MilkyActions, OneBotActions};
}

/// Engine 内部管线:分发引擎据以搭建的对象安全 handler/rule/waiter 机制
/// (`ErasedHandler`、`Check`、`Selector`、`WaiterBuilder`、`StateMap`)。handler
/// 作者从不直接点名这些;不进精选的 crate 根。转自 [`nagisa_core::engine`]。
pub mod engine {
    pub use nagisa_core::engine::{Check, ErasedHandler, Selector, StateMap, WaiterBuilder};
}

// `inventory`由 `nagisa_core` 转出;这里再露一层,让 `#[command]` 宏生成的
// `::nagisa_core::inventory::submit!` 经门面也能解析,插件 crate 无需直接依赖它。
pub use nagisa_core::inventory;

// `async_trait` 转出:实现自定义 `Service` / `ActionInvoker` 的作者要给 `impl` 标
// `#[nagisa::async_trait]`,否则会撞上费解的 `E0195`。经门面露出,免去直接依赖
// `async-trait`。
pub use nagisa_core::async_trait;

// ── 抽象层(nagisa-types) ──────────────────────────────────────────────────────
// 跨协议的领域模型:事件、消息、段、实体、id、资源、统一的
// `Error`/`Result`,以及 `Capability`。
pub use nagisa_types::capability::{Capability, Protocol};
pub use nagisa_types::vendor::Vendor;
// 实体类型 —— `Bot` 方法返回的统一领域结构。handler 若要写自己的 `Result<T>`
// (如 `nagisa::Result<nagisa::UserInfo>`)需要它们在根可见,所以全部精选到这里。
pub use nagisa_types::entity::{
    AiCharacter, AiCharacterGroup, Announcement, Business, EssenceMessage, FileFetch, FileMeta, FriendCategory,
    FriendCategoryList, FriendGroup, FriendInfo, FriendStatus, GroupFileList, GroupFolder, GroupInfo, HonorList,
    HonorMember, ImplStatus, MemberInfo, ProfileLiker, Rkey, Role, Sex, UserInfo,
};

/// `ImplInfo`:`get_impl_info()` 的返回 —— 已连接实现的身份(app 名/版本、QQ 协议
/// 信息)。定义在 engine crate。厂商判定走 `bot.vendor()`(OneBot 专用轴),不在此类型上。
pub use nagisa_core::ImplInfo;
pub use nagisa_types::context::Context;
pub use nagisa_types::error::{ActionErrorKind, Error, Result, TransportError};

/// `bail!("...")` / `bail!(kind, "...")`:从 handler 里提前返回一个 `nagisa`
/// [`Error`],对标 `anyhow::bail!`。见 [`Error::action`]。
pub use nagisa_types::bail;
pub use nagisa_types::event::{
    Event, HonorKind, MessageEvent, Meta, Notice, RawEvent, ReactionKind, Request, RequestToken,
};
pub use nagisa_types::id::{MessageId, Peer, Scene, Uin};
pub use nagisa_types::message::{Message, MessageExt, Msg};
pub use nagisa_types::resource::{Media, ResourceRef, ResourceSource};
pub use nagisa_types::segment::{Forward, ForwardNode, ImageSubType, Segment};

// 同时把 `nagisa_types` crate 本身露出来,作为罕见的逃生口:业务偶尔需要 prelude
// 没暴露的名字。
pub use nagisa_types;

// 把支撑用的 engine crate 以自己的名字露出,好让 `#[command]` / `#[event]` /
// `plugin!` / `#[derive(Args)]` / `#[derive(ArgEnum)]` 这些宏发出的全限定路径仅经门面
// 就能解析 —— 守住「bot 作者只依赖 `nagisa`」的承诺。当门面是被依赖的 crate 时,
// `nagisa-macros` 用 `proc-macro-crate` 把它发出的 `nagisa_core::…` 根改写成
// `::nagisa::nagisa_core::…`;正是这条重新导出让那些路径点到 engine(`plugin`/`registry`/
// `args` 子模块本就是 `pub`,深层尾巴也能解析)。与上面 `pub use nagisa_types;` 逃生口对称。
#[doc(hidden)]
pub use nagisa_core;

// ── SIGINT 便捷封装 ─────────────────────────────────────────────────────────────

/// 起一个后台任务,收到 SIGINT(Ctrl-C)时取消返回的 [`ShutdownToken`]。
///
/// 这是让 bot 可停机最省事的办法:不必自己 new 一个 [`ShutdownToken`] 再接信号
/// 处理,直接 `ctrl_c_shutdown()` 把结果传给 `run_onebot`/`run_milky` 即可。
///
/// ```ignore
/// #[tokio::main]
/// async fn main() -> nagisa::Result<()> {
///     let shutdown = nagisa::ctrl_c_shutdown();
///     App::new().run_onebot(cfg, shutdown).await
/// }
/// ```
///
/// 任务会忽略 `tokio::signal::ctrl_c` 的错误(比如平台不支持)—— 那种平台上 token
/// 只是不会从这个来源触发,你仍可手动取消它。
pub fn ctrl_c_shutdown() -> ShutdownToken {
    let token = ShutdownToken::new();
    let child = token.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_err() {
            tracing::warn!("ctrl_c signal listener failed; shutdown will not fire on Ctrl-C");
        }
        child.cancel();
    });
    token
}

// ── 宏(nagisa-macros) ─────────────────────────────────────────────────────────
/// `#[command(...)]` 属性宏:声明式登记一个 matcher 门控的 handler,在 [`App::new`]
/// 时经 `inventory` 自动收集。
pub use nagisa_macros::command;

/// `plugin!{ .. }` 宏:把当前模块声明为一个插件。同模块(及更下层)的触发器按
/// `module_path!()` 关联到它,其分层开关继承插件的 `default_enable`/`can_disable`。
/// 见插件模型。
pub use nagisa_macros::plugin;

/// `#[event(Kind, id="..")]` 属性宏:声明式登记一个事件触发的 handler(与
/// `#[command]` 对等),在给定 [`EventKind`] 的事件上触发,受同一套分层插件开关门控。
pub use nagisa_macros::event;

/// `#[derive(Args)]`:为结构体派生 [`ParseArgs`],使其可经 [`struct@Args`]`<T>` 从命令
/// 参数里提取。派生宏与 `Args<T>` 提取器刻意共用 `Args` 这个名字(跨宏/类型命名空间),
/// 同 serde 的 `Serialize`。
pub use nagisa_macros::Args;

/// `#[derive(ArgEnum)]`:为无字段枚举派生 [`FromArg`](变体名转小写;支持
/// `#[arg(rename="..")]`/`#[arg(alias="..")]`),使其可作受限的 `Args<T>` 字段类型
/// (如 `on|off`)。
pub use nagisa_macros::ArgEnum;

/// `#[derive(Slots)]`:为带结构体级 `#[slots(full="..")]` 头部、以及带类型可选
/// `#[slot(re=..)]`/`#[slot(union=[..])]`/`#[slot(tail)]` 正则槽的结构体派生 [`FromSlots`],
/// 可经 [`struct@Slots`]`<T>` 提取。HEAD 侧,与持有 TAIL 的 `#[derive(Args)]` 对称。
/// `Option<T>` 表可选;元组字段是多分组 codegen(如 `Option<(u8,u8)>`)。具名槽版的
/// 带类型参数解析。
pub use nagisa_macros::Slots;

/// `matcher! { full = "..", <field>: <ty> = re("..")|union("a","b")|tail }`:对同一套
/// `#[derive(Slots)]` codegen 的函数式糖,用于写一个简单的内联头部 matcher;求值为
/// [`Matcher`]。派生宏才是规范形式(它还顺带产出 `Slots<T>` 提取器类型)。
pub use nagisa_macros::matcher;

// ── 协议适配器(feature 门控) ─────────────────────────────────────────────────
#[cfg(feature = "onebot")]
pub use nagisa_onebot;
#[cfg(feature = "onebot")]
pub use nagisa_onebot::{OneBotAdapter, OneBotConfig};

#[cfg(feature = "milky")]
pub use nagisa_milky;
#[cfg(feature = "milky")]
pub use nagisa_milky::{MilkyAdapter, MilkyConfig, MilkyMode};

// ── 日志门面(feature 门控) ────────────────────────────────────────────────────
/// 可选的日志工具箱(`use nagisa::log::*;`),挂在 `log` feature 后面,好让 bot 作者只
/// 依赖 `nagisa`。转出 [`nagisa_log`]:
///
/// - [`render`](nagisa_log::render)`(&Event) -> String` —— 纯函数的可读事件渲染器;
/// - [`EventLog`](nagisa_log::EventLog) —— 给 `App::on_top` 用的可选 top 观察者(运行期
///   开关 + 按 kind 过滤);
/// - [`init`](nagisa_log::init) —— 统一记录器(控制台 + 可选滚动文件 + 按 source/`target`
///   过滤),同时返回「日志即事件」的 [`LogBus`](nagisa_log::LogBus);
/// - [`on_record`](nagisa_log::on_record) —— 把 bus 抽到一个异步 sink 里,让业务持久化日志,
///   与消息 Event/handler 路径分开(防自激)。
#[cfg(feature = "log")]
pub use nagisa_log as log;

/// 可选排版引擎(`use nagisa::render::*;`),挂在 `render` feature 后面,好让 bot 作者只
/// 依赖 `nagisa`。转出 [`nagisa_render`]:把标记文本 / 构建器文档排版成图片字节(PNG / WebP),
/// 再经既有的 `Segment::image_bytes` 出图。
#[cfg(feature = "render")]
pub use nagisa_render as render;

/// bot 作者唯一需要的一行 glob 导入:`use nagisa::prelude::*;`。
///
/// 带进:
/// - 宏 —— `#[command]` / `#[event]` / `plugin!{}` / `#[derive(Args)]` /
///   `#[derive(ArgEnum)]` / `#[derive(Slots)]` / `matcher!{}` / `bail!`;
/// - [`App`] 构建器、[`Bot`] 句柄、[`Router`],以及
///   [`ctrl_c_shutdown`] / [`ShutdownToken`];
/// - handler 签名按类型点名的常用 `FromContext` 提取器
///   (`Reply` / `GroupMessage` / `Command` / `ArgText` / `State<T>` / `Session` /
///   `Sender` / 各 `#[event]` 提取器 …);
/// - 门控/规则组合子(`to_me` / `group_admin` / `superuser` / … 以及
///   `switch` / `awake` / `Cooldown`)、中断引擎句柄
///   (`Session` / `Waiter` / `Scope` / `Rendezvous`)、带类型目标选择器
///   (`GroupRef` / `MemberRef` / `FriendRef`);
/// - 统一领域模型(`Segment` / `Message` / `Peer` / `Uin` /
///   `MessageId` / `Event` / `Result` / `Error` …)。
///
/// 适配器配置(`OneBotConfig` / `MilkyConfig` / `MilkyMode`)在对应 feature 开启时
/// 一并带入。
pub mod prelude {
    // 宏:#[command]/#[event] 属性、plugin!{} 声明、
    // #[derive(Args)]/#[derive(ArgEnum)]/#[derive(Slots)] 派生、matcher!{} 糖,
    // 以及让 handler 提前失败的 `bail!`。
    pub use crate::{bail, command, event, matcher, plugin, ArgEnum, Args, Slots};

    // 自定义参数解析:`ParseArgs` trait 及其 token/error 类型,加上字段类型 trait
    // `FromArg` —— 给手写 `Args<T>` impl(而非 `#[derive(Args)]`)的 handler 用。
    // `ArgSpec`/`ArgKind`/`ArgsMeta`:参数元数据,help 据此自动生成用法。
    pub use crate::{ArgError, ArgKind, ArgSpec, ArgToken, ArgsMeta, FromArg, ParseArgs};

    // 带类型的具名槽 matcher(HEAD 侧,与 Args<T> 对称):`Tail` 通配载荷,以及手写头部用的
    // `FromSlots`/`SlotValue` trait。(`Slots<T>` 本身随上面 `#[derive(Slots)]` 宏一起带入 ——
    // 类型与派生共用名字,同 `Args`。)
    pub use crate::{FromSlots, FromTailText, SlotValue, Tail};

    // App + engine 句柄。
    pub use crate::{ctrl_c_shutdown, App, Bot, Router, ShutdownToken};

    // `#[async_trait]` —— `FromContext`/自定义 async trait 绕不开的搭档
    // (`#[async_trait] impl FromContext for MyExtractor { async fn from_context(ctx: &Ctx) .. }`)。
    // 经 glob 露出,自定义提取器作者无需再写一行 import,也无需冗长的 `#[nagisa::async_trait]`。
    // (crate 根也仍有这条转出。)
    pub use crate::async_trait;

    // 服务生命周期(业务偶尔会用到)。
    pub use crate::{Service, ServiceBus, Supervisor};

    // handler 签名按类型点名的常用提取器,加上自定义 `FromContext` impl 作者会用到的
    // 提取结果类型(`Extracted`/`Reject`)。
    pub use crate::{
        ArgText, At, Captures, Command, CommandArg, Ctx, EventPeer, Extracted, FromContext, GroupMessage,
        HandlerResult, Image, PrivateMessage, Reject, Reply, ReplyMsg, Sender, State, ToMe,
    };

    // 规则/权限组合子 + 内置规则(用 `&` / `|` / `!` 组合)。`Superusers` 是 `superuser()`
    // 背后注入的集合 —— 需要手动判断(比如按是否 superuser 分支而非直接门控)的 handler 经
    // `State<Superusers>` 点名它。
    pub use crate::{
        awake, awake_silent, from_user, group_admin, group_only, group_owner, in_group, keyword, private, replying,
        superuser, switch, to_me, GateReply, KillSwitch, Rule, SleepState, Superusers,
    };

    // 冷却门控(经 `Cooldown::into_rule` 变成 `Rule`)+ 命令式的 `Cd` 注入句柄。
    pub use crate::{Cd, CdKey, Cooldown, CooldownScope, CooldownStore, TriggerId};

    // 洋葱中间件。
    pub use crate::{Flow, Middleware, Next, RateLimit, RateLimitScope};

    // 中断引擎(Session/Waiter/Scope)。
    pub use crate::{Rendezvous, Scope, Session, WaitFlow, Waiter};

    // 带类型的目标选择器。
    pub use crate::{FriendRef, GroupRef, MemberRef};

    // 插件元数据(给 help/菜单插件用)+ 运行期开关。
    pub use crate::{
        collect_into, registered_plugins, registered_triggers, Category, EnabledSet, EventKind, PluginMeta, SwitchKey,
        TriggerKind, TriggerMeta, TriggerSpec,
    };

    // 事件触发提取器(`#[event(Kind)]` handler 签名按类型点名),含生命周期:
    // Connect/Disconnect(传输)、Ready(框架)、BotOnline/BotOffline(账号)、Heartbeat。
    pub use crate::{
        AdminChange, BotOffline, BotOnline, Connect, Disconnect, FriendAdd, FriendRequest, GroupCardChange,
        GroupJoinRequest, Heartbeat, Honor, LuckyKing, MemberJoin, MemberLeave, Mute, Nudge, Ready, Recall,
    };

    // 统一领域模型。
    pub use crate::{
        ActionErrorKind, Capability, Context, Error, Event, Forward, ForwardNode, FriendInfo, GroupInfo, Media,
        MemberInfo, Message, MessageEvent, MessageExt, MessageId, Msg, Notice, Peer, Protocol, ReactionKind, Request,
        ResourceSource, Result, Scene, Segment, Uin,
    };

    // 适配器配置(feature 门控)。
    #[cfg(feature = "onebot")]
    pub use crate::OneBotConfig;
    #[cfg(feature = "milky")]
    pub use crate::{MilkyConfig, MilkyMode};

    // 协议/厂商专属动作 trait：让 `bot.actions().<method>()` 直达口的方法在作用域内可解析
    // （`Actions` 是组合 marker，`OneBotActions`/`MilkyActions` 提供实际方法）。
    pub use crate::adapter::{Actions, MilkyActions, OneBotActions};
}
