//! 插件静态模型。一个文件/文件夹 = 一个插件；触发器挂到「其声明处
//! （`module_path!()`）是触发器所在模块路径之最长前缀」的那个插件。与触发器一样经
//! `inventory` 收集。

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

/// 插件分类。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Category { Core, User, Tool, Fun, Push, Admin }

/// 插件级元数据 + 总开关策略。全 `&'static`，运行期零开销。
#[derive(Clone, Debug, PartialEq)]
pub struct PluginMeta {
    pub key: &'static str,
    pub name: &'static str,
    pub category: Category,
    pub version: &'static str,
    pub description: &'static str,
    /// 给 help/菜单的详细用法（合并转发帮助卡里该插件那一节）。比 `description` 长：怎么用。
    /// 空串 ⇒ 无。
    pub usage: &'static str,
    /// 总开关是否可被关掉。`false` ⇒ 门控把每个触发器都视为豁免（恒开）——给「永不下线」
    /// 的核心插件用。
    pub can_disable: bool,
    /// 该插件默认是否启用（在任何覆盖之前）。
    pub default_enable: bool,
    /// 不在生成的菜单/帮助里显示。
    pub hidden: bool,
    /// 维护锁：一旦禁用就不许再启用。这是**切换时**的守卫（当 `meta.maintain` 时拒绝
    /// 重新启用），由 admin/`set` 命令层执行——**不**在分发门控处（把一个从未被禁的
    /// `maintain` 插件强行关掉并不符合本字段语义）。
    pub maintain: bool,
    pub module_path: &'static str,
}

impl PluginMeta {
    /// `plugin!{}` 展开用的字段默认值：隐式、恒存在、user 分类的插件。`key == ""` 是哨兵，
    /// 链接时由 `module_path` 的最后一段填入（链接前在内部解析，见 `resolve_plugin_for`）。
    pub const DEFAULT: PluginMeta = PluginMeta {
        key: "",
        name: "",
        category: Category::User,
        version: "",
        description: "",
        usage: "",
        can_disable: true,
        default_enable: true,
        hidden: false,
        maintain: false,
        module_path: "",
    };
}

/// 触发器是靠命令匹配触发还是靠事件触发。
#[derive(Clone, Copy, Debug)]
pub enum TriggerKind { Command, Event(crate::event_trigger::EventKind) }

/// 触发器级元数据（命令或事件）。`key == "<plugin_key>.<id>"`。
#[derive(Clone, Debug)]
pub struct TriggerMeta {
    pub id: &'static str,
    pub key: &'static str,
    pub plugin_key: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// 详细用法（help 命令详情里展开这条命令时显示）。比 `description` 长：怎么用。空串 ⇒ 无。
    pub usage: &'static str,
    /// 命令的字面调用词：首个为主命令词、其余为别名。正则/槽位匹配器与事件触发器为空切片。
    pub words: &'static [&'static str],
    /// 命令参数规格（`#[derive(Args)]` 经 `args: Args<T>` 形参带入），供 help 自动生成用法。
    /// 无 `Args<T>` 形参的命令、以及事件触发器为空切片。
    pub args: &'static [crate::args::ArgSpec],
    /// help 里同插件命令的展示次序（`#[command(order = N)]`，小在前；缺省 0、并列保持注册序）。
    pub order: i32,
    pub can_disable: bool,
    pub default_enable: bool,
    pub hidden: bool,
    pub kind: TriggerKind,
    pub module_path: &'static str,
}

/// 一条 inventory 收集到的插件声明（由 `plugin!{}` 产出）。
pub struct PluginSpec { pub meta: PluginMeta }
inventory::collect!(PluginSpec);

/// 取 `module_path` 是 `trigger_path` 之最长前缀的那个插件。
pub fn link_trigger<'a>(trigger_path: &str, plugins: &'a [PluginMeta]) -> Option<&'a PluginMeta> {
    plugins
        .iter()
        .filter(|p| is_module_prefix(p.module_path, trigger_path))
        .max_by_key(|p| p.module_path.len())
}

/// `prefix` 等于 `path`、或是它的祖先模块时为真
/// （以 `::` 为边界，故 `a::horse` 不是 `a::horseshoe` 的前缀）。
fn is_module_prefix(prefix: &str, path: &str) -> bool {
    path == prefix
        || (path.starts_with(prefix) && path[prefix.len()..].starts_with("::"))
}

/// 全部已声明的插件（inventory 的克隆）。顺序不稳定。
pub fn registered_plugins() -> Vec<PluginMeta> {
    inventory::iter::<PluginSpec>.into_iter().map(|s| s.meta.clone()).collect()
}

/// 泄漏缓存：把运行期构造的字符串（`"plugin.id"` 键、默认插件键）去重并固化成稳定的
/// `&'static str`。插件/触发器有限且启动时构造一次，故泄漏有界。模块私有。
fn intern(s: &str) -> &'static str {
    static CACHE: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashSet::new()));
    let mut guard = cache.lock().expect("intern cache mutex poisoned");
    if let Some(&existing) = guard.get(s) {
        return existing;
    }
    let leaked: &'static str = Box::leak(s.to_string().into_boxed_str());
    guard.insert(leaked);
    leaked
}

/// 模块路径里以 `::` 分隔的最后一段（隐式插件键的兜底）。
fn last_segment(module_path: &str) -> &str {
    module_path.rsplit("::").next().unwrap_or(module_path)
}

/// 同 `registered_plugins()`，但把每个空 `key` 用模块路径的最后一段填上（intern 固化）。
/// 链接时用它，使未显式给 `key` 的插件仍能解析到一个稳定标识。
fn registered_plugins_resolved() -> Vec<PluginMeta> {
    registered_plugins()
        .into_iter()
        .map(|mut p| {
            if p.key.is_empty() {
                p.key = intern(last_segment(p.module_path));
            }
            p
        })
        .collect()
}

/// 解析管辖 `module_path` 处某触发器的插件 `(key, default_enable, can_disable)`。
/// 兜底为一个以模块路径本身为键的隐式、恒存在插件。
pub fn resolve_plugin_for(module_path: &'static str) -> (&'static str, bool, bool) {
    let plugins = registered_plugins_resolved();
    match link_trigger(module_path, &plugins) {
        Some(p) => (p.key, p.default_enable, p.can_disable),
        None => (intern(module_path), true, true),
    }
}

/// 构造一个泄漏的 `"plugin.id"` 触发器键。
pub fn trigger_key(plugin_key: &str, id: &str) -> &'static str {
    intern(&format!("{plugin_key}.{id}"))
}

/// 开关键的类型化句柄，[`EnabledSet::set`](crate::EnabledSet::set)/`reset` 接受它。
///
/// `#[command]`/`#[event]` 宏给每个触发器产出一个 `pub const <FN>_KEY: SwitchKey`，故 admin
/// 代码用 `es.set(my_cmd_KEY, ..)` 翻开关，而不必手敲会因笔误/改名而悄悄失效的 `"plugin.id"`
/// 字符串。插件总开关（或任意字面量）仍可经 `&str`/`String` 的 `From` impl 传入。
///
/// 设计说明：点分的 `"<plugin_key>.<id>"` 在宏展开时无从得知（插件是链接期从 `module_path`
/// 解析的），故 `Trigger` 携带原始 `module_path`+`id`、惰性解析——使它保持为一个廉价、不会失败
/// 的句柄。
#[derive(Clone, Copy, Debug)]
pub enum SwitchKey {
    /// 一个已解析好的字面量键：插件总开关，或任意手传的字符串。
    Literal(&'static str),
    /// 在 `module_path` 处声明、`id` 为标识的触发器；解析为 `"<plugin_key>.<id>"`。
    Trigger { module_path: &'static str, id: &'static str },
}

impl SwitchKey {
    /// 构造一个触发器句柄（宏用；可 `const` 调用）。
    pub const fn trigger(module_path: &'static str, id: &'static str) -> Self {
        SwitchKey::Trigger { module_path, id }
    }

    /// 解析成门控据以存储的点分开关键字符串。
    pub fn resolve(&self) -> &'static str {
        match *self {
            SwitchKey::Literal(s) => s,
            SwitchKey::Trigger { module_path, id } => {
                let (plugin_key, _, _) = resolve_plugin_for(module_path);
                trigger_key(plugin_key, id)
            }
        }
    }
}

// 接受任意 `&str`（静态或运行期，如用户输入）。运行期字符串被 intern 成稳定的 `&'static str`；
// intern 缓存去重，故泄漏有界。
impl From<&str> for SwitchKey {
    fn from(s: &str) -> Self {
        SwitchKey::Literal(intern(s))
    }
}

impl From<String> for SwitchKey {
    fn from(s: String) -> Self {
        SwitchKey::Literal(intern(&s))
    }
}

/// `plugin_key`/`key` 已解析好的触发器（宏产出时它们为空）。
pub fn registered_triggers_resolved() -> Vec<TriggerMeta> {
    let plugins = registered_plugins_resolved();
    crate::registry::registered_triggers()
        .into_iter()
        .map(|mut t| {
            let pk = match link_trigger(t.module_path, &plugins) {
                Some(p) => p.key,
                None => intern(t.module_path),
            };
            t.plugin_key = pk;
            t.key = trigger_key(pk, t.id);
            t
        })
        .collect()
}
