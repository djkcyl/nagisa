//! `nagisa-onebot`：OneBot v11 协议适配器。
//!
//! 主要面向 Lagrange.OneBot,同时兼容 NapCat / LLOneBot 的各种细节差异。对外只暴露
//! [`OneBotAdapter`],它同时实现入站事件流 [`nagisa_core::EventSource`] 与出站动作 API
//! [`nagisa_core::adapter::ActionInvoker`]。一个适配器实例承载收发两个方向;
//! 统一的 [`OneBotConfig`] 选定传输模式。
//!
//! # 传输模式
//!
//! 所有模式跑同一条 `decode → sink` 入站管线、同一条 `encode → call` 出站路径,差别只在
//! 字节怎么搬。由 [`OneBotTransport`] 选定:
//!
//! ```text
//! 模式              事件入站经                动作出站经              nagisa 角色   备注
//! ───────────────── ──────────────────────── ─────────────────────── ────────── ──────────────────────────
//! Forward           正向 WS(单 socket)      同一 socket,echo 关联    WS 客户端   默认;重连 + idle 看门狗
//! ReverseWs         accept 的 WS(按角色)    server_outbound socket   WS 服务端   X-Client-Role 分流信道
//! Http              HTTP-POST webhook        HTTP-API POST(无 echo)   HTTP 两侧   X-Signature(hmac-sha1)
//! HttpApi           —(无;空转)            HTTP-API POST            HTTP 客户端 配独立事件源成对使用
//! LLOneBotHttp      SSE /_events | get_event HTTP-API POST            HTTP 客户端 LLOneBot 拉取式事件
//! ```
//!
//! 正向 / 反向 WS 在单 socket 上做 demux:带 `echo` 字段的帧是动作响应(解决一个挂起的
//! [`oneshot`](tokio::sync::oneshot)),其余皆为事件。HTTP 模式无 echo 关联——动作响应就是
//! POST 的正文。
//!
//! # 厂商处理
//!
//! 厂商(Lagrange / NapCat / LLOneBot / 其他)在连上后由 `get_version_info` 探得,经
//! [`ActionInvoker::vendor`](nagisa_core::adapter::ActionInvoker::vendor) 暴露。三条刻意的策略
//! 吸收跨厂商差异:
//!
//! - **宽松 wire 类型**([`wire`]):绝不 `deny_unknown_fields`;缺字段降级为 `None`/默认值,
//!   解不开的帧降级为 `Event::Raw`(见 [`decode`])。
//! - **解码与厂商无关;*契约* 只在适配器边缘施加一次**
//!   ([`adapter::prepare_inbound`](adapter)):丢弃被框架统一信号取代的帧(OneBot
//!   `lifecycle.connect`,与事件源已发的 `Meta::Connect` 重复),并施加按厂商的 `@` 提及名
//!   归一化。所有入站路径(正/反向 WS、HTTP-POST、SSE、长轮询)都经它,故规则只此一处。
//! - **自身消息原样透传**:消息解码时带上 `is_self`(来自 `post_type: message_sent` 或
//!   `user_id == self_id`)后原样转发——适配器从不过滤,这条策略留给消费方。
//! - **出站别名 / 双拼写回退**(动作面里 [`adapter`] 的 `OneBotActions` impl):当各端给同一
//!   语义动作起了不同名字时,`call_alias` 在 `Unsupported` 上改用备名重试,或一次同发两种参数
//!   拼写(如群文件移动)。
//!
//! # 模块
//!
//! - [`wire`]:宽松 serde wire 类型(事件封包、消息段、响应)。
//! - [`decode`]:wire → 统一 `nagisa_types::Event` / `Segment`(绝不丢弃、绝不 panic;未知载荷
//!   降级为 `Event::Raw` / `Segment::Raw`)。
//! - [`encode`]:统一 `Segment` → OneBot wire 段数组。
//! - [`adapter`]:[`OneBotAdapter`] 本体——配置、正向 WS 运行循环、echo 关联、响应映射、
//!   入站契约,以及两个 trait(完整的 `OneBotActions` 动作面)。
//! - [`reverse_ws`]:`ReverseWs` 模式的反向 WS **服务端**循环。
//! - [`http_post`]:HTTP-POST webhook,以及 `Http` / `LLOneBotHttp` 模式下 LLOneBot 的拉取式
//!   事件源(SSE / 长轮询)。
#![forbid(unsafe_code)]

pub mod adapter;
pub mod decode;
pub mod encode;
pub mod http_post;
pub mod reverse_ws;
pub mod wire;

pub use adapter::{LLOneBotEventMode, OneBotAdapter, OneBotConfig, OneBotTransport};
pub use decode::{decode_cq_string, decode_event, decode_event_batch, decode_event_value, decode_segments};
pub use encode::encode_segments;
