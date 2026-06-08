//! `nagisa-milky`：Milky 1.2 协议适配器（主要面向 Lagrange.Milky，兼顾 LLOneBot 等更全实现）。
//!
//! [`MilkyAdapter`] 是唯一对外实体，同时实现两侧能力：
//! - **出站动作**（[`nagisa_core::adapter::ActionInvoker`] + 协议族 trait）：经 HTTP `POST`
//!   `{api_base}{action}` 调用，无 echo、动作名在 URL 路径里。
//! - **入站事件**（[`nagisa_core::EventSource`]）：从协议端接收 Milky 事件封包，decode 成统一
//!   `nagisa-types` 的 [`Event`](nagisa_types::event::Event) 推入 sink。
//!
//! ## 三条入站路径
//! 事件可经三条互斥路径之一进入，由 [`MilkyConfig`] 选定：
//! - **ws**（[`MilkyMode::Ws`]，默认）：nagisa 主动连协议端 `/event`（WebSocket Upgrade），
//!   逐文本帧 decode；自带 keepalive Ping 与 idle 看门狗，断线指数退避重连。
//! - **sse**（[`MilkyMode::Sse`]）：nagisa 主动 `GET /event`（`text/event-stream`，不 Upgrade），
//!   增量 chunk → SSE 解析凑齐事件 payload；与 ws 共用 idle 看门狗与重连。
//! - **webhook**（[`MilkyConfig::with_webhook`]）：反向信道——nagisa 自己起 axum 服务端，协议端
//!   `POST` 事件到指定 path。这是独立运行路径，**优先于** `mode` 的出站 ws/sse；它不重连
//!   （服务端常驻至 shutdown），但与 ws/sse 共享同一条 decode→sink 漏斗。
//!
//! ws/sse/webhook 三条入站路径最终都汇入 `MilkyAdapter::dispatch_event`（结构性破坏降级为
//! `Event::Raw`，绝不丢弃），日志在同一 `nagisa::wire` target 上一处覆盖收发两端。
//!
//! ## 与 OneBot 适配的关系
//! 出站动作分布在三个 trait 上：通用的 [`ActionInvoker`](nagisa_core::adapter::ActionInvoker)、
//! 协议专属的 [`MilkyActions`](nagisa_core::adapter::MilkyActions) 与
//! [`OneBotActions`](nagisa_core::adapter::OneBotActions)。后两者的方法默认实现全部返回
//! `Error::Unsupported`。`MilkyAdapter` 实现 `ActionInvoker` 与 `MilkyActions` 的真实映射，
//! 而对 `OneBotActions` 只留一个**空 impl**——所有 OneBot 独有/厂商扩展动作经默认实现走
//! `Unsupported`。OneBot 适配器对称地反过来（实现 `OneBotActions`、空 impl `MilkyActions`）。
//! 因此跨协议方法天然降级、互不污染，调用方据 [`supports()`](nagisa_core::adapter::ActionInvoker::supports)
//! 与返回的 `Unsupported` 降级。
//!
//! ## 标准 vs Lagrange 覆盖
//! 动作映射按 **Milky 1.2 标准 IR** 实现全集（含群管理 / 请求处理 / 文件 / 转发 / 资料）。
//! Lagrange.Milky 是**稀疏实现**：标准集里的不少动作在 wire 上返回 HTTP 404 → `call` 归一为
//! `Error::Unsupported`。连接时探测到的 impl_info 若表明是 Lagrange，`supports()` 会对这些
//! 已知缺口诚实返回 `false`；LLOneBot 等覆盖更全的实现则保持乐观。
//!
//! ## 模块布局
//! - `config`（私有）：[`MilkyConfig`] / [`MilkyMode`] 配置实体与 builder。
//! - [`transport`]：[`MilkyAdapter`] 本体——构造、URL 推导、`call` 动作通道、retcode 归类、
//!   协议日志;并 re-export `MilkyConfig`/`MilkyMode` 保持 `transport::{…}` 路径稳定。
//! - `sources`（私有）：[`EventSource`](nagisa_core::EventSource) 实现 + ws/sse pump
//!   骨架（`FrameSource`/`WsSource`/`SseSource`）+ idle 看门狗 + 重连。
//! - [`webhook`]：反向信道的 axum 接收端。
//! - `actions`（私有）：`ActionInvoker` / `MilkyActions` / `OneBotActions` 在
//!   `MilkyAdapter` 上的实现。
//! - [`wire`]：Milky wire 类型（宽松 serde；incoming/outgoing 段双结构）。
//! - [`decode`]：Milky 事件/段/实体 → 统一 `nagisa-types`（未知 → `Raw`，绝不丢弃）。
//! - [`encode`]：统一 `Segment` → Milky `OutgoingSegment`（资源 → 单 `uri`）。
#![forbid(unsafe_code)]
// 溯源注释里特意保留裸 URL(OFFICIAL:/ENDPOINT: 行,非给 rustdoc 渲染的链接);
// 不为它们刷一片 bare_urls 告警、淹没真问题。
#![allow(rustdoc::bare_urls)]

mod actions;
mod config;
pub mod decode;
pub mod encode;
mod sources;
pub mod transport;
pub mod webhook;
pub mod wire;

pub use transport::{MilkyAdapter, MilkyConfig, MilkyMode};
