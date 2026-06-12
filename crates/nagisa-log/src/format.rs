//! loguru 风格的行格式器:`时间 | 级别 | 来源 - 正文`。
//!
//! 取代 `tracing-subscriber` 的默认行格式——后者打 UTC ISO 长串时间戳(与本地时间差着
//! 时区,且 27 列宽),来源列长短不齐,整屏难对齐。本格式器:
//!
//! ```text
//! 2026-06-07 23:12:37.164 | INFO     | nagisa::event - [群 …] 张三(10001): 这是一条示例消息
//! 2026-06-07 23:12:40.178 | WARN     | nagisa::dispatch - handler 出错 plugin=sign
//! ```
//!
//! - **时间**:本地时区、毫秒精度、绿色——与消息正文里的时间(签到/转账文案)同一口径,
//!   不再出现「日志戳 15:12、正文 23:12」的时区错位。
//! - **级别**:左对齐补到 8 列(loguru 口径),按级别着色;`WARN`/`ERROR` 连**正文**一起
//!   染成级别色,扫一眼就能从信息流里跳出来。
//! - **来源**:`target` 完整保留、青色——`RUST_LOG=nagisa::event=off` 之类过滤指令照抄即可。
//! - **span 作用域**:正文前以暗色补上 `event{kind=… peer=…}: ` 一段——dispatch 给每个
//!   事件的 handler 任务都套了这样一个 `debug_span`,专为并发分发时把同一事件的日志串起来;
//!   丢掉它,`RUST_LOG=…=debug` 下这些关联字段就没了。无 span 时零输出。
//! - **着色随 writer 走**:层的 `with_ansi(..)` 说了算(文件层 false → 整行纯文本),
//!   本格式器不自带开关。
//!
//! 字段渲染仍由 [`RawMessageFields`](crate::fields::RawMessageFields) 负责(`message`
//! 不转义,事件日志内嵌的 ANSI 颜色原样生效;其余字段 `key=value`)。
use std::fmt;

use chrono::Local;
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields, FormattedFields};
use tracing_subscriber::registry::LookupSpan;

/// ANSI 复位。
const RESET: &str = "\x1b[0m";
/// 时间戳:绿(loguru 口径)。
const C_TIME: &str = "\x1b[32m";
/// 来源 `target`:青(loguru 口径)。
const C_TARGET: &str = "\x1b[36m";
/// 分隔符 `|` / `-`:暗淡,把视线让给正文。
const C_SEP: &str = "\x1b[2m";

/// 级别的 ANSI 色:TRACE 暗青 / DEBUG 蓝 / INFO 粗体 / WARN 黄 / ERROR 粗红(loguru 口径)。
fn level_color(level: Level) -> &'static str {
    match level {
        Level::TRACE => "\x1b[2;36m",
        Level::DEBUG => "\x1b[34m",
        Level::INFO => "\x1b[1m",
        Level::WARN => "\x1b[33m",
        Level::ERROR => "\x1b[1;31m",
    }
}

/// loguru 风格行格式器(见模块文档)。挂法:`fmt::layer().event_format(LoguruFormat)`。
pub(crate) struct LoguruFormat;

impl<S, N> FormatEvent<S, N> for LoguruFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(&self, ctx: &FmtContext<'_, S, N>, mut writer: Writer<'_>, event: &Event<'_>) -> fmt::Result {
        let meta = event.metadata();
        let level = *meta.level();
        let target = meta.target();
        let time = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let ansi = writer.has_ansi_escapes();

        if ansi {
            let lv = level_color(level);
            write!(
                writer,
                "{C_TIME}{time}{RESET} {C_SEP}|{RESET} {lv}{:<8}{RESET} {C_SEP}|{RESET} \
                 {C_TARGET}{target}{RESET} {C_SEP}-{RESET} ",
                level.as_str()
            )?;
        } else {
            write!(writer, "{time} | {:<8} | {target} - ", level.as_str())?;
        }

        // span 作用域(见模块文档):自根到叶逐个写 `名{字段}: `,暗色——它是关联线索,
        // 不该与正文争夺视线。字段串由本层的字段格式器在 span 创建时记好(FormattedFields);
        // RawMessageFields 给非 message 字段一律前置空格,故 trim_start 后再包大括号。
        if let Some(scope) = ctx.event_scope() {
            for span in scope.from_root() {
                if ansi {
                    write!(writer, "{C_SEP}")?;
                }
                write!(writer, "{}", span.name())?;
                let ext = span.extensions();
                if let Some(fields) = ext.get::<FormattedFields<N>>() {
                    if !fields.fields.is_empty() {
                        write!(writer, "{{{}}}", fields.fields.trim_start())?;
                    }
                }
                write!(writer, ": ")?;
                if ansi {
                    write!(writer, "{RESET}")?;
                }
            }
        }

        // WARN/ERROR 连正文一起染级别色(loguru 口径)。事件日志(message 内嵌 ANSI、自带
        // 复位符)几乎总在 INFO 发出,不走这支;框架自身的 warn/error 正文无内嵌色,染色安全。
        // 万一哪行 WARN 真嵌了颜色,内层复位至多让外层色提前结束——失色,不乱码。
        if ansi && (level == Level::WARN || level == Level::ERROR) {
            write!(writer, "{}", level_color(level))?;
            ctx.format_fields(writer.by_ref(), event)?;
            write!(writer, "{RESET}")?;
        } else {
            ctx.format_fields(writer.by_ref(), event)?;
        }
        writeln!(writer)
    }
}
