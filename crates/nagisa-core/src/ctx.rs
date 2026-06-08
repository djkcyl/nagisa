//! 每事件上下文 `Ctx`：提取器从它取料。
//!
//! 组合 `Arc<Event>` + `Bot` + 每事件 memo（`extensions`，缓存昂贵提取结果）
//! + app 共享只读状态（`state`）。
use crate::bot::Bot;
use nagisa_types::event::MessageEvent;
use nagisa_types::prelude::*;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// app 级共享状态表：`TypeId → Arc<dyn Any>`。`Router::data` 写入，`State<T>` 取出。
/// 启动期由 builder 填好，运行期只读（故 `Arc` 共享、无锁）。
pub type StateMap = HashMap<TypeId, Arc<dyn Any + Send + Sync>>;

/// dev/诊断模式标记，由 `App::debug()` 插入 [`StateMap`]。
///
/// 设计取舍：dev 标志走**共享状态表**（与 `EnabledSet`/`WaiterStore` 同机制）而非给
/// `Ctx` 加一个新字段——后者会改动 `Ctx::new` 的公有签名、波及每个构造点。
/// 走状态表则零构造点改动、零新依赖，[`Ctx::is_dev`] 一次 `TypeId` 查表即可。
///
/// 打开后，router 与 `Args<T>` 提取器在「静默跳过」处改发 `WARN` 解释**为何**跳过
/// （哪个开关关了 / 匹配器没中 / 哪个 `Args` 字段解析失败），并可选地把命令用法回贴用户。
pub struct DevMode;

/// 每事件上下文。廉价构造（`Arc` 克隆 + 一个空 memo map）。
pub struct Ctx {
    event: Arc<Event>,
    bot: Bot,
    /// 每事件 memo：提取器可缓存昂贵结果，同一事件内复用。`Mutex` 提供内部可变性。
    extensions: Mutex<HashMap<TypeId, Box<dyn Any + Send + Sync>>>,
    state: Arc<StateMap>,
}

impl Ctx {
    /// 由事件 + bot + 共享状态构造一个新上下文。memo 表初始为空。
    pub fn new(event: Arc<Event>, bot: Bot, state: Arc<StateMap>) -> Self {
        Self { event, bot, extensions: Mutex::new(HashMap::new()), state }
    }

    /// 本次事件。
    pub fn event(&self) -> &Arc<Event> {
        &self.event
    }

    /// 触发本次事件的 bot 句柄。
    pub fn bot(&self) -> &Bot {
        &self.bot
    }

    /// app 共享状态表。
    pub fn state(&self) -> &Arc<StateMap> {
        &self.state
    }

    /// 本次 dispatch 是否处于 dev/诊断模式（`App::debug()` 注入了 [`DevMode`] 标记）。
    /// router 与 `Args<T>` 提取器据此决定是否对「静默跳过」改发 `WARN` + 用法回贴。
    pub fn is_dev(&self) -> bool {
        self.state.contains_key(&TypeId::of::<crate::ctx::DevMode>())
    }

    /// 若本次事件是消息事件，返回内层 `MessageEvent`。
    pub fn message(&self) -> Option<&MessageEvent> {
        match &*self.event {
            Event::Message(m) => Some(m),
            _ => None,
        }
    }

    /// 从每事件 memo 取一份 `T` 的克隆（若先前 `insert_ext` 过）。
    /// mutex 中毒时取用 guard 内的数据，而非 panic（保持无 panic 合约）。
    pub fn get_ext<T: Clone + Send + Sync + 'static>(&self) -> Option<T> {
        let exts = self.extensions.lock().unwrap_or_else(|e| e.into_inner());
        exts.get(&TypeId::of::<T>()).and_then(|v| v.downcast_ref::<T>()).cloned()
    }

    /// 把 `T` 存进每事件 memo（同类型覆盖）。
    /// mutex 中毒时取用 guard 内的数据，而非 panic（保持无 panic 合约）。
    pub fn insert_ext<T: Send + Sync + 'static>(&self, value: T) {
        let mut exts = self.extensions.lock().unwrap_or_else(|e| e.into_inner());
        exts.insert(TypeId::of::<T>(), Box::new(value));
    }

    /// 仅在 `T` 尚未存在时写入（**首写者胜**）；已存在则保留原值、丢弃 `value`，返回 `false`。
    ///
    /// reply-on-veto 的定序合约依赖它：门控链 `&` 严格左→右短路，故
    /// **最左**的失败 `replying` 叶子最先跑、最先 `insert_ext_if_absent(GateReply(..))`，
    /// 其右侧叶子永不求值；即便未来某组合子改为急切求值，本方法也保证最左失败叶子的回复胜出。
    /// mutex 中毒时取用 guard 内的数据，而非 panic（保持无 panic 合约）。
    pub fn insert_ext_if_absent<T: Send + Sync + 'static>(&self, value: T) -> bool {
        let mut exts = self.extensions.lock().unwrap_or_else(|e| e.into_inner());
        if exts.contains_key(&TypeId::of::<T>()) {
            return false;
        }
        exts.insert(TypeId::of::<T>(), Box::new(value));
        true
    }

    /// 从每事件 memo 移除 `T`（若存在则返回其值）。
    /// 用于 dispatch 把命令型 handler 的 `ParsedCommand` 限定在该 handler 作用域内，
    /// 避免泄漏给同事件后续的 handler。
    /// mutex 中毒时取用 guard 内的数据，而非 panic（保持无 panic 合约）。
    pub fn remove_ext<T: Send + Sync + 'static>(&self) -> Option<T> {
        let mut exts = self.extensions.lock().unwrap_or_else(|e| e.into_inner());
        exts.remove(&TypeId::of::<T>()).and_then(|v| v.downcast::<T>().ok().map(|b| *b))
    }
}
