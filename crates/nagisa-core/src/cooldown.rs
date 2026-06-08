//! 冷却（cooldown）门控——默认语义：**按人全局**（`UserGlobal`），
//! 以 `(count, first_ts)` **滑动窗口**表达 `max_exec` 的「N 次后再冷却」，并提供
//! `bypass`（如 `superuser()` 越权）与命令式 DI 句柄 [`Cd`]。
//!
//! ## 为什么默认是 `UserGlobal`
//!
//! 默认存储是一个**进程级、跨全部触发器**、以 `sender.id` 为键的桶映射——同一个人在
//! 任意命令上的调用共享同一个冷却桶。故默认 key **只看发送者 id**（不含触发器）；
//! `per_trigger`/`per_peer`/`Global` 是显式 opt-in。
//!
//! ## 窗口 + 「到点才盖章」(依赖门控链 `&` 的左→右短路定序)
//!
//! 存的不是单个 `Instant`，而是窗口 `(count, first_ts)`：窗口内允许 `max_exec` 次，
//! 满了才否决；窗口过期则重置。冷却 [`Check`] 作为门控链**最右**叶子被 `&` 进去，
//! 凭 `&` 严格左→右短路**仅当左侧全部门控通过时才求值**——故被权限/开关挡下的
//! 尝试**不会**消耗冷却（永不在被拒尝试上盖章）。
//!
//! 这是唯一一个**有状态**的 `Check`：状态在 [`CooldownStore`]（`App::new` 注入共享态，
//! 与 `WaiterStore`/`Rendezvous` 同机制），命令式路径经 [`Cd`] 复用同一存储。
use crate::ctx::Ctx;
use crate::extract::{Extracted, FromContext, Reject, State};
use crate::rule::{Check, Rule};
use async_trait::async_trait;
use nagisa_types::id::{Peer, Uin};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 冷却键的作用域。`UserGlobal` 是**默认**（key 仅 `sender.id`，跨全部触发器）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CooldownScope {
    /// 默认：按人**全局**——key 仅为 `sender.id`，跨**所有**触发器共享一个桶。
    UserGlobal,
    /// 按人**且**按触发器：同一个人在不同命令上各有独立桶。
    User,
    /// 按对端会话（群/好友）：同一会话共享一个桶（与触发器无关）。
    Peer,
    /// 全局单桶：所有人、所有触发器共享。
    Global,
}

/// 一个触发器的稳定标识（`plugin_key` + `trigger_key`）。用于 `User`/per-trigger 作用域的键。
///
/// 声明式挂载经 `TriggerId::of(plugin_key, trigger_key)` 构造（`#[command(cooldown=…)]` 在
/// lowering 时填好）；命令式 `Cd` 路径直接用 [`CdKey`]（自定义字符串键），无需 `TriggerId`。
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct TriggerId {
    plugin: String,
    trigger: String,
}

impl TriggerId {
    /// 由插件键 + 触发器键构造（宏在 lowering 时调用）。
    pub fn of(plugin: impl Into<String>, trigger: impl Into<String>) -> Self {
        Self { plugin: plugin.into(), trigger: trigger.into() }
    }
}

/// 冷却存储的实际键：把各作用域投影到一个可哈希的判别式。
///
/// `UserGlobal` 仅含 `sender.id`；`User` 含 `(sender, trigger)`；
/// `Peer` 含会话；`Global` 是单元。命令式 [`Cd`] 用 [`CdKey::Custom`] 自定义字符串键。
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum CdKey {
    /// 按人全局：仅发送者 id。
    UserGlobal(Uin),
    /// 按人且按触发器。
    User(Uin, TriggerId),
    /// 按对端会话。
    Peer(Peer),
    /// 全局单桶。
    Global,
    /// 命令式句柄的自定义键（数据派生，如 `format!("bili:{aid}")`）。
    Custom(String),
}

impl From<&str> for CdKey {
    fn from(s: &str) -> Self {
        CdKey::Custom(s.to_string())
    }
}
impl From<String> for CdKey {
    fn from(s: String) -> Self {
        CdKey::Custom(s)
    }
}

/// 一个键的滑动窗口：窗口内已发生 `count` 次，窗口自 `first` 起算。
#[derive(Clone, Copy, Debug)]
struct Window {
    count: u32,
    first: Instant,
}

/// 冷却存储：`key → Window` 的进程级共享表。`App::new` 注入共享态（与 `WaiterStore` 同机制），
/// 声明式 [`Cooldown`] 的 [`Check`] 与命令式 [`Cd`] 句柄共用它。
///
/// 用 `Mutex<HashMap>`（与 `RateLimit`/`Rendezvous` 一致的 std 内存存储，无新依赖）。
#[derive(Default)]
pub struct CooldownStore {
    map: Mutex<HashMap<CdKey, Window>>,
}

impl CooldownStore {
    /// 新建空存储。
    pub fn new() -> Self {
        Self::default()
    }

    /// 窗口核心：尝试在 `key` 上记一次（窗口 `interval`、上限 `max_exec`）。
    ///
    /// - 无窗口、或窗口已过 `interval` → 开新窗口（`count=1`），**放行并盖章**。
    /// - 窗口内 `count < max_exec` → `count += 1`，**放行并盖章**。
    /// - 窗口内 `count >= max_exec` → **否决**（不动窗口）。
    ///
    /// 返回 `true` 表示放行（已盖章），`false` 表示在冷却中被否决。
    fn try_stamp(&self, key: CdKey, interval: Duration, max_exec: u32) -> bool {
        let now = Instant::now();
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        match map.get_mut(&key) {
            Some(w) if now.duration_since(w.first) < interval => {
                if w.count < max_exec {
                    w.count += 1;
                    true
                } else {
                    false
                }
            }
            // 无窗口 or 窗口已过期 → 开新窗口。
            _ => {
                map.insert(key, Window { count: 1, first: now });
                true
            }
        }
    }

    /// 只读窥探：`key` 现在是否可放行（不盖章、不改状态）。
    fn ready(&self, key: &CdKey, interval: Duration, max_exec: u32) -> bool {
        let now = Instant::now();
        let map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        match map.get(key) {
            Some(w) if now.duration_since(w.first) < interval => w.count < max_exec,
            _ => true,
        }
    }

    /// 强制盖一次章（命令式「成功后再扣冷却」用）：窗口内自增，过期/缺失则开新窗口。
    fn stamp(&self, key: CdKey, interval: Duration) {
        let now = Instant::now();
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        match map.get_mut(&key) {
            Some(w) if now.duration_since(w.first) < interval => w.count += 1,
            _ => {
                map.insert(key, Window { count: 1, first: now });
            }
        }
    }
}

/// 声明式冷却门控的构建体。`new(secs)` 默认 `UserGlobal` / `max_exec=1` / 静默；
/// 以 `.per_peer()`/`.per_trigger()`/`.max_exec(n)`/`.bypass(rule)` 链式定制。
///
/// `into_rule(trigger)` 产出一条 [`Rule`]——挂载侧把它 `&` 进门控链**最右**，
/// 凭 `&` 左→右短路实现「权限在前、冷却在后，只在通过时盖章」。
#[derive(Clone)]
pub struct Cooldown {
    interval: Duration,
    scope: CooldownScope,
    max_exec: u32,
    bypass: Option<Rule>,
}

impl Cooldown {
    /// 新建一条冷却：默认 `UserGlobal`、`max_exec=1`、静默。
    pub fn new(secs: u64) -> Self {
        Self {
            interval: Duration::from_secs(secs),
            scope: CooldownScope::UserGlobal,
            max_exec: 1,
            bypass: None,
        }
    }

    /// 改为按对端会话（群/好友）。
    pub fn per_peer(mut self) -> Self {
        self.scope = CooldownScope::Peer;
        self
    }

    /// 改为按人且按触发器（同一个人在不同命令上各有独立桶）。
    pub fn per_trigger(mut self) -> Self {
        self.scope = CooldownScope::User;
        self
    }

    /// 改为全局单桶。
    pub fn global(mut self) -> Self {
        self.scope = CooldownScope::Global;
        self
    }

    /// 窗口内允许的执行次数（达到后进入冷却）。`n=0` 视为 `1`。
    pub fn max_exec(mut self, n: u32) -> Self {
        self.max_exec = n.max(1);
        self
    }

    /// 越权放行规则（如 `override_level=MASTER` ⇒ `.bypass(superuser())`）：
    /// 该规则通过时直接放行且**不盖章**。
    pub fn bypass(mut self, r: Rule) -> Self {
        self.bypass = Some(r);
        self
    }

    /// 物化为一条 [`Rule`]：挂载侧把它 `&` 进门控链最右。
    ///
    /// `trigger` 仅在 `User`（per-trigger）作用域下进入键；其余作用域忽略它。
    pub fn into_rule(self, trigger: TriggerId) -> Rule {
        Rule::new(CooldownCheck { cd: self, trigger })
    }
}

/// `cooldown = 30` 糖：`Cooldown::from(30u64)`。
impl From<u64> for Cooldown {
    fn from(secs: u64) -> Self {
        Cooldown::new(secs)
    }
}

/// 从当前事件 + 作用域算出冷却键。非消息事件（无 sender/peer）→ `None`（调用方据此放行：
/// 没有「人」可冷却的事件不被冷却挡住）。
fn key_for(ctx: &Ctx, scope: CooldownScope, trigger: &TriggerId) -> Option<CdKey> {
    // bot 自己回显的消息(is_self,如 OneBot `message_sent`)不参与冷却:否则 bot 一发言就给
    // Peer/Global/UserGlobal 盖上时间戳,可能把紧接着的真实用户挡在冷却外。
    if ctx.message().is_some_and(|m| m.is_self) {
        return None;
    }
    match scope {
        CooldownScope::UserGlobal => Some(CdKey::UserGlobal(ctx.message()?.sender)),
        CooldownScope::User => Some(CdKey::User(ctx.message()?.sender, trigger.clone())),
        CooldownScope::Peer => Some(CdKey::Peer(ctx.message()?.peer)),
        CooldownScope::Global => Some(CdKey::Global),
    }
}

/// 取共享 [`CooldownStore`]（缺失时——理论上 `App::new` 总会注入——保守 fail-open，放行）。
///
/// 故意**不**复用 [`State<CooldownStore>`] 提取器：`State` 在缺失时返回 `Reject::Error`
/// （把缺存储当成错误），而冷却门控要的是缺存储时**静默放行**（fail-open，与 `RateLimit`
/// 缺桶放行一致）。这里直接做 `TypeId` 取值 + downcast，刻意保留 `Option`（缺失 → `None`
/// → 放行），并避开 `State` 的 `async`。请勿“简化”回 `State`，否则会把缺存储变成错误。
fn store(ctx: &Ctx) -> Option<Arc<CooldownStore>> {
    use std::any::TypeId;
    ctx.state()
        .get(&TypeId::of::<CooldownStore>())
        .and_then(|a| Arc::clone(a).downcast::<CooldownStore>().ok())
}

/// 有状态的冷却 [`Check`]：先看 `bypass`，再窥探窗口并盖章；仅在窗口到点（满 `max_exec`）时否决。
struct CooldownCheck {
    cd: Cooldown,
    trigger: TriggerId,
}

#[async_trait]
impl Check for CooldownCheck {
    async fn check(&self, ctx: &Ctx) -> bool {
        // 越权：bypass 通过 → 直接放行，不盖章。
        if let Some(b) = &self.cd.bypass {
            if b.eval(ctx).await {
                return true;
            }
        }
        // 没有「人/会话」可冷却（非消息事件）→ 放行。
        let Some(key) = key_for(ctx, self.cd.scope, &self.trigger) else {
            return true;
        };
        // 存储缺失 → 保守放行（fail-open，与 RateLimit 缺桶放行一致）。
        let Some(store) = store(ctx) else {
            return true;
        };
        // 窗口内放行并盖章;到点则否决。
        store.try_stamp(key, self.cd.interval, self.cd.max_exec)
    }
}

/// 命令式 DI 冷却句柄：在 handler 体内对**数据派生**键做条件冷却。
///
/// 形如 `async fn h(cd: Cd, ..) { cd.gate(format!("bili:{aid}"), D60)?; reduce_gold().await?; .. }`。
/// 它就是 `State<CooldownStore>` 加几个方法，故零额外机制。
pub struct Cd(Arc<CooldownStore>);

#[async_trait]
impl FromContext for Cd {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        let State(store) = State::<CooldownStore>::from_context(ctx).await?;
        Ok(Cd(store))
    }
}

impl Cd {
    /// 检查并盖章：冷却中返回 `Err(Skip)`（handler 用 `?` 提前退出），否则盖章放行 `Ok(())`。
    /// `max_exec=1` 语义（一次一窗口）；需要 N 次请用声明式 [`Cooldown::max_exec`]。
    pub fn gate(&self, key: impl Into<CdKey>, dur: Duration) -> Extracted<()> {
        if self.0.try_stamp(key.into(), dur, 1) {
            Ok(())
        } else {
            Err(Reject::Skip)
        }
    }

    /// 只读窥探：`key` 此刻是否可放行（不盖章）。
    pub fn ready(&self, key: impl Into<CdKey>, dur: Duration) -> bool {
        self.0.ready(&key.into(), dur, 1)
    }

    /// 成功后再盖章（「先做事、成功了才扣冷却」）。
    pub fn stamp(&self, key: impl Into<CdKey>, dur: Duration) {
        self.0.stamp(key.into(), dur);
    }
}
