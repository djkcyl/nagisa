//! 命令/事件启用/禁用状态（让 `TriggerMeta`/`PluginMeta` 的 `can_disable`/`default_enable`
//! 在运行期生效;分层:插件总开关 + 触发器子开关,总闸 OFF ⇒ 内部全部 OFF）。
//!
//! 提供 `enable()`/`disable()` 与按会话(群)开关。一条命令对某会话默认
//! 启用与否取决于其 `default_enable`；可被全局覆盖或按 `Peer` 覆盖。`can_disable=false`
//! 的命令永远启用（在 dispatch 处直接短路，不查本表）。
//!
//! 运行期可变：内部 `RwLock`。通常作为共享状态注入（`App::new` 自动注册一个空表），
//! 业务侧用 `State<EnabledSet>` 取句柄做 `/enable`、`/disable` 之类管理命令。
use crate::plugin::SwitchKey;
use nagisa_types::id::Peer;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Mutex, RwLock};

#[derive(Default)]
struct Inner {
    /// 命令名 → 全局启用覆盖。
    global: HashMap<String, bool>,
    /// (命令名, 会话) → 启用覆盖（优先于全局）。
    per_peer: HashMap<(String, Peer), bool>,
}

/// 所有覆盖的可序列化快照。`per_peer` 用扁平列表（绕开「map 键非字符串」的序列化难题）。
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct EnabledOverrides {
    pub global: HashMap<String, bool>,
    pub per_peer: Vec<(String, Peer, bool)>,
}

type ChangeFn = dyn Fn(&str, Option<Peer>, bool) + Send + Sync;

/// 命令启用状态表。廉价共享（内部锁），运行期可改。
pub struct EnabledSet {
    inner: RwLock<Inner>,
    on_change: Mutex<Option<Box<ChangeFn>>>,
}

impl EnabledSet {
    pub fn new() -> Self {
        Self { inner: RwLock::new(Inner::default()), on_change: Mutex::new(None) }
    }

    /// 设置某命令的启用状态：`peer=None` 为全局覆盖，`Some(p)` 为按会话覆盖。
    /// 在释放写锁后触发 `on_change` 回调。
    ///
    /// 同时接受纯字符串键与类型化的 [`SwitchKey`]（`#[command]`/`#[event]` 宏产出的 `<FN>_KEY`
    /// 常量），故触发器键无需手敲字符串。
    pub fn set(&self, key: impl Into<SwitchKey>, peer: Option<Peer>, enabled: bool) {
        let name = key.into().resolve();
        {
            let mut g = self.inner.write().unwrap_or_else(|e| e.into_inner());
            match peer {
                None => {
                    g.global.insert(name.to_string(), enabled);
                }
                Some(p) => {
                    g.per_peer.insert((name.to_string(), p), enabled);
                }
            }
        }
        self.fire(name, peer, enabled);
    }

    /// 清除某命令的覆盖，回到 `default_enable`：`peer=None` 清全局，`Some` 清该会话。
    pub fn reset(&self, key: impl Into<SwitchKey>, peer: Option<Peer>) {
        let name = key.into().resolve();
        let mut g = self.inner.write().unwrap_or_else(|e| e.into_inner());
        match peer {
            None => {
                g.global.remove(name);
            }
            Some(p) => {
                g.per_peer.remove(&(name.to_string(), p));
            }
        }
    }

    /// 在某作用域解析一个键：按会话覆盖 > 全局覆盖 > `default`。
    fn key_on(&self, key: &str, default: bool, peer: Option<Peer>) -> bool {
        let g = self.inner.read().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = peer {
            if let Some(v) = g.per_peer.get(&(key.to_string(), p)) {
                return *v;
            }
        }
        if let Some(v) = g.global.get(key) {
            return *v;
        }
        default
    }

    /// 分层的触发器判定：
    /// 1. 触发器 `can_disable=false` → 恒开（豁免，无视总开关）；
    /// 2. 插件总开关 OFF（且插件 can_disable）→ 关（一刀压平）；
    /// 3. 否则看触发器自身的开关。
    #[allow(clippy::too_many_arguments)]
    pub fn is_enabled_keyed(
        &self,
        plugin_key: &str,
        trigger_key: &str,
        plugin_default: bool,
        plugin_can_disable: bool,
        trigger_default: bool,
        trigger_can_disable: bool,
        peer: Option<Peer>,
    ) -> bool {
        if !trigger_can_disable {
            return true;
        }
        if plugin_can_disable && !self.key_on(plugin_key, plugin_default, peer) {
            return false;
        }
        self.key_on(trigger_key, trigger_default, peer)
    }

    /// 单键查询（无插件层适用处用它）。
    pub fn is_enabled(&self, name: &str, default_enable: bool, peer: Option<Peer>) -> bool {
        self.key_on(name, default_enable, peer)
    }

    /// 注册一个在每次 `set` 调用后（锁释放后）触发的回调。只能注册一个，再次调用替换前一个。
    /// `reset` 不触发回调（它是本地回退，不是需持久化的变更）。
    pub fn on_change<F>(&self, f: F)
    where
        F: Fn(&str, Option<Peer>, bool) + Send + Sync + 'static,
    {
        *self.on_change.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(f));
    }

    fn fire(&self, key: &str, peer: Option<Peer>, enabled: bool) {
        if let Some(cb) = self.on_change.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            cb(key, peer, enabled);
        }
    }

    /// 把当前所有覆盖快照成一个可序列化结构。
    pub fn snapshot(&self) -> EnabledOverrides {
        let g = self.inner.read().unwrap_or_else(|e| e.into_inner());
        EnabledOverrides {
            global: g.global.clone(),
            per_peer: g.per_peer.iter().map(|((k, p), v)| (k.clone(), *p, *v)).collect(),
        }
    }

    /// 从先前快照的结构恢复覆盖（替换当前状态）。
    pub fn restore(&self, ov: EnabledOverrides) {
        let mut g = self.inner.write().unwrap_or_else(|e| e.into_inner());
        g.global = ov.global;
        g.per_peer = ov.per_peer.into_iter().map(|(k, p, v)| ((k, p), v)).collect();
    }
}

impl Default for EnabledSet {
    fn default() -> Self {
        Self::new()
    }
}
