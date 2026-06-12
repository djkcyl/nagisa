//! Notice 类事件 decode：撤回（`message_recall`）、会话置顶（`peer_pin_change`）、
//! 好友/群戳一戳（`friend_nudge` / `group_nudge`）→ 统一 [`Notice`]。`message_scene`
//! 缺失/未知降级为 `Temp`，所有 wire 字段缺失均降级，绝不 panic。
use super::*;

pub(super) fn decode_recall(data: &Value, _time: i64) -> Option<Notice> {
    let scene_str = data.get("message_scene")?.as_str()?;
    let scene = match scene_str {
        "friend" => Scene::Friend,
        "group" => Scene::Group,
        _ => Scene::Temp,
    };
    let peer = Peer { scene, id: Uin(get_i64(data, "peer_id")) };
    let seq = get_i64(data, "message_seq");
    let suffix = get_str(data, "display_suffix");
    Some(Notice::Recall {
        peer,
        id: MessageId::from_seq(peer, seq),
        sender: Uin(get_i64(data, "sender_id")),
        operator: Uin(get_i64(data, "operator_id")),
        suffix: (!suffix.is_empty()).then_some(suffix),
    })
}

pub(super) fn decode_peer_pin(data: &Value) -> Option<Notice> {
    let scene_str = data.get("message_scene")?.as_str()?;
    let scene = match scene_str {
        "friend" => Scene::Friend,
        "group" => Scene::Group,
        _ => Scene::Temp,
    };
    Some(Notice::PeerPinChange {
        peer: Peer { scene, id: Uin(get_i64(data, "peer_id")) },
        is_pinned: get_bool(data, "is_pinned"),
    })
}

fn nudge_display(data: &Value) -> NudgeDisplay {
    let img = get_str(data, "display_action_img_url");
    NudgeDisplay {
        action: get_str(data, "display_action"),
        suffix: get_str(data, "display_suffix"),
        action_img_url: (!img.is_empty()).then_some(img),
    }
}

pub(super) fn decode_friend_nudge(data: &Value) -> Event {
    Event::Notice(Notice::FriendNudge {
        user: Uin(get_i64(data, "user_id")),
        is_self_send: get_bool(data, "is_self_send"),
        is_self_receive: get_bool(data, "is_self_receive"),
        display: nudge_display(data),
    })
}

pub(super) fn decode_group_nudge(data: &Value) -> Event {
    Event::Notice(Notice::GroupNudge {
        group: Uin(get_i64(data, "group_id")),
        sender: Uin(get_i64(data, "sender_id")),
        receiver: Uin(get_i64(data, "receiver_id")),
        display: nudge_display(data),
    })
}
