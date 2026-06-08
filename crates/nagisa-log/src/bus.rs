//! 日志即事件：把（已过滤的）`tracing` 日志记录转发到一个 `broadcast` 总线，业务侧可订阅
//! 并持久化（写库 / 上报）。
//!
//! 这条总线与消息 `Event` / handler 路径**完全分离**：它只单向地把日志记录广播出去，订阅者
//! 拿到的是 [`LogRecord`]（纯数据），既不会回灌到 dispatch，也不产生新的消息事件——天然防回环。
//!
//! **防回环的两道闸**（都在 [`LogBusLayer`] 里）：
//! 1. **保留 `target`**：凡 `target` 以 `nagisa::log` 开头的记录一律不转发——总线消费者
//!    （写库 / 上报）自己打的日志若复用这个保留前缀，就不会再次被总线捕获、形成回环。
//! 2. **最低级别**：低于 `min_level` 的记录直接丢弃，避免 `DEBUG`/`TRACE` 洪泛把总线撑爆。
//!
//! 用法:
//! ```rust,ignore
//! let (layer, bus) = LogBusLayer::new(256, Level::INFO);
//! // 把 layer 组合进 subscriber（init() 会替你做）:registry().with(layer)
//! on_record(&bus, move |rec| { let db = db.clone(); async move { db.insert_log(rec).await; } });
//! ```
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::SystemTime;

use tokio::sync::broadcast;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

/// 总线消费者自报日志的保留 `target` 前缀。任何以此开头的记录都**不会**被
/// [`LogBusLayer`] 转发，从而保证「消费总线时打的日志」不会再回灌到总线（防回环）。
pub const RESERVED_TARGET_PREFIX: &str = "nagisa::log";

/// 广播给业务侧的一条日志记录（纯数据快照，可 `Clone` / 跨任务搬运）。
#[derive(Debug, Clone)]
pub struct LogRecord {
    /// 日志级别。
    pub level: Level,
    /// 来源 `target`（即事件的 `target`，形如 `nagisa::dispatch` / `nagisa::onebot` / `biz`）。
    pub source: String,
    /// 渲染后的消息正文（`message` 字段的文本）。
    pub message: String,
    /// 其余结构化字段（`message` 除外），按字段名有序排列，便于业务侧检索 / 入库。
    pub fields: BTreeMap<String, String>,
    /// 记录产生时刻（墙钟时间）。业务侧可直接转成时间戳落库。
    pub timestamp: SystemTime,
}

/// 业务侧订阅句柄。`recv().await` 取下一条记录；落后过多时 `broadcast` 会回 `Lagged`，
/// 由调用方决定是否容忍丢弃。
pub type LogBusReceiver = broadcast::Receiver<LogRecord>;

/// 日志总线：一个 `broadcast::Sender<LogRecord>` 的薄封装，作为**订阅者工厂**。
///
/// [`init`](crate::init) 在装好 [`LogBusLayer`] 后把对应的 `LogBus` 一并返回，业务用
/// [`LogBus::subscribe`]（或便捷的 [`on_record`]）拿到记录流。`LogBus` 不持有
/// 任何 `tracing` 状态，纯粹是分发端。
#[derive(Clone)]
pub struct LogBus {
    tx: broadcast::Sender<LogRecord>,
}

impl LogBus {
    /// 再取一个订阅者。多个消费者各自独立收取（典型用法：一个写库、一个上报）。
    pub fn subscribe(&self) -> LogBusReceiver {
        self.tx.subscribe()
    }

    /// 当前活跃订阅者数量。无人订阅时转发会被跳过，故可据此判断是否值得开销。
    pub fn receiver_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

/// 一个 `tracing` [`Layer`]：把每条（通过两道闸的）event 转成 [`LogRecord`] 并 `try_send`
/// 到 `broadcast` 总线。
///
/// 两道闸（防回环 / 防洪泛）：
/// - `target` 以 [`RESERVED_TARGET_PREFIX`] 开头 → 跳过；
/// - 级别低于 `min_level` → 跳过。
///
/// 其余的来源过滤交给与它组合的 `EnvFilter`（来源过滤是统一记录器的职责，见
/// [`crate::init`]）：总线收到的就是「已按来源过滤后的」记录。
#[derive(Clone)]
pub struct LogBusLayer {
    tx: broadcast::Sender<LogRecord>,
    min_level: Level,
}

impl LogBusLayer {
    /// 新建总线层，返回（可作为 `tracing` 层挂载的）[`LogBusLayer`] 与配套的 [`LogBus`]
    /// 订阅者工厂。`capacity` 是 `broadcast` 环形缓冲容量（订阅者落后超过它会收到
    /// `Lagged`）；`min_level` 是转发的最低级别（更低的记录直接丢弃，防洪泛）。
    pub fn new(capacity: usize, min_level: Level) -> (Self, LogBus) {
        let (tx, _rx) = broadcast::channel(capacity);
        (Self { tx: tx.clone(), min_level }, LogBus { tx })
    }

    /// 该 event 是否应转发：保留 `target` 与低于 `min_level` 的一律不转发。
    fn should_forward(&self, meta: &tracing::Metadata<'_>) -> bool {
        if meta.target().starts_with(RESERVED_TARGET_PREFIX) {
            return false;
        }
        // tracing 的 Level：值越「verbose」排序越大（TRACE > … > ERROR），故「level <= min_level」
        // 表示「至少和 min_level 一样重要」。
        meta.level() <= &self.min_level
    }
}

impl<S: Subscriber> Layer<S> for LogBusLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        // 防回环 + 防洪泛：保留 target / 低级别直接不转发。
        if !self.should_forward(meta) {
            return;
        }
        // 无人订阅就别白干（也避免把无意义的记录塞进环形缓冲）。
        if self.tx.receiver_count() == 0 {
            return;
        }
        let mut visitor = RecordVisitor::default();
        event.record(&mut visitor);
        let record = LogRecord {
            level: *meta.level(),
            source: meta.target().to_string(),
            message: visitor.message,
            fields: visitor.fields,
            timestamp: SystemTime::now(),
        };
        // try_send 语义：用非阻塞 send；它只在零订阅者时失败——上面已挡掉，这里忽略竞态下
        // 的偶发失败即可。绝不在失败时重新打日志（那会破坏防回环）。
        let _ = self.tx.send(record);
    }
}

/// 把一条 event 的字段拆成「正文 + 结构化字段」：`message` 字段进 [`Self::message`]，
/// 其余字段进 [`Self::fields`]（按名有序）。
#[derive(Default)]
struct RecordVisitor {
    message: String,
    fields: BTreeMap<String, String>,
}

impl Visit for RecordVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            // message 字段就是正文本体（去掉 Debug 的引号留给业务自行决定，这里保留原样文本）。
            let mut s = String::new();
            let _ = write!(s, "{value:?}");
            self.message = s;
        } else {
            // 其它结构化字段单独留存，便于业务侧按字段检索 / 入库。
            self.fields.insert(field.name().to_string(), format!("{value:?}"));
        }
    }
}

/// 在后台 tokio 任务里消费 `bus`，对每条 [`LogRecord`] 调用异步 `f` —— 业务据此把日志
/// 持久化（写库 / 上报）。返回该任务的 [`tokio::task::JoinHandle`]（通常无需保留）。
///
/// 这是「日志即事件」的业务入口:
/// ```rust,ignore
/// on_record(&bus, move |rec| {
///     let db = db.clone();
///     async move { db.insert_log(rec).await; }
/// });
/// ```
///
/// **防回环**：`f` 内部若要打日志，请使用以 [`RESERVED_TARGET_PREFIX`]（`"nagisa::log"`）
/// 开头的 `target`，这样那条日志不会再被 [`LogBusLayer`] 转发回总线。
///
/// 总线 `Lagged`（消费过慢、环形缓冲覆盖）时跳过丢失的那批记录继续消费；发送端全部
/// drop（`Closed`）时任务退出。
pub fn on_record<F, Fut>(bus: &LogBus, mut f: F) -> tokio::task::JoinHandle<()>
where
    F: FnMut(LogRecord) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(record) => f(record).await,
                // 消费太慢被覆盖：跳过丢失的批次，继续（容忍丢弃，业务不阻塞日志路径）。
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                // 所有发送端已 drop：再没有新记录，退出任务。
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}
