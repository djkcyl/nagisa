//! Nagisa 的日志层:事件渲染 + 可选观察者 + 名称缓存 +
//! 文件/控制台输出 + 日志总线。
//!
//! `nagisa-core` 只打机制/协议日志(连接、分发、调用),不渲染事件内容;本 crate 补上
//! 「把每个事件渲染成可读中文一行」这件事,并把整套日志输出装好。一次 [`init`] 即得
//! 控制台 +(可选)滚动文件;一个 [`EventLog`] 挂到 `App::on_top` 即得可读事件流。
//!
//! 三件事正交,各管各的:
//! 1. **可读事件日志**:纯函数 [`render`]（`&Event -> String`,无 IO)+ 可选观察者
//!    [`EventLog`]（运行时开关 + 按种类过滤,挂 `App::on_top`,渲染时把群号/QQ 号解析成名字)。
//! 2. **统一输出** [`init`]:控制台 +(可选)按天滚动文件 + 按来源/`target` 过滤,设为
//!    全局 `tracing` 订阅者。返回的 [`LogGuard`] 须持有到进程退出。
//! 3. **日志总线** [`LogBus`]/[`LogBusLayer`]:一个 `tracing` 层把已过滤的日志记录广播到
//!    `broadcast` 总线,业务用 [`on_record`] 持久化。总线与消息事件 / handler 路径分离,且对
//!    保留 `target`（[`RESERVED_TARGET_PREFIX`]）与低级别记录设闸,防回环 / 防洪泛。
//!
//! 模块地图(以各自的公开项为锚):
//! - `render`([`render`] / [`render_line`]):把 [`Event`] 渲染成一行可读中文。
//! - `observer`([`EventLog`]):观察者——把事件渲染成日志发到 `nagisa::event`。
//! - `names`([`NameStore`]):名称缓存——就地学习 + Bot API 回填,把号解析成名字。
//! - `messages`([`MessageStore`]):最近消息缓存——给撤回通知补「撤回了什么」。
//! - `init`([`init`] / [`LogConfig`]):统一记录器,装控制台/文件/总线层与来源过滤。
//! - `bus`([`LogBus`] / [`on_record`]):日志总线——把日志记录广播给业务持久化。
//! - `format`(crate 内部):loguru 风格行格式器——`本地时间 | 级别 | 来源 - 正文`。
//! - `fields`(crate 内部):控制台字段格式器,不转义 `message`,使事件日志里嵌入的
//!   ANSI 颜色生效。
//!
//! 接入:业务只依赖 `nagisa` 门面,开 `features = ["log"]` 后经 `nagisa::log` 用本 crate。
#![forbid(unsafe_code)]

mod bus;
mod fields;
mod format;
mod init;
mod messages;
mod names;
mod observer;
mod render;

pub use bus::{
    on_record, LogBus, LogBusLayer, LogBusReceiver, LogRecord, RESERVED_TARGET_PREFIX,
};
pub use init::{init, LogConfig, LogGuard};
pub use messages::{MessageStore, StoredMessage};
pub use names::NameStore;
pub use observer::{EventLog, EventLogHandle};
pub use render::{render, render_line, render_segments, RenderOpts, Style};

/// 便捷再导出:业务调用 [`render`] 时常需引用 [`Event`]。
pub use nagisa_types::event::Event;
