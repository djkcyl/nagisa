//! 中断引擎：`Session`/`Waiter`——事件通用、作用域可组合的条件监听。
//! 支持多轮（一个 waiter 经 `mpsc` 收多条事件）、群内多人（作用域不绑死发送者）、
//! 嵌套（深者优先），以及内层依赖注入（收到的事件重建成新 `Ctx`）；分发优先级三层、
//! 由 router 保证（top > waiter > 默认 handler）。
use crate::bot::Bot;
use crate::ctx::{Ctx, StateMap};
use crate::event_trigger::EventKind;
use crate::extract::{Extracted, FromContext, Reject, Reply};
use async_trait::async_trait;
use nagisa_types::event::Event;
use nagisa_types::prelude::*;
use nagisa_types::message::MessageExt;
use std::any::TypeId;
use std::collections::HashSet;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio::time::Instant;

/// 用户提供的候选事件谓词，作为 [`Selector`] 的兜底维度。起别名是为了让
/// `Selector::filter` 避开 `clippy::type_complexity`。
pub type EventFilter = Arc<dyn Fn(&Event) -> bool + Send + Sync>;

/// 未来事件的选择器。默认 `EventKind::Message`。`None` 字段是该维度的通配，
/// 所有 `Some` 字段必须全部命中（AND）。任意额外要求（含命令/文本匹配）走 `filter`。
#[derive(Clone)]
pub struct Selector {
    pub kind: EventKind,
    pub peer: Option<Peer>,
    pub user: Option<Uin>,
    pub filter: Option<EventFilter>,
}

impl Default for Selector {
    fn default() -> Self {
        Selector { kind: EventKind::Message, peer: None, user: None, filter: None }
    }
}

impl Selector {
    /// `event` 是否满足本选择器所有受约束的维度。
    pub fn matches(&self, event: &Event) -> bool {
        if EventKind::of(event) != Some(self.kind) {
            return false;
        }
        if let Some(p) = self.peer {
            if event.peer() != Some(p) {
                return false;
            }
        }
        if let Some(u) = self.user {
            if event.sender() != Some(u) {
                return false;
            }
        }
        if let Some(f) = &self.filter {
            if !f(event) {
                return false;
            }
        }
        true
    }
}

/// 仅消息的便捷预设（事件型作用域手写 `.on(Kind).filter(..)`）。
#[derive(Clone)]
pub struct Scope(Selector);

impl Scope {
    /// `peer`（某个具体的群或好友会话）里的消息。
    ///
    /// 取名 `peer`（而非 `group`）是为了协调一致：[`Scope`] 寻址一个完整的 [`Peer`]
    /// （带场景信息），而 `Bot::group(impl Into<Uin>)` 收的是裸群号。名字区分开，避免
    /// `Scope::group(123)` 看起来该能编译、实际只接受 `Peer`。
    pub fn peer(peer: Peer) -> Scope {
        Scope(Selector { peer: Some(peer), ..Selector::default() })
    }
    /// 任意会话里来自 `user` 的消息。
    pub fn user(user: Uin) -> Scope {
        Scope(Selector { user: Some(user), ..Selector::default() })
    }
    /// `peer` 里来自 `user` 的消息。
    pub fn from(peer: Peer, user: Uin) -> Scope {
        Scope(Selector { peer: Some(peer), user: Some(user), ..Selector::default() })
    }
    /// 消费成内层 `Selector`。
    pub fn into_selector(self) -> Selector {
        self.0
    }
}

/// `try_deliver` 探测的结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Delivery {
    pub delivered: bool,
    pub block: bool,
}

/// [`recv_with`](Waiter::recv_with) / [`recv_with_sync`](Waiter::recv_with_sync)
/// 的闭包对每条投递事件返回的类型化裁决：闭包**逐事件**决定继续等、用某值收束、还是放弃。
///
/// 刻意**没有 `Consume(T)`** 变体：waiter 条目的 `block`（吞没还是放行）在注册时
/// （builder 上的 `.block(true|false)`）就固定了，`try_deliver` 在闭包跑**之前**读它。
/// 故闭包裁决物理上无法回溯改写已发出的 `Delivery`——传播是构建期的选择，不是流程里的决定。
pub enum WaitFlow<T> {
    /// 在剩余超时内继续等。
    Continue,
    /// 用 `Some(value)` 收束本次等待。命中事件的传播在 `build()` 时已定。
    Done(T),
    /// 立即放弃 → 等待返回 `None`。
    Cancel,
}

/// store 里一条已注册的 waiter。
struct WaiterEntry {
    id: u64,
    depth: u32,
    selector: Selector,
    block: bool,
    tx: mpsc::Sender<Arc<Event>>,
}

/// 活跃 waiter 的共享注册表。用 `Vec`（而非单槽），故多个并发/嵌套作用域可共存。
/// 注入 app 共享态；router 在 `run_handlers` 的 waiter 检查层查询它。
pub struct WaiterStore {
    waiters: Mutex<Vec<WaiterEntry>>,
    next_id: AtomicU64,
}

impl Default for WaiterStore {
    fn default() -> Self {
        Self { waiters: Mutex::new(Vec::new()), next_id: AtomicU64::new(1) }
    }
}

impl WaiterStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<WaiterEntry>> {
        self.waiters.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 取一个新的 waiter id（`Waiter` 用它，使 `Drop` 能精确摘除自己）。
    pub fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// 注册一条 waiter 条目，返回其 id（生产路径走 `WaiterBuilder::build`，它预分配好 id）。
    pub fn register(
        &self,
        depth: u32,
        selector: Selector,
        block: bool,
        tx: mpsc::Sender<Arc<Event>>,
    ) -> u64 {
        let id = self.next_id();
        self.register_with_id(id, depth, selector, block, tx);
        id
    }

    /// 用调用方给定的 id 注册（使 `Waiter` 持同一 id 供 `Drop` 摘除）。
    pub fn register_with_id(
        &self,
        id: u64,
        depth: u32,
        selector: Selector,
        block: bool,
        tx: mpsc::Sender<Arc<Event>>,
    ) {
        self.lock().push(WaiterEntry { id, depth, selector, block, tx });
    }

    /// 摘除 id 为 `id` 的条目（幂等；由 `Waiter::drop` 调用）。
    pub fn remove(&self, id: u64) {
        self.lock().retain(|e| e.id != id);
    }

    /// 当前存活条目数。
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// 是否没有存活条目。
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    /// 尝试把 `event` 投递给最深的匹配 waiter。投给已关闭的接收端则摘除该条目、记为未命中。
    pub fn try_deliver(&self, event: &Arc<Event>) -> Delivery {
        loop {
            // 取 depth 最高的匹配条目；同深则取 id 最大者（最新）。
            let chosen = {
                let guard = self.lock();
                guard
                    .iter()
                    .filter(|e| e.selector.matches(event))
                    .max_by_key(|e| (e.depth, e.id))
                    .map(|e| (e.id, e.block, e.tx.clone()))
            };
            let Some((id, block, tx)) = chosen else {
                return Delivery { delivered: false, block: false };
            };
            match tx.try_send(Arc::clone(event)) {
                Ok(()) => return Delivery { delivered: true, block },
                // 接收端已 drop → waiter 已死;摘除后重试下一个匹配。
                Err(TrySendError::Closed(_)) => {
                    self.remove(id);
                    continue;
                }
                // 缓冲满但接收端仍在 → waiter 是个活跃 session、只是 handler 暂时落后。**不**摘除
                // (那会静默丢数据);当作未命中处理。
                Err(TrySendError::Full(_)) => return Delivery { delivered: false, block: false },
            }
        }
    }
}

/// single-flight 作用域的可哈希身份，从 [`Selector`] 的受约束维度（kind + peer + user）投影而来。
/// 选择器任意的 `filter` **不**进键——single-flight 作用域（`Scope::peer`/`user`/`from`）从不设
/// filter，且闭包本身也无从哈希/比较身份。
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct FlightKey {
    kind: EventKind,
    peer: Option<Peer>,
    user: Option<Uin>,
}

impl FlightKey {
    fn from_selector(sel: &Selector) -> Self {
        FlightKey { kind: sel.kind, peer: sel.peer, user: sel.user }
    }
}

/// 在飞 single-flight 作用域的共享注册表。一个装着当前持有的作用域键
/// （kind + peer + user，从 [`Selector`] 投影）的 `HashSet`，由 `Mutex` 守护（std 内存存储、
/// 无新依赖——与 [`CooldownStore`](crate::CooldownStore) 同形状）。`App::new` 把它与
/// `WaiterStore` 并排注入共享态；[`Session::single_flight`] 据此抢占。
pub struct FlightStore {
    held: Mutex<HashSet<FlightKey>>,
}

impl Default for FlightStore {
    fn default() -> Self {
        Self { held: Mutex::new(HashSet::new()) }
    }
}

impl FlightStore {
    /// 新建空存储。
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashSet<FlightKey>> {
        self.held.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 当前在飞的作用域数。
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// 当前是否没有作用域在飞。
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    /// 尝试抢占 `selector` 的作用域。作用域空闲时返回 [`FlightGuard`]（drop 时释放）；
    /// 已被持有则返回 `None`（第二个并发启动被拒）。
    fn acquire(store: Arc<FlightStore>, selector: Selector) -> Option<FlightGuard> {
        let key = FlightKey::from_selector(&selector);
        {
            let mut held = store.lock();
            if !held.insert(key.clone()) {
                return None; // 已在飞 → 拒绝第二个并发启动。
            }
        }
        Some(FlightGuard { store, key })
    }
}

/// 一个 RAII 的 single-flight 令牌。存活期间把其作用域标记为在飞，故同作用域上第二次
/// [`Session::single_flight`] 返回 `None`。其 `Drop` 释放作用域，故被守护 handler 的**每条**
/// 退出路径——`?` 提前返回、正常路径、panic——都无需手写清理即可释放该槽。沿用 [`Waiter`] 的
/// drop-摘除纪律。
pub struct FlightGuard {
    store: Arc<FlightStore>,
    key: FlightKey,
}

impl Drop for FlightGuard {
    fn drop(&mut self) {
        self.store.lock().remove(&self.key);
    }
}

/// 嵌套深度标记，打在重建的 `Ctx` 上，使从中提取的 `Session` 比产出它的 waiter 深一层注册。
#[derive(Clone, Copy, Debug)]
pub struct WaiterDepth(pub u32);

/// `Waiter` 的构建器。默认 `kind = Message`、`block = true`。`depth` 由创建它的 `Session`
/// 设定（`self.depth + 1`）。
pub struct WaiterBuilder {
    bot: Bot,
    state: Arc<StateMap>,
    store: Arc<WaiterStore>,
    depth: u32,
    selector: Selector,
    block: bool,
    /// session 的源会话 peer（如有）与触发用户（如有），随构建器携带，
    /// 使 [`from_starter`](Self::from_starter) 能直接作用域到「这个 peer + 这个 user」，
    /// 无需调用方重新填写。
    starter_peer: Option<Peer>,
    starter_user: Option<Uin>,
}

impl WaiterBuilder {
    /// 监听该事件种类（默认 `Message`）。
    pub fn on(mut self, kind: EventKind) -> Self {
        self.selector.kind = kind;
        self
    }
    /// 用 `Scope` 预设整个替换选择器。
    pub fn scope(mut self, scope: Scope) -> Self {
        self.selector = scope.into_selector();
        self
    }
    /// 限定该会话 peer。
    pub fn peer(mut self, peer: Peer) -> Self {
        self.selector.peer = Some(peer);
        self
    }
    /// 限定该行为用户。
    pub fn user(mut self, user: Uin) -> Self {
        self.selector.user = Some(user);
        self
    }
    /// 限定 `peer` 里的 `user`。
    pub fn from(mut self, peer: Peer, user: Uin) -> Self {
        self.selector.peer = Some(peer);
        self.selector.user = Some(user);
        self
    }
    /// 作用域到**触发本 session 的会话和用户**：session 的源 peer 加其触发用户。这把每个
    /// 「问当初发问的同一个人」waiter 否则都要重复的 `if grp == group && mbr == member { .. }`
    /// 手写守卫收掉。只有 session 确实缺某维度时才留作通配（如根于无 peer 事件的 session 无
    /// `starter_user`）。
    pub fn from_starter(mut self) -> Self {
        self.selector.peer = self.starter_peer;
        self.selector.user = self.starter_user;
        self
    }
    /// 额外要求该任意谓词。
    pub fn filter<F>(mut self, f: F) -> Self
    where
        F: Fn(&Event) -> bool + Send + Sync + 'static,
    {
        self.selector.filter = Some(Arc::new(f));
        self
    }
    /// 命中是否吞掉、不下传到更低层（默认 `true`）。
    pub fn block(mut self, block: bool) -> Self {
        self.block = block;
        self
    }
    /// 注册条目并返回 RAII 的 `Waiter`。
    pub fn build(self) -> Waiter {
        let (tx, rx) = mpsc::channel(16);
        let id = self.store.next_id();
        self.store.register_with_id(id, self.depth, self.selector, self.block, tx);
        Waiter {
            id,
            rx: tokio::sync::Mutex::new(rx),
            store: self.store,
            bot: self.bot,
            state: self.state,
            depth: self.depth,
        }
    }
}

/// 一个活跃 waiter。RAII：其 `Drop` 摘除 store 条目。经 `mpsc` 收投递事件（多轮、无需重注册）。
pub struct Waiter {
    id: u64,
    rx: tokio::sync::Mutex<mpsc::Receiver<Arc<Event>>>,
    store: Arc<WaiterStore>,
    bot: Bot,
    state: Arc<StateMap>,
    depth: u32,
}

impl Drop for Waiter {
    fn drop(&mut self) {
        self.store.remove(self.id);
    }
}

impl Waiter {
    /// 等下一条投递事件（最多 `timeout`），把它重建成新 `Ctx`（使 `FromContext` 提取器可用），
    /// 并打上 `WaiterDepth(self.depth)` 标记，使从中提取的嵌套 `Session` 注册得更深。
    pub async fn recv_event(&self, timeout: Duration) -> Option<Ctx> {
        let ev = self.recv_raw(timeout).await?;
        let ctx = Ctx::new(ev, self.bot.clone(), Arc::clone(&self.state));
        ctx.insert_ext(WaiterDepth(self.depth));
        Some(ctx)
    }

    /// 循环：`recv_event` → 跑 `T::from_context`；遇 `Reject::Skip` 在**剩余**超时内继续等；
    /// `Ok(v) -> Some(v)`；超时或 `Reject::Error -> None`。
    pub async fn recv<T: FromContext>(&self, timeout: Duration) -> Option<T> {
        Some(self.recv_session::<T>(timeout).await?.0)
    }

    /// 同 [`recv`](Self::recv)，但额外从同一收到的事件重建一个就绪、深一层的 [`Session`] 一并返回。
    /// 这是嵌套 / 事件发起式中断的顺手入口：handler 可在返回的 `Session` 上继续挂下一个 waiter，
    /// 无需泄漏裸 `Ctx`、也无需手搓 `Error`（即旧的 `recv_event` → `Session::from_context(&ctx)`）。
    pub async fn recv_session<T: FromContext>(&self, timeout: Duration) -> Option<(T, Session)> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return None;
            }
            let ctx = self.recv_event(remaining).await?;
            match T::from_context(&ctx).await {
                // Ctx 带着 `WaiterDepth(self.depth)`,故提取出的 Session 比本 waiter 深一层注册。
                // 这里提取必成功(WaiterStore 一定在——本 waiter 就是它产出的)。
                Ok(v) => match Session::from_context(&ctx).await {
                    Ok(session) => return Some((v, session)),
                    Err(_) => return None,
                },
                Err(Reject::Skip) => continue,
                Err(Reject::Error(_)) => return None,
            }
        }
    }

    /// 类型化的逐事件裁决等待。闭包收到从每条投递事件重建的、**owned、可做依赖注入**的 `Ctx`
    /// （故能提取任意东西——跑 `Slots<T>` 匹配、经 `Reply::from_context` 引用、对话中途回话），
    /// 返回类型化的 [`WaitFlow<T>`]：
    /// - [`WaitFlow::Continue`] —— 在**剩余**超时内继续等；
    /// - [`WaitFlow::Done(v)`](WaitFlow::Done) —— 收束为 `Some(v)`；
    /// - [`WaitFlow::Cancel`] —— 立即放弃 → `None`。
    ///
    /// 传播（吞没还是放行）**不是**闭包裁决：它在注册时经 builder 的 `.block(true|false)` 固定。
    /// 闭包无法回溯吞掉刚命中它的事件——`try_deliver` 在本闭包跑之前就已发出 `build()` 时定下的
    /// `Delivery.block`。
    ///
    /// ## 借用合约
    ///
    /// `Ctx` 不是 `Clone`，故每轮产出一个**全新 owned** 的 `Ctx`。由于 `FnMut` 返回的 `async move`
    /// future *逃出*闭包体，这里无法把裸 `&mut` 借用跨 await 点持有；**async** 路径上要跨轮可变状态，
    /// 就捕获一个共享 cell（`Rc<RefCell<_>>` / `Arc<Mutex<_>>`）并**每轮**克隆句柄，使每个 future
    /// 持有自己的重借用：
    ///
    /// ```ignore
    /// let players = Rc::new(RefCell::new(roster));      // FnMut 捕获的共享 cell
    /// let outcome = waiter.recv_with(timeout, |ctx| {
    ///     let players = Rc::clone(&players);            // 每轮重借用
    ///     async move {                                  // 只 move 这个克隆 + owned 的 `ctx`
    ///         // players.borrow_mut().push(..); 经 Reply::from_context(&ctx) 回话 …
    ///     }
    /// }).await;
    /// ```
    ///
    /// 裁决**无需**等待中途 `.await` 时，优先用 [`recv_with_sync`](Self::recv_with_sync)：其闭包直接
    /// 返回 `WaitFlow<T>`，跨轮的裸 `&mut` 无 async-move 税即可用（常见的对战/开局凑人形态）。
    pub async fn recv_with<T, F, Fut>(&self, timeout: Duration, mut f: F) -> Option<T>
    where
        F: FnMut(Ctx) -> Fut,
        Fut: Future<Output = WaitFlow<T>>,
    {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            let ctx = self.recv_event(remaining).await?;
            match f(ctx).await {
                WaitFlow::Continue => continue,
                WaitFlow::Done(v) => return Some(v),
                WaitFlow::Cancel => return None,
            }
        }
    }

    /// [`recv_with`](Self::recv_with) 的**同步**裁决变体：闭包**无中途 `.await`** 地直接返回
    /// [`WaitFlow<T>`]，故捕获的 `&mut` 状态无需 async-move 重借用税。这是常见的「命中或继续等、
    /// 裁决里不做 I/O」形态；裁决本身须 `.await`（查 DB、等待中途回话）时用
    /// [`recv_with`](Self::recv_with)。
    pub async fn recv_with_sync<T, F>(&self, timeout: Duration, mut f: F) -> Option<T>
    where
        F: FnMut(Ctx) -> WaitFlow<T>,
    {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            let ctx = self.recv_event(remaining).await?;
            match f(ctx) {
                WaitFlow::Continue => continue,
                WaitFlow::Done(v) => return Some(v),
                WaitFlow::Cancel => return None,
            }
        }
    }

    /// 是/否确认。等 `who` 的一条消息；`yes` 词收束 `Some(true)`，`no` 词收束 `Some(false)`。
    /// **其它任何输入都重新追问**（回贴 `yes`/`no` 提示、在剩余超时内继续等）而非收束，
    /// 即「没听懂，请回答 y/n」式的循环。超时 → `None`。
    ///
    /// 匹配在消息纯文本上做：去首尾空白、大小写不敏感。
    pub async fn confirm(
        &self,
        timeout: Duration,
        who: Uin,
        yes: &str,
        no: &str,
    ) -> Option<bool> {
        let yes = yes.trim().to_lowercase();
        let no = no.trim().to_lowercase();
        self.recv_with(timeout, |ctx| {
            let yes = yes.clone();
            let no = no.clone();
            async move {
                let Some(m) = ctx.message() else { return WaitFlow::Continue };
                if m.sender != who {
                    return WaitFlow::Continue;
                }
                let text = m.content.extract_text();
                let t = text.trim().to_lowercase();
                if t == yes {
                    WaitFlow::Done(true)
                } else if t == no {
                    WaitFlow::Done(false)
                } else {
                    // 来自目标用户的无法识别输入 → 重新追问、继续等。
                    if let Ok(reply) = Reply::from_context(&ctx).await {
                        let _ = reply.text(format!("请回复「{yes}」或「{no}」")).await;
                    }
                    WaitFlow::Continue
                }
            }
        })
        .await
    }

    /// 等一行自由文本。作用域内首条消息收束 `Some(text)`，但 `cancel` 词的消息收束 `None`。
    /// 空/纯空白文本忽略（继续等）。超时 → `None`。
    pub async fn recv_text(&self, timeout: Duration, cancel: &str) -> Option<String> {
        let cancel = cancel.trim().to_lowercase();
        self.recv_with_sync(timeout, |ctx| {
            let Some(m) = ctx.message() else { return WaitFlow::Continue };
            let text = m.content.extract_text();
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return WaitFlow::Continue;
            }
            if trimmed.to_lowercase() == cancel {
                return WaitFlow::Cancel;
            }
            WaitFlow::Done(text)
        })
        .await
    }

    /// 等一行自由文本并**解析它、失败则重新追问**（[`confirm`](Self::confirm)/
    /// [`recv_text`](Self::recv_text) 的兄弟）。`parse` 把 trim 后的文本映射成 `Ok(value)` →
    /// 收束 `Some(value)`，或 `Err(hint)` → 回贴 `hint` 并在剩余超时内继续等。`cancel` 词的消息
    /// 收束 `None`；空/空白输入忽略；超时 → `None`。
    ///
    /// 与兄弟方法一样，它在**内部**派生 [`Reply`]（无 `&Reply` 形参）并吞掉发送错误，故一个多步
    /// 追问收成一次调用：`let Some(c) = waiter.recv_parse(d, "取消", parse_color).await else { … };`。
    /// 那句礼节性的「已取消」留作调用侧的小尾巴（cancel/超时都返回裸 `None`，与 `recv_text` 一致）。
    pub async fn recv_parse<T, F>(&self, timeout: Duration, cancel: &str, parse: F) -> Option<T>
    where
        F: Fn(&str) -> std::result::Result<T, String>,
    {
        // 局部两段式裁决:先同步解析(借 `ctx`),再仅为重新追问把 `ctx` move 进 async 块——
        // 让 `parse`(一个 `Fn`)留在被 move 的 future 之外。
        enum Pre<U> {
            Skip,
            Cancel,
            Done(U),
            Reprompt(String),
        }
        let cancel = cancel.trim().to_lowercase();
        self.recv_with(timeout, move |ctx| {
            let pre = match ctx.message() {
                None => Pre::Skip,
                Some(m) => {
                    let text = m.content.extract_text();
                    let trimmed = text.trim();
                    if trimmed.is_empty() {
                        Pre::Skip
                    } else if trimmed.to_lowercase() == cancel {
                        Pre::Cancel
                    } else {
                        match parse(trimmed) {
                            Ok(v) => Pre::Done(v),
                            Err(hint) => Pre::Reprompt(hint),
                        }
                    }
                }
            };
            async move {
                match pre {
                    Pre::Skip => WaitFlow::Continue,
                    Pre::Cancel => WaitFlow::Cancel,
                    Pre::Done(v) => WaitFlow::Done(v),
                    Pre::Reprompt(hint) => {
                        if let Ok(reply) = Reply::from_context(&ctx).await {
                            let _ = reply.text(hint).await;
                        }
                        WaitFlow::Continue
                    }
                }
            }
        })
        .await
    }

    async fn rx_guard(&self) -> tokio::sync::MutexGuard<'_, mpsc::Receiver<Arc<Event>>> {
        self.rx.lock().await
    }

    async fn recv_raw(&self, timeout: Duration) -> Option<Arc<Event>> {
        let mut rx = self.rx_guard().await;
        match tokio::time::timeout(timeout, rx.recv()).await {
            Ok(Some(ev)) => Some(ev),
            _ => None,
        }
    }
}

/// 一个会话句柄（`FromContext` 提取器）。捕获 bot、共享态、`WaiterStore`（取自共享态）、
/// 当前会话 peer（如有），以及当前嵌套深度（取自 Ctx 上的 `WaiterDepth` ext，默认 0）。
pub struct Session {
    bot: Bot,
    state: Arc<StateMap>,
    store: Arc<WaiterStore>,
    flight: Arc<FlightStore>,
    peer: Option<Peer>,
    user: Option<Uin>,
    depth: u32,
}

impl Session {
    /// 提取本 session 的会话 peer（如有）。根于无 peer 事件的 session 为 `None`。
    pub fn peer(&self) -> Option<Peer> {
        self.peer
    }

    /// 触发本 session 的用户（如有，即消息发送者）。根于无 sender 事件的 session 为 `None`。
    pub fn user(&self) -> Option<Uin> {
        self.user
    }

    /// 开一个比本 session 深一层的 waiter。
    pub fn waiter(&self) -> WaiterBuilder {
        WaiterBuilder {
            bot: self.bot.clone(),
            state: Arc::clone(&self.state),
            store: Arc::clone(&self.store),
            depth: self.depth + 1,
            selector: Selector::default(),
            block: true,
            starter_peer: self.peer,
            starter_user: self.user,
        }
    }

    /// 在 `scope` 上抢一个 single-flight 守卫。若另一次调用已持同一作用域则返回 `None`
    /// （第二个并发启动被拒）。返回的 [`FlightGuard`] drop 时自动释放，故每条退出路径——含 `?`
    /// 提前返回与 panic——都无需手写 `del RUNNING[..]` 即可释放该槽。
    ///
    /// 语法糖：`Scope::user(uin)` 按人、`Scope::peer(peer)` 按会话（典型的游戏守卫）、
    /// `Scope::from(peer, uin)` 按「会话内某人」。
    pub fn single_flight(&self, scope: Scope) -> Option<FlightGuard> {
        FlightStore::acquire(Arc::clone(&self.flight), scope.into_selector())
    }

    /// 作用域到**本 session 自己的触发用户**的 single-flight——是 waiter 的
    /// [`WaiterBuilder::from_starter`] 在 flight 上的对应物，故常见的「行为用户」守卫无需
    /// `Scope::user(Uin(me))` 绕一圈。若该用户已持有作用域、**或** session 无触发用户（无 peer
    /// 事件）则返回 `None`——对 `#[command]`（消息）handler 无害，它们总有 sender。
    pub fn single_flight_user(&self) -> Option<FlightGuard> {
        self.user.and_then(|u| self.single_flight(Scope::user(u)))
    }
}

#[async_trait]
impl FromContext for Session {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        let store = ctx
            .state()
            .get(&TypeId::of::<WaiterStore>())
            .and_then(|a| Arc::clone(a).downcast::<WaiterStore>().ok())
            .ok_or_else(|| {
                Reject::Error(Error::action_kind(
                    ActionErrorKind::Other,
                    "WaiterStore not registered (use App::new or register it)",
                ))
            })?;
        // FlightStore 由 `App::new` 与 WaiterStore 并排自动注册。若是裸 Router 没装它(嵌入方场景),
        // 退回一个 per-session 的新 store,使 `single_flight` 仍可用(只是此时它不跨 session 共享)。
        let flight = ctx
            .state()
            .get(&TypeId::of::<FlightStore>())
            .and_then(|a| Arc::clone(a).downcast::<FlightStore>().ok())
            .unwrap_or_else(|| Arc::new(FlightStore::new()));
        let depth = ctx.get_ext::<WaiterDepth>().map(|d| d.0).unwrap_or(0);
        Ok(Session {
            bot: ctx.bot().clone(),
            state: Arc::clone(ctx.state()),
            store,
            flight,
            peer: ctx.event().peer(),
            user: ctx.event().sender(),
            depth,
        })
    }
}
