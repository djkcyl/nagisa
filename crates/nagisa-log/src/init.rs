//! 统一记录器:一次 [`init`] 装好控制台、可选滚动文件、可选日志总线,以及按来源
//! （`target`)过滤的全局 `tracing` 订阅者。
//!
//! 四个层 / 一个过滤器正交组合,经 [`LogConfig`] 取舍:
//! 1. **来源过滤**:一个 [`EnvFilter`]。优先读环境变量 `RUST_LOG`,否则用
//!    [`LogConfig::filter`]。框架的每条日志都带一个 `target`（形如
//!    `nagisa::dispatch` / `nagisa::onebot` / `nagisa::event`),所以
//!    `RUST_LOG=nagisa::onebot=warn,nagisa::event=info` 能**按来源**而不仅按级别过滤。
//! 2. **控制台层**:一个 fmt 层,`ansi` / `json` 由 [`LogConfig`] 决定。非 JSON 时用
//!    loguru 风格行格式(`LoguruFormat`:本地时间 | 级别 | 来源 - 正文)+
//!    `RawMessageFields` 不转义 `message`,使事件日志里嵌入的 ANSI 颜色生效。
//! 3. **文件层**（可选):当 [`LogConfig::file`] 为 `Some` 时,挂一个
//!    `tracing-appender` 的按天滚动、非阻塞文件层。其后台写线程靠一个
//!    `WorkerGuard` 维持——该 guard 被装进返回的 [`LogGuard`],**调用方必须持有它**
//!    （否则进程退出/drop 时尾部日志可能丢失)。
//! 4. **日志总线层**（可选):当 [`LogConfig::bus`] 为 `true` 时,挂一个
//!    [`LogBusLayer`](crate::LogBusLayer),把（按来源过滤后、不低于
//!    [`LogConfig::bus_min_level`] 的、非保留 `target` 的)日志记录广播到一个
//!    `broadcast` 总线。`init` 把配套的 [`LogBus`](crate::LogBus) 一并返回,业务用
//!    [`on_record`](crate::on_record) 消费、持久化。该总线与消息事件 / handler 路径分离,
//!    天然防回环。
//!
//! [`init`] 把组装好的订阅者设为全局默认,返回 [`LogGuard`] 与（启用时的)[`LogBus`]。
use std::io::IsTerminal;
use std::path::PathBuf;

use tracing::Level;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;

use crate::bus::{LogBus, LogBusLayer};

/// 日志总线 `broadcast` 环形缓冲的默认容量。订阅者落后超过它会收到 `Lagged`。
const DEFAULT_BUS_CAPACITY: usize = 1024;

/// 统一记录器配置。`Default` 给出「控制台、`info`、终端则带颜色、非 JSON、无文件」的常用起点。
#[derive(Debug, Clone)]
pub struct LogConfig {
    /// 来源过滤指令（[`EnvFilter`] 语法）。**当 `RUST_LOG` 存在时以它为准**，本字段
    /// 作为缺省。形如 `"info"` 或 `"nagisa::onebot=warn,nagisa::event=info"`。
    pub filter: String,
    /// 滚动文件的「目录/前缀」。`Some(path)` 时按天滚动写入 `path` 的父目录、以其文件名
    /// 为前缀（实际文件名带日期后缀）；`None` 时不写文件。
    pub file: Option<PathBuf>,
    /// 是否以 JSON 行格式输出（便于机器采集）。
    pub json: bool,
    /// 控制台是否带 ANSI 颜色。默认按 stdout 是否为终端自动判定(fmt 层的 `with_ansi`
    /// 是写死的开关、**无**自动检测,这里不判,重定向/管道/journald 下转义码就会原样进文件)。
    /// 显式覆盖时注意与 [`EventLog::color`](crate::EventLog::color) 一起调——行框架与事件
    /// 正文的着色判据不同源的话,会出现「彩色框架 + 纯文本正文」的半着色行。
    pub ansi: bool,
    /// 是否启用「日志即事件」总线。`true` 时 [`init`] 额外挂一个
    /// [`LogBusLayer`](crate::LogBusLayer) 并返回一个 [`LogBus`](crate::LogBus)；
    /// `false`（默认）时 `init` 返回的 `LogBus` 为 `None`。
    pub bus: bool,
    /// 转发到日志总线的最低级别（默认 `INFO`）。低于它的记录不进总线，防洪泛。
    /// 仅当 [`Self::bus`] 为 `true` 时生效。
    pub bus_min_level: Level,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            filter: "info".to_string(),
            file: None,
            json: false,
            // 终端才上色(与 EventLog 的默认判定同口径);重定向/管道下自动转纯文本。
            ansi: std::io::stdout().is_terminal(),
            bus: false,
            bus_min_level: Level::INFO,
        }
    }
}

/// [`init`] 的返回值。**持有它直到进程结束**：当配置了文件层时，它拥有
/// `tracing-appender` 的后台写线程 `WorkerGuard`——一旦 drop，后台线程会 flush 并退出，
/// 之后的文件日志将丢失。无文件层时它只是一个空壳。
#[must_use = "drop 掉 LogGuard 会停掉文件日志的后台写线程并可能丢失尾部日志；请持有它直到退出"]
pub struct LogGuard {
    _file: Option<WorkerGuard>,
}

/// 解析过滤指令：`RUST_LOG` 优先，否则用 `cfg.filter`；两者都解析失败时退回 `info`，
/// 保证 `init` 永不因脏指令而 panic。
fn build_filter(cfg: &LogConfig) -> EnvFilter {
    // try_from_default_env 读 RUST_LOG；缺失或不可解析则回落到 cfg.filter，再回落到 info。
    EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(&cfg.filter))
        .unwrap_or_else(|_| EnvFilter::new("info"))
}

/// 把「目录/前缀」拆成 `tracing-appender::rolling::daily` 需要的 (目录, 文件名前缀)。
/// 无父目录时落到当前目录 `.`；无文件名时用 `nagisa.log` 兜底。
fn split_file_path(path: &std::path::Path) -> (PathBuf, PathBuf) {
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).map_or_else(|| PathBuf::from("."), PathBuf::from);
    let prefix =
        path.file_name().map_or_else(|| PathBuf::from("nagisa.log"), PathBuf::from);
    (dir, prefix)
}

/// 安装统一记录器：控制台层 +（可选）滚动文件层 +（可选）日志总线层 + 来源过滤，并设为
/// 全局默认订阅者。
///
/// 返回 `(LogGuard, Option<LogBus>)`：
/// - [`LogGuard`] 必须由调用方持有到进程退出（详见其文档）。
/// - [`LogBus`] 仅当 [`LogConfig::bus`] 为 `true` 时为 `Some`——把它交给
///   [`on_record`](crate::on_record) 即可在后台持久化日志。
///
/// 全局默认订阅者**只能设一次**；重复调用 `init` 会 panic（这是 `tracing` 的全局约束，
/// 不是本函数的额外限制）。测试请改用 `tracing::subscriber::with_default` 临时安装。
pub fn init(cfg: LogConfig) -> (LogGuard, Option<LogBus>) {
    let filter = build_filter(&cfg);

    // 控制台 fmt 层：json/ansi 二选一的格式器，但都走同一条 stdout。
    // 两个分支类型不同，故各自 boxed 成 trait object 再塞进 registry。
    // 非 JSON 分支：loguru 风格行格式（本地时间 | 级别 | 来源 - 正文，见 `format` 模块）
    // + RawMessageFields（不转义 message，使事件日志里嵌入的 ANSI 颜色生效——默认字段
    // 格式器会把颜色码转义成字面 `\x1b…`）。
    let console = if cfg.json {
        fmt::layer().json().with_ansi(false).boxed()
    } else {
        fmt::layer()
            .event_format(crate::format::LoguruFormat)
            .fmt_fields(crate::fields::RawMessageFields)
            .with_ansi(cfg.ansi)
            .boxed()
    };

    // 文件层可选：Some 时返回 (层, guard)；None 时返回 (None, None)。
    let (file_layer, file_guard) = match &cfg.file {
        Some(path) => {
            let (dir, prefix) = split_file_path(path);
            let appender = tracing_appender::rolling::daily(dir, prefix);
            let (writer, guard) = tracing_appender::non_blocking(appender);
            // 文件层永不带颜色（ANSI 转义写进文件会成乱码）。同一套 loguru 行格式
            // （`with_ansi(false)` → 整行纯文本）+ RawMessageFields，与控制台口径一致；
            // 文件场景下事件日志应配 EventLog::color(false)，message 即为纯文本。
            let layer = fmt::layer()
                .event_format(crate::format::LoguruFormat)
                .fmt_fields(crate::fields::RawMessageFields)
                .with_ansi(false)
                .with_writer(writer)
                .boxed();
            (Some(layer), Some(guard))
        }
        None => (None, None),
    };

    // 日志总线层可选：Some 时返回 (层, bus 工厂)；None 时不挂层、不返回 bus。
    let (bus_layer, bus) = if cfg.bus {
        let (layer, bus) = LogBusLayer::new(DEFAULT_BUS_CAPACITY, cfg.bus_min_level);
        (Some(layer), Some(bus))
    } else {
        (None, None)
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(console)
        .with(file_layer)
        .with(bus_layer)
        .init();

    (LogGuard { _file: file_guard }, bus)
}
