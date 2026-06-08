//! nagisa 的跨协议域模型：QQ 机器人框架各层共享的事件、消息段、实体、Id、错误、能力、
//! 资源类型。本 crate 只放数据形态和纯函数式便捷方法，无业务逻辑、无 I/O、不依赖 anyhow。
//!
//! 在 workspace 里的位置：适配器（`nagisa-onebot` / `nagisa-milky`）把各自协议的 JSON 翻译
//! 进出这些类型，`nagisa-core` 用它们驱动 `Bot` 句柄与事件分发，业务插件只接触本 crate 的
//! 类型，永不接触协议结构。
//!
//! 模块地图：
//! - [`error`]：统一 [`Error`](error::Error) / [`Result`](error::Result) 与 [`bail!`] 宏；
//!   业务看不到 retcode / HTTP status / 协议结构。
//! - [`context`]：[`Context`](context::Context) 扩展 trait（`.context()` / `.with_context()`），
//!   把任意 `Display` 错误或 `Option` 归一到统一 [`Result`](error::Result)。
//! - [`capability`]：[`Protocol`](capability::Protocol) 与可探测的 [`Capability`](capability::Capability)。
//! - [`vendor`]：OneBot 实现端厂商 [`Vendor`]（按 `app_name` 判定，用于动作名 aliasing）。
//! - [`id`]：[`Uin`](id::Uin) / [`Scene`](id::Scene) / [`Peer`](id::Peer) / [`MessageId`](id::MessageId)。
//! - [`resource`]：媒体资源的发送来源与接收引用。
//! - [`segment`]：统一消息段 [`Segment`](segment::Segment)。
//! - [`message`]：消息体 [`Message`](message::Message)（即 `Vec<Segment>`）、查询/变换扩展
//!   [`MessageExt`](message::MessageExt)、流式构造器 [`Msg`](message::Msg)。
//! - [`entity`]：好友 / 群 / 成员 / 文件等实体（取各协议字段并集，缺的为 `Option`）。
//! - [`event`]：统一事件 [`Event`](event::Event)（消息 / 通知 / 请求 / 元 / 逃生口）。
//!
//! 关键设计：
//! - **不绑协议**：所有类型按语义建模，不带任何 wire 细节；各协议的私有/未知形态有逃生口
//!   （[`Segment::Raw`](segment::Segment::Raw) / [`Event::Raw`](event::Event::Raw) / 实体的 `raw` 字段），
//!   适配器解码时绝不丢弃未知数据，绝不 panic。
//! - **基于 `Display` 的错误上下文**：[`Context`](context::Context) 是扩展 trait 而非
//!   `From<E: Display>`——后者与既有 `#[from]` 实现相干性冲突，方法式适配是唯一相干安全的形态。
//! - **收发同一套 `Segment` / `Message`**：业务用同一组消息段收发，wire 级的入/出差异由适配器
//!   私有承担；`Message` 就是 `Vec<Segment>`，便捷方法挂在 [`MessageExt`](message::MessageExt) 扩展 trait 上。
#![forbid(unsafe_code)]
// 溯源注释里特意保留裸 URL(OFFICIAL:/ENDPOINT: 行,非给 rustdoc 渲染的链接);
// 不为它们刷 bare_urls 告警、淹没真问题。
#![allow(rustdoc::bare_urls)]
pub mod error;
pub mod context;
pub mod capability;
pub mod vendor;
pub mod entity;
pub mod id;
pub mod resource;
pub mod message;
pub mod segment;
pub mod event;

pub use vendor::Vendor;

/// 常用类型的便捷导入：`use nagisa_types::prelude::*;`（业务侧通常经 `nagisa::prelude::*` 间接拿到）。
pub mod prelude {
    pub use crate::capability::{Capability, Protocol};
    pub use crate::entity::{
        AiCharacter, AiCharacterGroup, Announcement, Business, EmojiLiker, EssenceMessage,
        FileFetch, FileMeta, ForwardSendResult, FriendCategory, FriendCategoryList, FriendGroup,
        FriendInfo, FriendStatus, GroupFileList, GroupFolder, GroupInfo, HonorList, HonorMember,
        ImplStat, ImplStatus, MemberInfo, OcrText, ProfileLiker, Rkey, Role, Sex, UserInfo,
        VersionInfo,
    };
    pub use crate::context::Context;
    pub use crate::error::{ActionErrorKind, Error, Result, TransportError};
    pub use crate::event::{
        Anonymous, Event, HonorKind, MemberDecreaseReason, Meta, MessageEvent, MessageStyle,
        Notice, ReactionKind, Request, RequestState, RequestToken,
    };
    pub use crate::id::{MessageId, Peer, Scene, Uin};
    pub use crate::message::{Message, MessageExt, Msg};
    pub use crate::resource::{Media, ResourceRef, ResourceSource};
    pub use crate::segment::{
        ContactKind, Forward, ForwardNode, ImageSubType, MusicShare, Segment,
    };
}
