//! 插件注册表：基于 [`inventory`] 的编译期收集 + 插件元数据。
//!
//! `#[command(...)]` 宏在保留原 `async fn` 与兄弟注册函数 `<fn>__nagisa_register`
//! 之外，额外 `inventory::submit!` 一个 [`TriggerSpec`]：把「元数据 + 挂载函数」
//! 登记到一个进程级集合里。启动时 [`collect_into`] 把所有 spec 的 `register`
//! 折叠到一个 [`Router`] 上，[`registered_triggers`] 则供 help/菜单插件列举命令。
//!
//! # DCE（死代码消除）注意事项
//!
//! `inventory` 依赖链接器把分散在各 crate 的静态注册项收集到一起。**若某插件
//! crate 从未被消费方 `use`（即整个 crate 对 bin 而言是死代码），链接器可能把它
//! 连同其 `inventory::submit!` 项一起剔除**——于是这些命令在运行期凭空消失，且
//! 不会有任何报错。
//!
//! 防御办法：**bin/app 必须既依赖插件 crate、又在源码里真实引用它**，让链接器
//! 保留该编译单元。常见两种写法：
//!
//! ```ignore
//! // 写法一：在 main.rs 顶部 glob-use 插件 crate（哪怕没用到任何名字）。
//! use my_plugins::*;
//!
//! // 写法二：插件 crate 暴露一个空的 `force_link()`，bin 显式调用一次。
//! pub fn force_link() {}            // 在插件 crate 里
//! my_plugins::force_link();          // 在 bin 里
//! ```
//!
//! 同一编译单元内（如本 crate 的集成测试）不受此影响——只要测试里引用了被
//! `#[command]` 标注的函数（例如调用其 `<fn>__nagisa_register`），该单元就会被保留。
use crate::plugin::TriggerMeta;
use crate::router::Router;

/// 一条 inventory 收集到的触发器：元数据 + 一个挂载 handler 的函数。
pub struct TriggerSpec {
    pub meta: TriggerMeta,
    pub register: fn(Router) -> Router,
}
inventory::collect!(TriggerSpec);

/// 把每条收集到的 `TriggerSpec` 折叠到 `router` 上（顺序不稳定；先后由各触发器自身的
/// `priority` 决定，而非折叠顺序）。见上文 DCE 注意事项。
pub fn collect_into(mut router: Router) -> Router {
    for spec in inventory::iter::<TriggerSpec> {
        router = (spec.register)(router);
    }
    router
}

/// 全部已注册触发器的元数据（克隆）。供 help/菜单与插件分组使用。
pub fn registered_triggers() -> Vec<TriggerMeta> {
    inventory::iter::<TriggerSpec>.into_iter().map(|s| s.meta.clone()).collect()
}
