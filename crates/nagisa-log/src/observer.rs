//! 可选的事件日志观察者。
//!
//! [`EventLog`] 是一个可配置的构建器:把统一 [`Event`] 经 [`crate::render_line`] 渲染成一行
//! 可读文本（按种类着色、把群号/QQ 号尽量解析成名字、折叠消息内换行),再以 `tracing::event!`
//! 在固定 `target = "nagisa::event"` 上发出——故事件日志可与协议/机制日志（`nagisa-core` 打的
//! 机制日志)按来源单独过滤。
//!
//! 可调开关，互不影响：
//! 1. **运行时开关**：`Arc<AtomicBool>`（默认开），`.handle()` 取一个 [`EventLogHandle`]
//!    在运行时翻转（如挂 `/日志 on|off`）。
//! 2. **按种类过滤**：`.only(&[..])` 白名单 / `.exclude(&[..])` 黑名单（默认只排除心跳）。
//! 3. **日志级别**：`.level(Level)`（默认 `INFO`）。
//! 4. **着色**：`.color(bool)`（默认按 stdout 是否为终端自动判定）——终端上色、重定向/落盘则纯文本。
//! 5. **名字解析**：`.resolve_names(bool)`（默认开）——就地从事件学名字 + 经 `Bot` API 后台回填
//!    群名/成员名（detached，绝不阻塞日志或命令；查不到当次显示裸号，回填后续事件显示名）。
//!
//! `.observer()` 返回一个可克隆闭包，提取 `Arc<Event>` + `Bot`（后者只为后台回填克隆用），
//! 直接喂给 `App::on_top(..)`。观察者只读不写、从不阻断传播、仅发日志，天然防回环。
use crate::messages::MessageStore;
use crate::names::NameStore;
use crate::render::{render_line, RenderOpts, Style};
use nagisa_core::{Bot, EventKind};
use nagisa_types::event::{Event, MessageEvent};
use nagisa_types::id::{MessageId, Peer, Uin};
use nagisa_types::segment::Segment;
use std::collections::HashSet;
use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::Level;

/// 按事件种类的过滤策略。
#[derive(Clone)]
enum Filter {
    /// 放行所有种类。
    All,
    /// 仅放行白名单内的种类。
    Only(HashSet<EventKind>),
    /// 放行除黑名单外的所有种类。
    Exclude(HashSet<EventKind>),
}

impl Filter {
    /// 该事件种类是否应被记录。无法判定种类的事件（`EventKind::of` 返回 `None`）
    /// 一律放行——它们不是高频噪声，宁可记下也不静默丢弃。
    fn allows(&self, kind: Option<EventKind>) -> bool {
        match self {
            Filter::All => true,
            Filter::Only(set) => kind.is_some_and(|k| set.contains(&k)),
            Filter::Exclude(set) => kind.is_none_or(|k| !set.contains(&k)),
        }
    }
}

/// 默认黑名单：只排除**心跳**——它每隔几秒一次,是唯一真正周期性刷屏的 `Meta`。
/// 其余生命周期事件都**不**默认排除:连接/断开(协议端 flap 是想看到的诊断信号)、
/// 账号上线/下线(罕见且重要)、就绪(每次启动一次的「bot 已就绪」标记)都照常显示。
/// 想连心跳也看就 `.all_kinds()`;想另设黑名单就 `.exclude(&[..])`。
fn default_excluded() -> HashSet<EventKind> {
    [EventKind::Heartbeat].into_iter().collect()
}

/// 运行时开关句柄：克隆自 [`EventLog`] 的标志位，业务可在运行时翻转事件日志的开/关
/// （例如一个 `/日志 on|off` 命令）。多处克隆共享同一原子标志。
#[derive(Clone)]
pub struct EventLogHandle {
    enabled: Arc<AtomicBool>,
}

impl EventLogHandle {
    /// 打开事件日志。
    pub fn on(&self) {
        self.enabled.store(true, Ordering::Relaxed);
    }

    /// 关闭事件日志。
    pub fn off(&self) {
        self.enabled.store(false, Ordering::Relaxed);
    }

    /// 直接设置开关。
    pub fn set(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    /// 当前是否开启。
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// 翻转开关，返回翻转后的状态。便于 `/日志 toggle` 之类命令。
    pub fn toggle(&self) -> bool {
        !self.enabled.fetch_xor(true, Ordering::Relaxed)
    }
}

/// 可选的事件日志观察者构建器。配置完毕后用 [`EventLog::observer`] 取出一个可挂到
/// `App::on_top(..)` 的观察者。
#[derive(Clone)]
pub struct EventLog {
    enabled: Arc<AtomicBool>,
    filter: Filter,
    level: Level,
    style: Style,
    resolve_names: bool,
    names: Arc<NameStore>,
    messages: Arc<MessageStore>,
}

/// 撤回内容预览用的最近消息缓存默认容量。
const DEFAULT_RECALL_CAP: usize = 500;

impl Default for EventLog {
    fn default() -> Self {
        Self {
            enabled: Arc::new(AtomicBool::new(true)),
            filter: Filter::Exclude(default_excluded()),
            level: Level::INFO,
            // 默认：stdout 是终端就上色，重定向/落盘则纯文本（避免把 ANSI 写进文件）。
            style: if std::io::stdout().is_terminal() { Style::Ansi } else { Style::Plain },
            resolve_names: true,
            names: NameStore::shared(),
            messages: MessageStore::shared(DEFAULT_RECALL_CAP),
        }
    }
}

impl EventLog {
    /// 新建一个默认配置的事件日志：开启、`INFO` 级、只排除心跳、终端自动上色、开启名字解析。
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置日志级别（默认 `INFO`）。
    pub fn level(mut self, level: Level) -> Self {
        self.level = level;
        self
    }

    /// 显式设置是否着色（默认按 stdout 是否为终端自动判定）。
    pub fn color(mut self, color: bool) -> Self {
        self.style = if color { Style::Ansi } else { Style::Plain };
        self
    }

    /// 是否解析名字（群号/QQ 号 → 名字；默认开）。关掉则一律显示裸号、不发任何 API 回填请求。
    pub fn resolve_names(mut self, resolve: bool) -> Self {
        self.resolve_names = resolve;
        self
    }

    /// 取消按种类过滤：记录所有事件种类（包括默认黑名单里的心跳等）。覆盖此前的
    /// 任何 `.only`/`.exclude` 设置。
    pub fn all_kinds(mut self) -> Self {
        self.filter = Filter::All;
        self
    }

    /// 白名单：仅记录给定的事件种类。覆盖此前的任何 `.only`/`.exclude` 设置。
    pub fn only(mut self, kinds: &[EventKind]) -> Self {
        self.filter = Filter::Only(kinds.iter().copied().collect());
        self
    }

    /// 黑名单：记录除给定种类外的所有事件。覆盖此前的任何 `.only`/`.exclude` 设置
    /// （包括默认黑名单——显式 `.exclude` 即完整指定黑名单）。
    pub fn exclude(mut self, kinds: &[EventKind]) -> Self {
        self.filter = Filter::Exclude(kinds.iter().copied().collect());
        self
    }

    /// 取出一个运行时开关句柄（克隆同一标志位），供业务在运行时开/关事件日志。
    pub fn handle(&self) -> EventLogHandle {
        EventLogHandle { enabled: Arc::clone(&self.enabled) }
    }

    /// 取出共享的名称缓存句柄（与观察者用的是同一实例），便于业务自行预热/查询。
    pub fn names(&self) -> Arc<NameStore> {
        Arc::clone(&self.names)
    }

    /// 设置「最近消息缓存」容量（撤回通知据此显示被撤内容)。默认 500;`0` 即禁用(不记消息、
    /// 撤回只显示「撤回了一条消息」)。
    pub fn recall_cache(mut self, cap: usize) -> Self {
        self.messages = MessageStore::shared(cap);
        self
    }

    /// 取出共享的最近消息缓存句柄(与观察者同一实例),供业务做防撤回等处理。
    pub fn messages(&self) -> Arc<MessageStore> {
        Arc::clone(&self.messages)
    }

    /// 取出观察者：一个可克隆闭包，提取 `Arc<Event>` + `Bot`，实现 nagisa 的 `Handler`
    /// 形状，可直接喂给 `App::on_top(..)`。
    ///
    /// 每个事件：先就地学名字（同步）；若开关关 / 种类被过滤 → 不渲染；否则把
    /// [`render_line`] 出的可读行以配置级别、`target = "nagisa::event"` 发出；最后（若开启名字
    /// 解析）spawn 一个 detached 后台任务经 `Bot` 回填群名/成员名（绝不阻塞当前事件或命令）。
    pub fn observer(
        &self,
    ) -> impl Fn(Arc<Event>, Bot) -> std::future::Ready<()> + Clone + Send + Sync + 'static {
        // 安装出站消息日志器(进程级、只装一次):把 bot 发出的消息合成一条 `is_self` 的
        // `MessageEvent`,走与入站**完全相同**的 [`log_event`](同一记录/渲染/名字解析/开关/级别)。
        // 故出站与入站共用同一套渲染(`render_line`)、同一 `NameStore`、同一 `nagisa::event` 流。
        {
            let enabled = Arc::clone(&self.enabled);
            let filter = self.filter.clone();
            let level = self.level;
            let style = self.style;
            let resolve_names = self.resolve_names;
            let names = Arc::clone(&self.names);
            let messages = Arc::clone(&self.messages);
            nagisa_core::set_outgoing_logger(Box::new(move |peer, segs, self_id, id| {
                let ev = synth_self_message(peer, segs, self_id, id);
                log_event(&ev, &enabled, &filter, style, level, resolve_names, &names, &messages);
            }));
        }

        let enabled = Arc::clone(&self.enabled);
        let filter = self.filter.clone();
        let level = self.level;
        let style = self.style;
        let resolve_names = self.resolve_names;
        let names = Arc::clone(&self.names);
        let messages = Arc::clone(&self.messages);
        move |event: Arc<Event>, bot: Bot| {
            // bot 自己发出的消息会被部分协议端回显成一条 is_self 入站消息(OneBot
            // `message_sent` / sender==self_id)。这类消息出站日志器已记过([发送] 行),入站再记
            // 一遍就成了双重日志,故本观察者跳过它们的记录/渲染。名字回填仍照常(其 peer 可能带新名)。
            let self_echo = matches!(event.as_ref(), Event::Message(m) if m.is_self);
            if !self_echo {
                log_event(event.as_ref(), &enabled, &filter, style, level, resolve_names, &names, &messages);
            }
            // 后台回填（detached，不阻塞）：补该事件涉及的群名/成员名，惠及后续事件。出站事件
            // 无新名可补,故只入站观察者做(且它有 `Bot`)。
            if resolve_names {
                names.backfill(&bot, event.as_ref());
            }
            std::future::ready(())
        }
    }
}

/// 记录 + 渲染一个事件 —— **入站观察者与出站日志器共用的唯一管线**。学名字(同步、廉价)→ 记最近
/// 消息缓存(防撤回)→ 开关开且种类未被过滤则按级别 `render_line` 渲染发到 `nagisa::event`。
/// **不**做后台回填(那需 `Bot`,仅入站观察者做)。出站消息经 [`synth_self_message`] 合成成
/// `is_self` 的 `MessageEvent` 后也走这里,故收发口径字节级一致。
#[allow(clippy::too_many_arguments)]
fn log_event(
    event: &Event,
    enabled: &AtomicBool,
    filter: &Filter,
    style: Style,
    level: Level,
    resolve_names: bool,
    names: &NameStore,
    messages: &MessageStore,
) {
    if resolve_names {
        names.learn_from_event(event);
    }
    // 撤回缓存只回显「他人」被撤的内容,不收 bot 自己的消息(否则只会挤占有界缓存、稀释入站窗口)。
    if let Event::Message(m) = event {
        if !m.is_self {
            messages.record(m.id.clone(), m.sender, m.content.clone());
        }
    }
    if enabled.load(Ordering::Relaxed) && filter.allows(EventKind::of(event)) {
        let opts = RenderOpts {
            style,
            names: resolve_names.then_some(names),
            // 群上下文由 render_line 自事件 peer 自动填,这里给 None 即可。
            group: None,
            messages: Some(messages),
        };
        emit(level, &render_line(event, &opts));
    }
}

/// 把一次 bot 出站发送合成成一条 `is_self` 的 `MessageEvent`,以便走与入站相同的记录/渲染管线。
/// 仅用于日志/缓存——**不**进 dispatch,故不会触发命令 handler(bot 不会回应自己)。
fn synth_self_message(peer: &Peer, segs: &[Segment], self_id: Uin, id: &MessageId) -> Event {
    Event::Message(Box::new(MessageEvent {
        id: id.clone(),
        peer: *peer,
        sender: self_id,
        self_id,
        time: 0,
        content: segs.to_vec(),
        is_self: true,
        group: None,
        member: None,
        friend: None,
        anonymous: None,
        font: None,
        target_id: None,
        message_style: None,
        raw: serde_json::Value::Null,
    }))
}

/// 以固定 `target = "nagisa::event"` 在给定级别发出一行已渲染文本。
///
/// `tracing` 宏要求级别在编译期为常量，故按少数几个 `Level` 分派到对应宏。
fn emit(level: Level, line: &str) {
    const TARGET: &str = "nagisa::event";
    match level {
        Level::ERROR => tracing::error!(target: TARGET, "{line}"),
        Level::WARN => tracing::warn!(target: TARGET, "{line}"),
        Level::INFO => tracing::info!(target: TARGET, "{line}"),
        Level::DEBUG => tracing::debug!(target: TARGET, "{line}"),
        Level::TRACE => tracing::trace!(target: TARGET, "{line}"),
    }
}
