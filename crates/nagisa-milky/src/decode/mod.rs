//! Milky wire → 统一 `nagisa-types` 的 decode（入站）。
//!
//! [`decode_event`] 把一条事件文本帧解析成统一 [`Event`]，是 ws/sse/webhook 三条入站路径的
//! 共同收口（经 `MilkyAdapter::dispatch_event` 调用）：
//! - `message_receive` → [`Event::Message`]（按 `message_scene` 分发，**向上 propagate serde
//!   错误**——只有这类结构性破坏才返回 `Err`，由调用方降级为 `Event::Raw`）。
//! - 其余 19 个 event_type 尽可能映射到 [`Notice`] / [`Request`]，未识别字段宽松取值。
//! - 未知 event_type / segment → [`Event::Raw`] / `Segment::Raw{protocol: Milky}`，绝不丢弃。
//!
//! 另含一组 `pub` 辅助（`message_event_from_incoming` / `*_from_value` /
//! `notification_to_request` / `notification_to_notice` / 实体映射 等），供 `actions` 模块把
//! 动作响应体 decode 回统一类型，与事件路径共用同一套映射（口径一致）。所有 wire 字段缺失
//! 均降级，绝不 panic。
//!
//! 本文件是脚手架：共享 `Value` 取值小工具、`raw` 降级器、`decode_event` / `decode_envelope`
//! 顶层分发，并按事件家族分派子模块——`message`（消息事件 + 转发节点）、`segment`（消息段）、
//! `notice`（撤回/精华/置顶/戳一戳 等 Notice）、`request`（好友/入群/邀请请求 + 非请求群通知
//! 透出）、`entity`（实体 → `nagisa-types` 映射 + 杂项辅助）。
use nagisa_types::entity::{FriendCategory, FriendInfo, GroupInfo, MemberInfo, Role, Sex};
use nagisa_types::event::RequestToken as Token;
use nagisa_types::event::RequestTokenInner;
use nagisa_types::event::{
    EmojiLike, Event, MemberDecreaseReason, MessageEvent, Notice, NudgeDisplay, RawEvent,
    ReactionKind, Request, RequestState,
};
use nagisa_types::id::{MessageId, Peer, Scene, Uin};
use nagisa_types::prelude::Protocol;
use nagisa_types::resource::{Media, ResourceRef};
use nagisa_types::segment::{Forward, ImageSubType, Segment};
use serde_json::{Map, Value};

use crate::wire::{
    EventEnvelope, FriendEntity, GroupEntity, GroupMemberEntity, IncomingMessage, IncomingSegment,
    MessageScene, WireImageSubType, WireReactionType, WireRole, WireSex,
};

mod entity;
mod message;
mod notice;
mod request;
mod segment;

use notice::{
    decode_friend_nudge, decode_group_nudge, decode_peer_pin, decode_recall,
};
use request::{
    decode_friend_request, decode_group_invitation, decode_group_invited_join_request,
    decode_group_join_request,
};

pub use entity::{
    announcement_from_value, essence_message_from_value, file_meta_from_group_file, friend_info,
    group_folder_from_value, group_info, member_info, role_from_wire, sex_from_wire,
};
pub use message::{
    forward_node_from_value, message_event_from_incoming,
};
pub use request::{
    friend_request_to_request, notification_to_notice, notification_to_request,
};
pub use segment::decode_segments;

// ───────────────────────── 公共入口 ─────────────────────────

/// 文本帧 → 统一 `Event`。
///
/// 仅 `message_receive` 的 serde 失败（结构性破坏）才返回 `Err`；其余未知/解析不动的
/// 事件降级为 `Event::Raw`，绝不丢弃。
pub fn decode_event(text: &str) -> Result<Event, serde_json::Error> {
    let env: EventEnvelope = serde_json::from_str(text)?;
    decode_envelope(env)
}

/// 已解析的封包 → `Event`。
pub fn decode_envelope(env: EventEnvelope) -> Result<Event, serde_json::Error> {
    let self_id = Uin(env.self_id);
    let time = env.time;
    let data = env.data;

    let event = match env.event_type.as_str() {
        "message_receive" => {
            // message_scene 分发；serde 错误向上 propagate（不吞）。
            let msg: IncomingMessage = serde_json::from_value(data.clone())?;
            Event::Message(Box::new(message::decode_message(msg, self_id, time, data)))
        }
        "message_recall" => decode_recall(&data, time)
            .map(Event::Notice)
            .unwrap_or_else(|| raw(&env.event_type, data.clone())),
        "bot_offline" => Event::Notice(Notice::BotOffline {
            reason: get_str(&data, "reason"),
            // Milky bot_offline 无 tag（仅 reason）。
            tag: None,
        }),
        "peer_pin_change" => decode_peer_pin(&data)
            .map(Event::Notice)
            .unwrap_or_else(|| raw(&env.event_type, data.clone())),
        "friend_request" => decode_friend_request(&data, time)
            .map(Event::Request)
            .unwrap_or_else(|| raw(&env.event_type, data.clone())),
        "group_join_request" => decode_group_join_request(&data)
            .map(Event::Request)
            .unwrap_or_else(|| raw(&env.event_type, data.clone())),
        "group_invited_join_request" => decode_group_invited_join_request(&data)
            .map(Event::Request)
            .unwrap_or_else(|| raw(&env.event_type, data.clone())),
        "group_invitation" => decode_group_invitation(&data)
            .map(Event::Request)
            .unwrap_or_else(|| raw(&env.event_type, data.clone())),
        "friend_nudge" => decode_friend_nudge(&data),
        "group_nudge" => decode_group_nudge(&data),
        "group_admin_change" => Event::Notice(Notice::AdminChange {
            group: Uin(get_i64(&data, "group_id")),
            user: Uin(get_i64(&data, "user_id")),
            operator: get_opt_i64(&data, "operator_id").map(Uin),
            is_set: get_bool(&data, "is_set"),
        }),
        "group_essence_message_change" => Event::Notice(Notice::EssenceChange {
            group: Uin(get_i64(&data, "group_id")),
            seq: get_i64(&data, "message_seq"),
            // Milky group_essence_message_change 无原作者 sender_id 字段。
            sender: get_opt_i64(&data, "sender_id").map(Uin),
            operator: get_opt_i64(&data, "operator_id").map(Uin),
            is_set: get_bool(&data, "is_set"),
        }),
        "group_member_increase" => Event::Notice(Notice::MemberIncrease {
            group: Uin(get_i64(&data, "group_id")),
            user: Uin(get_i64(&data, "user_id")),
            operator: get_opt_i64(&data, "operator_id").map(Uin),
            invitor: get_opt_i64(&data, "invitor_id").map(Uin),
        }),
        // group_member_decrease: operator_id 仅在“管理员踢出”时存在。
        // OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/common.ts (group_member_decrease)
        "group_member_decrease" => {
            let operator = get_opt_i64(&data, "operator_id").map(Uin);
            let reason = if operator.is_some() {
                MemberDecreaseReason::Kick
            } else {
                MemberDecreaseReason::Leave
            };
            Event::Notice(Notice::MemberDecrease {
                group: Uin(get_i64(&data, "group_id")),
                user: Uin(get_i64(&data, "user_id")),
                operator,
                reason,
            })
        }
        "group_name_change" => Event::Notice(Notice::GroupNameChange {
            group: Uin(get_i64(&data, "group_id")),
            new_name: get_str(&data, "new_group_name"),
            operator: Uin(get_i64(&data, "operator_id")),
        }),
        "group_message_reaction" => {
            let face_id = get_str(&data, "face_id");
            Event::Notice(Notice::Reaction {
                group: Uin(get_i64(&data, "group_id")),
                user: Uin(get_i64(&data, "user_id")),
                seq: get_i64(&data, "message_seq"),
                // Milky 单 reaction:likes 为单元素(与 face_id/count 一致)。
                likes: vec![EmojiLike { face_id: face_id.clone(), count: None }],
                face_id,
                kind: reaction_kind(&data),
                is_add: get_bool(&data, "is_add"),
                // Milky 无独立 reaction sub_type（is_add 即语义）。
                sub_type: None,
                count: None,
            })
        }
        "group_mute" => Event::Notice(Notice::Mute {
            group: Uin(get_i64(&data, "group_id")),
            user: Uin(get_i64(&data, "user_id")),
            operator: Uin(get_i64(&data, "operator_id")),
            duration: get_i64(&data, "duration") as i32,
        }),
        "group_whole_mute" => Event::Notice(Notice::WholeMute {
            group: Uin(get_i64(&data, "group_id")),
            operator: Uin(get_i64(&data, "operator_id")),
            is_mute: get_bool(&data, "is_mute"),
        }),
        "group_file_upload" => Event::Notice(Notice::GroupFileUpload {
            group: Uin(get_i64(&data, "group_id")),
            user: Uin(get_i64(&data, "user_id")),
            file: file_meta(&data),
        }),
        "friend_file_upload" => Event::Notice(Notice::FriendFileUpload {
            user: Uin(get_i64(&data, "user_id")),
            file: file_meta(&data),
            is_self: get_bool(&data, "is_self"),
        }),
        // 未知 event_type → Raw，绝不丢弃。
        other => raw(other, data),
    };
    Ok(event)
}

fn raw(kind: &str, data: Value) -> Event {
    Event::Raw(RawEvent {
        protocol: Protocol::Milky,
        kind: kind.to_string(),
        raw: data,
    })
}

// ───────────────────────── 杂项 ─────────────────────────

fn reaction_kind(data: &Value) -> ReactionKind {
    let t: Option<WireReactionType> = data
        .get("reaction_type")
        .and_then(|v| serde_json::from_value(v.clone()).ok());
    match t {
        Some(WireReactionType::Emoji) => ReactionKind::Emoji,
        _ => ReactionKind::Face,
    }
}

fn file_meta(data: &Value) -> nagisa_types::entity::FileMeta {
    let hash = get_str(data, "file_hash");
    // file 段只带 id/name/size/hash;群文件的富字段(uploader/time/folder)不在段里。
    nagisa_types::entity::FileMeta {
        id: get_str(data, "file_id"),
        name: get_str(data, "file_name"),
        size: get_i64(data, "file_size").max(0) as u64,
        hash: (!hash.is_empty()).then_some(hash),
        busid: None,
        uploader: None,
        upload_time: None,
        dead_time: None,
        download_times: None,
        parent_folder_id: None,
    }
}

// ───────────────────────── Value 取值小工具 ─────────────────────────

fn get_i64(v: &Value, key: &str) -> i64 {
    v.get(key).and_then(Value::as_i64).unwrap_or(0)
}
fn get_opt_i64(v: &Value, key: &str) -> Option<i64> {
    v.get(key).and_then(Value::as_i64)
}
fn get_bool(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}
fn get_str(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}
fn get_opt_str(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}
