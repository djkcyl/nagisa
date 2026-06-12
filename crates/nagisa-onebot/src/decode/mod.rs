//! 把 OneBot v11 wire 事件/段解码成统一的 `nagisa` 类型。
//!
//! 金科玉律:**绝不丢弃、绝不 panic**。建模不了的东西都变成 `Event::Raw` / `Segment::Raw`,
//! 并保留原始载荷。[`decode_event`] 是顶层入口;[`decode_event_batch`] /
//! [`decode_event_value`] 覆盖 LLOneBot 拉取路径(`get_event` 数组、SSE `data:` 帧)。
//!
//! 本文件持有共享脚手架(`PROTO`、`event_raw_value`、`raw_event`、`post_type` 分发),
//! 各类事件分派给子模块:`message`(消息事件 + `sender` 合成)、`notice`(group/reaction/notify
//! 等 notice)、`meta_request`(friend/group 请求 + lifecycle/heartbeat meta)、`segment`
//! (段 / CQ 字符串解码)。
use crate::wire::{value_as_string, RawEventJson, WireMessage, WireSegment};
use nagisa_types::event::{
    EmojiLike, FlashFilePhase, HonorKind, MemberDecreaseReason, NudgeDisplay, OnlineFileDirection, RawEvent,
    ReactionKind,
};
use nagisa_types::prelude::*;
use nagisa_types::segment::{ContactKind, Forward, ForwardNode, MusicShare};
use serde_json::{Map, Value};

mod message;
mod meta_request;
mod notice;
mod segment;

use message::decode_message;
use meta_request::{decode_meta, decode_request};
use notice::decode_notice;
pub use segment::{decode_cq_string, decode_message_value, decode_segments};

const PROTO: Protocol = Protocol::OneBot11;

/// 把 wire 事件重新序列化成 `Value`,供 `raw` 字段 / `Event::Raw` 使用。`RawEventJson` 现在
/// 派生了 `Serialize`,故这是一次完整 round-trip。
fn event_raw_value(ev: &RawEventJson) -> Value {
    serde_json::to_value(ev).unwrap_or(Value::Null)
}

/// 把一个顶层 wire 事件解码成统一的 `Event`。设计上不会失败。
pub fn decode_event(ev: RawEventJson) -> Event {
    match ev.post_type.as_deref() {
        Some("message") | Some("message_sent") => decode_message(ev),
        Some("notice") => decode_notice(ev),
        Some("request") => decode_request(ev),
        Some("meta_event") => decode_meta(ev),
        _ => raw_event(&ev, "unknown"),
    }
}

fn raw_event(ev: &RawEventJson, fallback_kind: &str) -> Event {
    let kind = ev.post_type.clone().or_else(|| ev.notice_type.clone()).unwrap_or_else(|| fallback_kind.to_string());
    Event::Raw(RawEvent { protocol: PROTO, kind, raw: event_raw_value(ev) })
}

/// 把 LLOneBot `get_event` 长轮询响应的 `data` 载荷解码成一批统一的 [`Event`]。wire 形态通常是
/// OneBot 事件对象的 JSON **数组**;也兼容单个对象(解码成一条事件)与 `null`/空(→ 空 `Vec`)。
/// 每个元素都过 [`decode_event`],故畸形/未知元素降级为 `Event::Raw`,而非被丢弃或 panic。
/// 设计上不会失败。
pub fn decode_event_batch(data: Value) -> Vec<Event> {
    match data {
        Value::Array(items) => items.into_iter().map(decode_event_value).collect(),
        Value::Null => Vec::new(),
        // 裸对象:当成单条事件。
        other @ Value::Object(_) => vec![decode_event_value(other)],
        // 其余(string/number/bool)不可能是事件 → 浮现为 Raw。
        other => vec![Event::Raw(RawEvent { protocol: PROTO, kind: "undecodable".to_string(), raw: other })],
    }
}

/// 解码单个事件 `Value`(来自长轮询批或 SSE `data:` 帧)。解不成 `RawEventJson` 的值降级为
/// 携带原始载荷的 `Event::Raw`——绝不丢弃、绝不 panic。
pub fn decode_event_value(v: Value) -> Event {
    match serde_json::from_value::<RawEventJson>(v.clone()) {
        Ok(ev) => decode_event(ev),
        Err(_) => Event::Raw(RawEvent { protocol: PROTO, kind: "undecodable".to_string(), raw: v }),
    }
}
