//! 可组合的事件门控 `Rule`（`&` / `|` / `!`）+ 常用规则/权限构造器。
//!
//! 门控代数:AND/OR/NOT(`&` / `|` / `!`)三个布尔组合子都是一等且全总(total)的——任意
//! `Rule`/`Permission` 的组合都合法,没有需要禁止的非法组合。
//! 一条 `Rule` 就是「对 `Ctx` 求值的异步谓词」；权限只是带名字的 `Rule` 构造器。
//!
//! 角色查询（`group_admin`/`group_owner`）会命中网络，结果 memo 进 `Ctx`，故
//! `group_admin() | group_owner()` 只发一次请求；查询失败时 **fail closed**（判否）。
use crate::ctx::Ctx;
use crate::plugin::SwitchKey;
use async_trait::async_trait;
use nagisa_types::entity::Role;
use nagisa_types::message::MessageExt;
use nagisa_types::prelude::*;
use nagisa_types::segment::Segment;
use std::any::TypeId;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 对 `Ctx` 求值的异步谓词——门控代数的原子。
#[async_trait]
pub trait Check: Send + Sync + 'static {
    async fn check(&self, ctx: &Ctx) -> bool;
}

/// 可组合门控。`Arc` 包裹，克隆廉价；用 `&`/`|`/`!` 组合。
#[derive(Clone)]
pub struct Rule(Arc<dyn Check>);

impl Rule {
    /// 由自定义 `Check` 构造。
    pub fn new<C: Check>(c: C) -> Self {
        Rule(Arc::new(c))
    }
    /// 由同步谓词构造（大多数规则只读事件字段，无需 async）。
    pub fn pred<F>(f: F) -> Self
    where
        F: Fn(&Ctx) -> bool + Send + Sync + 'static,
    {
        Rule::new(Pred(f))
    }
    /// 对一个事件求值。
    pub async fn eval(&self, ctx: &Ctx) -> bool {
        self.0.check(ctx).await
    }
}

struct Pred<F>(F);
#[async_trait]
impl<F> Check for Pred<F>
where
    F: Fn(&Ctx) -> bool + Send + Sync + 'static,
{
    async fn check(&self, ctx: &Ctx) -> bool {
        (self.0)(ctx)
    }
}

struct And(Rule, Rule);
#[async_trait]
impl Check for And {
    async fn check(&self, ctx: &Ctx) -> bool {
        // 短路：左假则不求右。
        self.0.eval(ctx).await && self.1.eval(ctx).await
    }
}

struct Or(Rule, Rule);
#[async_trait]
impl Check for Or {
    async fn check(&self, ctx: &Ctx) -> bool {
        self.0.eval(ctx).await || self.1.eval(ctx).await
    }
}

struct Not(Rule);
#[async_trait]
impl Check for Not {
    async fn check(&self, ctx: &Ctx) -> bool {
        !self.0.eval(ctx).await
    }
}

impl std::ops::BitAnd for Rule {
    type Output = Rule;
    fn bitand(self, rhs: Rule) -> Rule {
        Rule::new(And(self, rhs))
    }
}
impl std::ops::BitOr for Rule {
    type Output = Rule;
    fn bitor(self, rhs: Rule) -> Rule {
        Rule::new(Or(self, rhs))
    }
}
impl std::ops::Not for Rule {
    type Output = Rule;
    fn not(self) -> Rule {
        Rule::new(Not(self))
    }
}

// ───────────────────────── 内置规则 ─────────────────────────

/// 消息是否「针对机器人」：私聊，或群里 @ 了机器人自身。
pub fn to_me() -> Rule {
    Rule::pred(|ctx| {
        let Some(m) = ctx.message() else { return false };
        if matches!(m.peer.scene, Scene::Friend | Scene::Temp) {
            return true;
        }
        m.content.mentions_user(ctx.bot().self_id())
    })
}

/// 仅群消息。
pub fn group_only() -> Rule {
    Rule::pred(|ctx| {
        ctx.message().map(|m| m.peer.scene == Scene::Group).unwrap_or(false)
    })
}

/// 仅私聊（好友/临时）消息。
pub fn private() -> Rule {
    Rule::pred(|ctx| {
        ctx.message()
            .map(|m| matches!(m.peer.scene, Scene::Friend | Scene::Temp))
            .unwrap_or(false)
    })
}

/// 限定群。
pub fn in_group(group: impl Into<Uin>) -> Rule {
    let g = group.into();
    Rule::pred(move |ctx| {
        ctx.message()
            .map(|m| m.peer.scene == Scene::Group && m.peer.id == g)
            .unwrap_or(false)
    })
}

/// 限定发送者。
pub fn from_user(user: impl Into<Uin>) -> Rule {
    let u = user.into();
    Rule::pred(move |ctx| ctx.message().map(|m| m.sender == u).unwrap_or(false))
}

/// 文本中包含任一关键词。
pub fn keyword<I, S>(words: I) -> Rule
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let set: Vec<String> = words.into_iter().map(Into::into).collect();
    Rule::pred(move |ctx| {
        let Some(m) = ctx.message() else { return false };
        let text = m.content.extract_text();
        set.iter().any(|w| text.contains(w.as_str()))
    })
}

// ───────────────────────── 权限规则 ─────────────────────────

/// 配置的超级用户集合。`App::superusers` 写入，`superuser()` 规则读取。
#[derive(Clone, Debug, Default)]
pub struct Superusers(pub HashSet<Uin>);

/// 发送者是配置的超级用户。
pub fn superuser() -> Rule {
    Rule::pred(|ctx| {
        let Some(m) = ctx.message() else { return false };
        ctx.state()
            .get(&TypeId::of::<Superusers>())
            .and_then(|a| a.downcast_ref::<Superusers>())
            .map(|s| s.0.contains(&m.sender))
            .unwrap_or(false)
    })
}

/// memo 进 `Ctx` 的角色查询结果（避免多条角色规则重复请求）。
#[derive(Clone, Copy)]
struct CachedRole(Option<Role>);

/// 取发送者角色，**对端种类无关**：
/// - 事件已带 `member`（群消息、或临时会话从群派生）→ 直接用其 `role`，与场景无关；
/// - 群场景且无 `member` → 查一次 `get_group_member_info` 并 memo；
/// - 好友/临时场景且无 `member` → `None`（私聊无群角色可言；调用方据此 fail closed）。
///
/// 关键：好友场景**不**发 `get_group_member_info`——`peer.id` 是好友 QQ 号、非群号，
/// 拿它当群号查询是错的。故私聊里 `group_admin()` 静默判否（fail closed），鉴权交给
/// `superuser()`：`group_admin() | superuser()` 因此在群/私聊两个场景都成立，让好友/群
/// 两份重复 handler 收敛为一。结果 memo 进 `Ctx`，多条角色规则只解析一次。
async fn sender_role(ctx: &Ctx) -> Option<Role> {
    let m = ctx.message()?;
    if let Some(c) = ctx.get_ext::<CachedRole>() {
        return c.0;
    }
    let role = if let Some(member) = &m.member {
        // 事件自带成员信息：任何场景都直接采信（无需场景判定、无需网络）。
        Some(member.role)
    } else if m.peer.scene == Scene::Group {
        // 群场景但事件未带 member：查一次。
        match ctx.bot().get_group_member_info(m.peer.id, m.sender, false).await {
            Ok(info) => Some(info.role),
            Err(_) => None,
        }
    } else {
        // 好友/临时场景且无 member：无群角色可言（不查 friend id 当群号）。
        None
    };
    ctx.insert_ext(CachedRole(role));
    role
}

struct RoleAtLeast {
    owner_only: bool,
}
#[async_trait]
impl Check for RoleAtLeast {
    async fn check(&self, ctx: &Ctx) -> bool {
        match sender_role(ctx).await {
            Some(Role::Owner) => true,
            Some(Role::Admin) => !self.owner_only,
            _ => false, // fail closed
        }
    }
}

/// 发送者是群主或管理员。
pub fn group_admin() -> Rule {
    Rule::new(RoleAtLeast { owner_only: false })
}

/// 发送者是群主。
pub fn group_owner() -> Rule {
    Rule::new(RoleAtLeast { owner_only: true })
}

// ───────────────────────── reply-on-veto 通道 ─────────────────────────

/// 否决时记录的回复：门控 [`Rule`] 本身是纯布尔，无法回话；某些门控（如休眠门控的
/// 「Zzz」、required-slot 用法提示）需要否决**并且**回话。故引入一个**增量、可选**的
/// `Ctx`-ext 通道：[`replying`] 包装的门控在否决时把要发的消息 stash 进 [`GateReply`]，
/// router 在门控否决点（`router.rs` 的 gate-veto）取出并发送一次。纯布尔门控从不触碰它。
///
/// **首写者胜合约**：`GateReply` 是**单槽**。`&` 严格左→右短路，故按
/// 左→右阅读**第一个失败的 `replying` 叶子**最先跑 `Check`、最先经 [`insert_ext_if_absent`]
/// 落下回复；其右侧叶子永不求值。`insert_ext_if_absent` 把这点变显式且稳健——即便未来某
/// 组合子急切求值，最左失败 `replying` 叶子的回复也恒胜出，是**已声明的正确性合约**。
///
/// [`insert_ext_if_absent`]: crate::ctx::Ctx::insert_ext_if_absent
#[derive(Clone)]
pub struct GateReply(pub Vec<Segment>);

/// 把一条门控 [`Rule`] 包成「否决即回话」：`rule` 求值为 `false` 时，若尚无 [`GateReply`]
/// 落下（首写者胜），则 `ctx.insert_ext_if_absent(GateReply(on_veto()))`；随后照常返回
/// `false`（语义仍是否决/`Skip`，只是额外记下一条回复）。`rule` 通过则原样放行、不触碰通道。
///
/// `on_veto` 是闭包（惰性求值）：仅在真的否决时才构造回复段，放行路径零开销。
///
/// `awake()` 与 debug-group 门控都是 `replying(..)` 之上的具名构造器；
/// 其 `.silent()` 变体用裸 [`Rule`]（不记 `GateReply`，纯静默 `Skip`）。
pub fn replying<F>(rule: Rule, on_veto: F) -> Rule
where
    F: Fn() -> Vec<Segment> + Send + Sync + 'static,
{
    Rule::new(ReplyingCheck { rule, on_veto: Box::new(on_veto) })
}

struct ReplyingCheck {
    rule: Rule,
    on_veto: Box<dyn Fn() -> Vec<Segment> + Send + Sync + 'static>,
}

#[async_trait]
impl Check for ReplyingCheck {
    async fn check(&self, ctx: &Ctx) -> bool {
        if self.rule.eval(ctx).await {
            return true;
        }
        // 否决：首写者胜地记下回复（已有则保留先到者）。
        ctx.insert_ext_if_absent(GateReply((self.on_veto)()));
        false
    }
}

// ───────────────────────── switch / awake 叶子构造器 ─────────────────────────

/// 进程级**全局总闸**（kill-switch）：置位后 [`switch`] 一律否决。`App` 写入共享态，
/// `switch()` 叶子读取。与 [`EnabledSet`](crate::enabled::EnabledSet) 的按命令/按会话开关
/// 正交——这是「一键全关」的紧急闸。
#[derive(Default)]
pub struct KillSwitch(AtomicBool);

impl KillSwitch {
    /// 新建（默认未触发，机器人可响应）。
    pub fn new() -> Self {
        Self(AtomicBool::new(false))
    }
    /// 设置总闸：`true` = 全关。
    pub fn set(&self, killed: bool) {
        self.0.store(killed, Ordering::SeqCst);
    }
    /// 当前是否已全关。
    pub fn is_killed(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// 一个**额外具名**的跨插件开关（`Function.require("name")`）：读取
/// [`EnabledSet`](crate::enabled::EnabledSet) 的按会话覆盖（per-peer override，优先于全局），
/// 并叠加全局总闸 [`KillSwitch`]。
///
/// 注意：插件/触发器**自身**的 `<FN>_KEY` 开关已由 router 在分发时套用
/// （`command_enabled`）；`switch()` 只用于额外的、跨插件的具名开关 + 全局 kill-switch。
/// 默认启用（`default = true`）：未设过覆盖的具名开关视为开。
///
/// 缺 `EnabledSet`（理论上 `App::new` 总会注入）→ 退化为「只看 kill-switch」（fail-open）。
/// 缺 `KillSwitch` → 视为未触发（放行）。
pub fn switch(key: impl Into<SwitchKey>) -> Rule {
    let key = key.into();
    Rule::pred(move |ctx| {
        // 全局总闸优先：置位则一律否决。
        let killed = ctx
            .state()
            .get(&TypeId::of::<KillSwitch>())
            .and_then(|a| a.downcast_ref::<KillSwitch>())
            .map(|k| k.is_killed())
            .unwrap_or(false);
        if killed {
            return false;
        }
        // 具名开关：按会话覆盖 > 全局覆盖 > 默认 on。
        let peer = ctx.event().peer();
        match ctx
            .state()
            .get(&TypeId::of::<crate::enabled::EnabledSet>())
            .and_then(|a| a.downcast_ref::<crate::enabled::EnabledSet>())
        {
            Some(es) => es.is_enabled(key.resolve(), true, peer),
            None => true, // 无 EnabledSet → 只看 kill-switch（上面已放行）
        }
    })
}

/// 机器人的「休眠」标志：置位时机器人装睡。`App`/管理命令写入共享态，
/// [`awake`] 叶子读取。
#[derive(Default)]
pub struct SleepState(AtomicBool);

impl SleepState {
    /// 新建（默认清醒）。
    pub fn new() -> Self {
        Self(AtomicBool::new(false))
    }
    /// 入睡 / 醒来：`true` = 睡着（装睡，否决业务门控）。
    pub fn set_asleep(&self, asleep: bool) {
        self.0.store(asleep, Ordering::SeqCst);
    }
    /// 当前是否睡着。
    pub fn is_asleep(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// 读取 [`SleepState`]：睡着 → `false`（否决），清醒 → `true`（放行）。
/// 缺 [`SleepState`] → 视为清醒（fail-open，放行）。
fn awake_inner() -> Rule {
    Rule::pred(|ctx| {
        ctx.state()
            .get(&TypeId::of::<SleepState>())
            .and_then(|a| a.downcast_ref::<SleepState>())
            .map(|s| !s.is_asleep())
            .unwrap_or(true)
    })
}

/// 仅当机器人**清醒**时放行；睡着时否决**并**经 reply-on-veto 通道回贴一次「Zzz」。
/// 静默变体见 [`awake_silent`]。
pub fn awake() -> Rule {
    replying(awake_inner(), || vec![Segment::text("Zzz")])
}


/// [`awake`] 的静默变体：睡着时**纯静默否决**（不记 `GateReply`、不回话）。
pub fn awake_silent() -> Rule {
    awake_inner()
}
