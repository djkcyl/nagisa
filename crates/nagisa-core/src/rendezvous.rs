//! 跨会话的 rendezvous：一个通用、带 TTL 的「留 token、稍后取」存储。
//! 第一条事件 [`issue`](Rendezvous::issue) 一个 `key → value`；一条独立的后续事件按 `key`
//! [`claim`](Rendezvous::claim) 它。这是第二种跨会话模型（第一种是挂起的
//! [`Waiter`](crate::session::Waiter)）：它跨**两条独立事件**，二者可能相距很远、在不同会话——
//! 正是 `token`/`bind` 流程要的（在群里贴个 token，私聊把 token 发来绑定）。
//!
//! 内存存储,每条带 TTL:`claim` 取走时若已过期则丢弃并返回 `None`,`peek` 把过期项当
//! `None`(但不从表里删),整表回收要显式 `sweep`。持久化经
//! [`snapshot`](Rendezvous::snapshot)/[`restore`](Rendezvous::restore)/
//! [`on_change`](Rendezvous::on_change) 交给业务层（与 [`EnabledSet`](crate::enabled::EnabledSet)
//! 同机制）。作为共享态注入，经 `State<Rendezvous<K, V>>` 取用。
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

/// `on_change` 回调类型：`(key, present)`，issue 时 `present == true`、claim/过期/移除时 `false`
/// ——使持久化层能镜像本存储。
type ChangeFn<K> = dyn Fn(&K, bool) + Send + Sync;

/// 带 TTL 的 rendezvous 存储。廉价共享（内部 `RwLock`）；运行期可改。
pub struct Rendezvous<K, V> {
    inner: RwLock<HashMap<K, (V, Instant)>>,
    ttl: Duration,
    on_change: Mutex<Option<Box<ChangeFn<K>>>>,
}

/// 自动提供的存储的默认每条 TTL（5 分钟——够「这边贴 token、那边兑换」流程用；
/// 与 token/bind 示例一致）。
pub const DEFAULT_TTL: Duration = Duration::from_secs(300);

/// 自动配置的默认值（与 `EnabledSet`/`WaiterStore` 同机制）：
/// [`App::new`](../../nagisa/struct.App.html) 注入一个 `Rendezvous<K, V>`，故 `State<Rendezvous<K, V>>`
/// 提取器无需手动 `.data(..)` 即可工作。用 [`DEFAULT_TTL`]；想改就在 run 前注册自己的存储。
impl<K: Eq + Hash + Clone, V> Default for Rendezvous<K, V> {
    fn default() -> Self {
        Self::new(DEFAULT_TTL)
    }
}

/// 可序列化快照。每条带其**剩余**寿命（毫秒，而非不可移植的绝对 `Instant`）；
/// `restore` 把截止时刻重锚到 `now + remaining`。
#[derive(Clone, Serialize, Deserialize)]
pub struct RendezvousSnapshot<K, V> {
    pub entries: Vec<(K, V, u64)>,
}

impl<K: Eq + Hash + Clone, V> Rendezvous<K, V> {
    /// 新建存储，每条默认 TTL 为 `ttl`。
    pub fn new(ttl: Duration) -> Self {
        Self { inner: RwLock::new(HashMap::new()), ttl, on_change: Mutex::new(None) }
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<K, (V, Instant)>> {
        self.inner.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<K, (V, Instant)>> {
        self.inner.write().unwrap_or_else(|e| e.into_inner())
    }

    /// 注册一个在每次 issue/移除时触发的回调（把变更持久化到你的存储）。
    pub fn on_change<F>(&self, f: F)
    where
        F: Fn(&K, bool) + Send + Sync + 'static,
    {
        *self.on_change.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(f));
    }

    /// 触发回调（在写锁释放之后调用——绝不在锁内）。
    fn fire(&self, key: &K, present: bool) {
        if let Some(cb) = self.on_change.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            cb(key, present);
        }
    }

    /// 以默认 TTL issue 一个 `key → value`。
    pub fn issue(&self, key: K, value: V) {
        self.issue_with_ttl(key, value, self.ttl);
    }

    /// 以指定 `ttl` issue 一个 `key → value`。
    pub fn issue_with_ttl(&self, key: K, value: V, ttl: Duration) {
        let deadline = Instant::now() + ttl;
        {
            self.write().insert(key.clone(), (value, deadline));
        }
        self.fire(&key, true);
    }

    /// 取走并移除 `key` 的值（若存在且未过期）。过期条目被丢弃（并经 `on_change` 上报），返回 `None`。
    pub fn claim(&self, key: &K) -> Option<V> {
        let now = Instant::now();
        let taken = {
            let mut g = self.write();
            g.remove(key)
        };
        match taken {
            Some((v, deadline)) => {
                self.fire(key, false);
                if now < deadline {
                    Some(v)
                } else {
                    None // 已过期
                }
            }
            None => None,
        }
    }

    /// 移除所有过期条目,对每个被移除的键触发 `on_change`。只有显式调用 `sweep` 才会全表扫描:
    /// `claim`/`peek` 都**不**触发它——`claim` 只移除自己请求的那个键(无论是否过期),`peek` 只读、
    /// 把过期项当 `None` 返回但留在表里。故未被 `claim` 单独清掉的残留过期条目要靠定期 `sweep` 回收。
    pub fn sweep(&self) {
        let now = Instant::now();
        let expired: Vec<K> = {
            let g = self.read();
            g.iter().filter(|(_, (_, d))| now >= *d).map(|(k, _)| k.clone()).collect()
        };
        if !expired.is_empty() {
            let mut g = self.write();
            for k in &expired {
                g.remove(k);
            }
            drop(g);
            for k in &expired {
                self.fire(k, false);
            }
        }
    }

    /// 条目数（含尚未清扫的过期条目）。
    pub fn len(&self) -> usize {
        self.read().len()
    }

    /// 存储当前是否没有条目。
    pub fn is_empty(&self) -> bool {
        self.read().is_empty()
    }
}

impl<K: Eq + Hash + Clone, V: Clone> Rendezvous<K, V> {
    /// 读取（不移除）`key` 的值（若存在且未过期）。
    pub fn peek(&self, key: &K) -> Option<V> {
        let now = Instant::now();
        let g = self.read();
        match g.get(key) {
            Some((v, deadline)) if now < *deadline => Some(v.clone()),
            _ => None,
        }
    }

    /// 快照存活（未过期）条目及其剩余寿命（ms）。
    pub fn snapshot(&self) -> RendezvousSnapshot<K, V> {
        let now = Instant::now();
        let g = self.read();
        let entries = g
            .iter()
            .filter_map(|(k, (v, deadline))| {
                let remaining = deadline.saturating_duration_since(now);
                if remaining.is_zero() {
                    None
                } else {
                    Some((k.clone(), v.clone(), remaining.as_millis() as u64))
                }
            })
            .collect();
        RendezvousSnapshot { entries }
    }

    /// 用快照替换整个存储，把截止时刻重锚到 `now + remaining`。
    pub fn restore(&self, snap: RendezvousSnapshot<K, V>) {
        let now = Instant::now();
        let mut g = self.write();
        g.clear();
        for (k, v, remaining_ms) in snap.entries {
            g.insert(k, (v, now + Duration::from_millis(remaining_ms)));
        }
    }
}
