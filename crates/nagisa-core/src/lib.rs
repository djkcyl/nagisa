//! Nagisa 框架运行时核心：统一 [`Bot`] 句柄、[`EventSource`]/[`ActionInvoker`](adapter::ActionInvoker)
//! 两个传输 trait，以及把二者串起来的 dispatch 引擎。
//!
//! adapter（`nagisa-onebot` / `nagisa-milky`）实现这两个 trait，把协议 wire 翻译进出
//! `nagisa-types` 的统一类型；业务 / 上层门面（`nagisa` crate 的 `App`）只面向本 crate 的抽象。
//!
//! # 模块地图
//!
//! 传输与运行：
//! - [`source`]：入站事件源 trait（`EventSource`）。
//! - [`bot`] / [`invoker`] / [`refs`] / [`impl_info`]：出站动作。[`Bot`] 是廉价克隆的句柄，
//!   内含 `Arc<dyn Actions>`；[`invoker`] 把动作分三层（[`ActionInvoker`](adapter::ActionInvoker)
//!   两协议通用、[`OneBotActions`](adapter::OneBotActions) OneBot 独有 + 厂商扩展、
//!   [`MilkyActions`](adapter::MilkyActions) Milky 独有），[`refs`] 是按目标分组的句柄糖。
//! - [`dispatch`]：消费事件流、逐个构造 [`Ctx`]、并发分发给 [`Router`]。
//! - [`reconnect`]：带上限指数退避的重连循环 helper（adapter 用）。
//! - [`sse`]：纯字节级 SSE 解析器（adapter 用）。
//! - [`wire`]：两适配器共用的 wire 基建——`log_wire` 协议帧日志、零依赖 base64、HTTP 动作通道
//!   骨架（adapter 用）。
//! - [`framesource`]：连接后的公共泵骨架——`FrameSource`/`Frame`/`pump`（adapter 用）。
//! - [`service`]：服务生命周期 `Supervisor` + 依赖 DAG + `ServiceBus`，门面据此搭线。
//!
//! 分发与匹配（路1：声明式 handler）：
//! - [`ctx`]：每事件上下文 `Ctx`（提取器从它取料）。
//! - [`router`]：handler 注册表 + app 共享状态 + 洋葱中间件 + 三层分发。
//! - [`handler`] / [`extract`] / [`args`] / [`slots`]：handler 抽象 + `FromContext`
//!   提取器 + 声明式参数（TAIL 段流）/ 命名 slot（HEAD 捕获组）解析。
//! - [`matcher`]：命令触发匹配器（字面量 / 正则 / 有序 slot）+ MentionMe 预处理。
//! - [`rule`]：可组合门控 `Rule`（`&`/`|`/`!` 代数）+ 权限 / 场景 / kill-switch / 休眠叶子。
//! - [`middleware`] / [`ratelimit`]：洋葱中间件 + 内置限流。
//! - [`cooldown`]：冷却门控（声明式 `Rule` + 命令式 `Cd` 句柄）。
//!
//! 插件与开关：
//! - [`plugin`] / [`registry`]：`inventory` 编译期收集的插件 / 触发器静态模型。
//! - [`enabled`]：分层启用 / 禁用状态（插件总开关 + 触发器子开关，按会话覆盖）。
//! - [`event_trigger`]：`EventKind` 判别式 + 细粒度事件提取器（`#[event(Kind)]` 用）。
//!
//! 交互（路2：命令式 waiter / session）：
//! - [`session`]：`Session`/`Waiter` 中断引擎 + `single_flight`。
//! - [`rendezvous`]：跨会话「留 token、稍后取」的 TTL 存储。
//!
//! # 派发模型：事件 → router → gate → handler
//!
//! [`dispatch::run_dispatch`] 从 [`EventSource`] 收事件，每个事件在独立 `tokio` 任务里
//! 构造一个 [`Ctx`]（共享 router 状态 + bot），交给 [`Router::dispatch`]。router 先穿过
//! 洋葱中间件链，末端跑**三层** handler（见 [`Router`]）：
//!
//! 1. **top observer**（`on_top`）：永远先跑、不被 waiter 拦。
//! 2. **waiter 检查**：若有挂起 waiter 命中本事件且其 `block` 为真，吞掉事件、不再下传。
//! 3. **default handler**：按优先级（小者先）逐个跑。
//!
//! 每个命令 / 事件型 handler 内部依次过：事件种类预筛（`#[event]`）→ 启用门
//! （[`EnabledSet`]）→ 匹配器（[`Matcher`]，命中产出 `ParsedCommand`）→ 门控
//! [`Rule`]（权限 / 场景 / 冷却）→ handler 调用。任一步不过即跳过本 handler，**同层
//! handler 互不阻断**。门控否决时可经 reply-on-veto 通道回贴一句（如休眠的「Zzz」）。
//!
//! # 交互模型：Waiter / Session
//!
//! 处理一个事件时，handler 可从 [`Ctx`] 提取一个 [`Session`]，再 `.waiter()` 挂起一个
//! [`Waiter`]：它把后续命中事件经 `mpsc` 投递回来（多轮、可嵌套，深者优先），handler
//! 借此实现「追问 → 等回答」的多步对话，无需为每步注册全局 handler。需要跨**两个独立
//! 事件**（可能相距很远、在不同会话）的流程则用 [`Rendezvous`]。
#![forbid(unsafe_code)]
// 溯源注释里特意保留裸 URL(OFFICIAL:/ENDPOINT: 行,非给 rustdoc 渲染的链接);
// 不为它们刷 190 条 bare_urls 告警、淹没真问题。
#![allow(rustdoc::bare_urls)]

pub mod args;
pub mod bot;
pub mod event_trigger;
pub use event_trigger::{
    AdminChange, BotOffline, BotOnline, Connect, Disconnect, EventKind, FriendAdd, FriendRequest, GroupCardChange,
    GroupJoinRequest, Heartbeat, Honor, LuckyKing, MemberJoin, MemberLeave, Mute, Nudge, Ready, Recall,
};
pub mod plugin;
pub use plugin::{
    registered_plugins, registered_triggers_resolved, Category, PluginMeta, PluginSpec, SwitchKey, TriggerKind,
    TriggerMeta,
};
pub mod cooldown;
pub mod ctx;
pub mod dispatch;
pub mod enabled;
pub mod extract;
pub mod framesource;
pub mod handler;
pub mod impl_info;
pub mod invoker;
pub mod matcher;
pub mod middleware;
pub mod ratelimit;
pub mod reconnect;
pub mod refs;
pub mod registry;
pub mod rendezvous;
pub mod router;
pub mod rule;
pub mod service;
pub mod session;
pub mod slots;
pub mod source;
pub mod sse;
pub mod wire;

pub use args::{ArgError, ArgKind, ArgSpec, ArgToken, Args, ArgsMeta, FromArg, ParseArgs};
pub use bot::{add_outgoing_logger, set_outgoing_logger, Bot};
pub use impl_info::ImplInfo;
pub use nagisa_types::vendor::Vendor;
pub use source::EventSource;

/// 给 adapter 作者的底层管线：协议 adapter 要实现的出站动作 trait。业务代码从不命名它们
/// （业务调 `Bot` 的固有方法）；只有写新协议 adapter 的 crate 才用得到。归到这里、不放在扁平
/// 根上，让经过筛选的根导出聚焦在 handler 真正会写的名字上。
pub mod adapter {
    pub use crate::invoker::{ActionInvoker, Actions, MilkyActions, OneBotActions};
}

/// 引擎内部管线：分发引擎赖以构建的、对象安全的 handler/rule/waiter 机件。作者从不直接命名
/// 它们（作者写 `async fn` handler、`Rule` 组合子、`Session`/`Waiter` 调用）；放在这里、不上
/// 扁平根，留给极少数搭建引擎级工具的场景。
pub mod engine {
    pub use crate::ctx::StateMap;
    pub use crate::handler::ErasedHandler;
    pub use crate::rule::Check;
    pub use crate::session::{Selector, WaiterBuilder};
}

pub use cooldown::{Cd, CdKey, Cooldown, CooldownScope, CooldownStore, TriggerId};
pub use ctx::{Ctx, DevMode};
pub use dispatch::run_dispatch;
pub use enabled::{EnabledOverrides, EnabledSet};
pub use extract::{
    ArgText, At, Captures, Command, CommandArg, EventPeer, Extracted, FromContext, GroupMessage, Image, PrivateMessage,
    Reject, Reply, ReplyMsg, Sender, State, ToMe,
};
pub use handler::{Handler, HandlerOutcome, HandlerResult, IntoHandlerResult};
pub use matcher::{regex_escape, CommandUsage, Flank, Matcher, ParsedCommand, SlotSpec};
pub use middleware::{Flow, Middleware, Next};
pub use nagisa_types::event::{Meta, Notice, Request};
pub use ratelimit::{RateLimit, RateLimitScope};
pub use refs::{FriendRef, GroupRef, MemberRef};
pub use registry::{collect_into, registered_triggers, TriggerSpec};
pub use rendezvous::{Rendezvous, RendezvousSnapshot};
pub use router::Router;
pub use rule::{
    awake, awake_silent, from_user, group_admin, group_only, group_owner, in_group, keyword, private, replying,
    superuser, switch, to_me, GateReply, KillSwitch, Rule, SleepState, Superusers,
};
pub use service::{Service, ServiceBus, Supervisor};
pub use session::{
    Delivery, FlightGuard, FlightStore, Replied, Scope, Session, WaitFlow, Waiter, WaiterDepth, WaiterStore,
};
pub use slots::{FromSlots, FromTailText, NamedCaptures, SlotValue, Slots, Tail};

/// re-export `inventory`，使 `#[command]` 生成的代码可调用
/// `::nagisa_core::inventory::submit!`。业务/插件 crate 无需直接依赖 `inventory`。
pub use inventory;

/// re-export `async_trait`：实现自定义 [`Service`]/[`ActionInvoker`](adapter::ActionInvoker) 的作者需要
/// `#[nagisa::async_trait]`（或 `#[nagisa_core::async_trait]`）来标注其 `impl`，
/// 否则会撞上晦涩的 `E0195`（异步 trait 方法的生命周期参数不匹配）。
pub use async_trait::async_trait;

/// 关停信号：所有长生命周期任务 `select!` 它来收束退出。
pub use tokio_util::sync::CancellationToken as ShutdownToken;
