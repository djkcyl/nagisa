//! 消息事件 decode：`message_receive` 封包 → [`MessageEvent`]（按 `message_scene` 分发，
//! 携带群/成员/好友实体），并含 `get_message` 等动作复用的 [`message_event_from_incoming`]
//! 与转发节点映射 [`forward_node_from_value`]。段内容委派 `segment` 家族。
use super::*;

fn scene_to_nagi(scene: MessageScene) -> Scene {
    match scene {
        MessageScene::Friend => Scene::Friend,
        MessageScene::Group => Scene::Group,
        // temp / unknown 都按 temp 处理（peer 仍可定位）。
        MessageScene::Temp | MessageScene::Unknown => Scene::Temp,
    }
}

pub(super) fn decode_message(msg: IncomingMessage, self_id: Uin, _time: i64, raw: Value) -> MessageEvent {
    let base = msg.base();
    let scene = scene_to_nagi(base.message_scene);
    let peer = Peer { scene, id: Uin(base.peer_id) };
    let id = MessageId::from_seq(peer, base.message_seq);
    let sender = Uin(base.sender_id);
    let content = decode_segments(&base.segments, peer);
    let is_self = sender == self_id;
    let (group, member, friend) = match &msg {
        IncomingMessage::Group(g) => (Some(group_info(&g.group)), Some(member_info(&g.group_member)), None),
        IncomingMessage::Temp(t) => (t.group.as_ref().map(group_info), None, None),
        // friend scene:透出每条消息自带的 FriendEntity(nickname/remark/sex/qid/category)。
        IncomingMessage::Friend(f) => (None, None, Some(friend_info(&f.friend))),
    };
    MessageEvent {
        id,
        peer,
        sender,
        self_id,
        time: base.time,
        content,
        is_self,
        group,
        member,
        friend,
        anonymous: None,
        font: None,
        // Milky 无私聊 target_id / 气泡 message_style。
        target_id: None,
        message_style: None,
        raw,
    }
}

/// 用于从 `get_message` 等动作返回构造 `MessageEvent`。
pub fn message_event_from_incoming(msg: IncomingMessage, self_id: Uin, raw: Value) -> MessageEvent {
    let time = msg.base().time;
    decode_message(msg, self_id, time, raw)
}

/// 把一条 `IncomingForwardedMessage`（来自 `get_forwarded_messages`）映射为 `ForwardNode`。
/// IR 字段：message_seq / sender_name / avatar_url / time / segments；无 user_id，
/// 故 `user` 取 0 哨兵，`name` 取 sender_name。段 decode 以 group(0) 作中性 peer 上下文
/// （peer 仅用于 Reply 的 MessageId 构造）。
/// OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/message.ts (get_forwarded_messages)
pub fn forward_node_from_value(data: &Value) -> nagisa_types::segment::ForwardNode {
    let peer = Peer { scene: Scene::Group, id: Uin(0) };
    let segments: Vec<IncomingSegment> =
        data.get("segments").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
    nagisa_types::segment::ForwardNode {
        // IR 的转发节点不带发送者 uin（只有 sender_name），故恒为 0 哨兵。
        user: Uin(0),
        name: get_str(data, "sender_name"),
        content: decode_segments(&segments, peer),
        time: data.get("time").and_then(Value::as_i64),
    }
}
