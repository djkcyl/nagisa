//! 控制台字段格式器:像默认一样渲染字段,但**不转义 `message`**——使事件日志里嵌入的
//! ANSI 颜色能原样写到终端。
//!
//! 背景:`tracing-subscriber` 默认会把字段值里的控制字符（含 ESC `\x1b`）转义成可见的
//! 字面 `\x1b…`（防「终端转义注入」)。可读事件日志([`crate::EventLog`])把整行连同 ANSI
//! 颜色塞进 `message` 字段,于是默认格式器会把颜色码变成一堆字面 `\x1b[..m`,终端看到的是
//! 乱码而非颜色。本格式器对 `message` 原样 `Display`(不转义),对其余字段照常 `key=value`,
//! 既让事件日志的颜色生效,又不影响结构化字段。框架自身的日志（dispatch/onebot/…）的字段
//! 值都不含控制字符,故「不转义」对它们是无副作用的恒等操作。
use std::fmt;

use tracing::field::{Field, Visit};
use tracing_subscriber::field::RecordFields;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::FormatFields;

/// 不转义 `message` 的字段格式器（见模块文档）。挂到非 JSON 的 fmt 层上：
/// `fmt::layer().fmt_fields(RawMessageFields)`。
pub(crate) struct RawMessageFields;

impl<'writer> FormatFields<'writer> for RawMessageFields {
    fn format_fields<R: RecordFields>(
        &self,
        mut writer: Writer<'writer>,
        fields: R,
    ) -> fmt::Result {
        let mut visitor = RawVisitor { writer: &mut writer, result: Ok(()) };
        fields.record(&mut visitor);
        visitor.result
    }
}

struct RawVisitor<'a, 'writer> {
    writer: &'a mut Writer<'writer>,
    result: fmt::Result,
}

impl Visit for RawVisitor<'_, '_> {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if self.result.is_err() {
            return;
        }
        self.result = if field.name() == "message" {
            // `message` 原样写出:其底层是 `format_args!` 的 `Arguments`,其 `Debug` 即原串,
            // 不会转义控制字符,故嵌入的 ANSI 颜色能原样落到终端。
            write!(self.writer, "{value:?}")
        } else {
            // 其余字段照常 `key=value`（紧跟 message 之后，前置一个空格分隔）。
            write!(self.writer, " {}={:?}", field.name(), value)
        };
    }
}
