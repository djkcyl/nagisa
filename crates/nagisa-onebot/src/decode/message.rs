//! 消息事件解码 + `sender` 子对象 → MemberInfo / FriendInfo 合成。
use super::*;

pub(super) fn decode_message(ev: RawEventJson) -> Event {
    let is_group = ev.message_type.as_deref() == Some("group");
    let peer = if is_group {
        Peer::group(ev.group_id.unwrap_or(0))
    } else if ev.sub_type.as_deref() == Some("group") {
        // OneBot 临时(群)会话:sub_type 为 group 的私聊消息。
        Peer::temp(ev.user_id.unwrap_or(0))
    } else {
        Peer::friend(ev.user_id.unwrap_or(0))
    };
    let sender = Uin(ev.user_id.unwrap_or(0));
    let self_id = Uin(ev.self_id);
    let is_self = ev.post_type.as_deref() == Some("message_sent") || sender == self_id;

    // LLOneBot/NapCat 携带会话内 message_seq：typed 落到 MessageId.seq（缺省 0）；
    // onebot_id 仍取 message_id（撤回/get_msg 等动作以 onebot_id 为锚点）。
    let id = MessageId { peer, seq: ev.message_seq.unwrap_or(0), onebot_id: ev.message_id };

    let content: Message = match &ev.message {
        // 把事件真实的 `peer` 串进去,让嵌套 `reply` 段恢复真实会话对端(与 Milky 对齐),
        // 而非 `decode_segment` 否则会用的 `friend(0)` 兜底。
        Some(WireMessage::Array(segs)) => decode_segments(segs, peer),
        Some(WireMessage::Cq(s)) => decode_cq_string(s, peer),
        None => Vec::new(),
    };

    let raw = event_raw_value(&ev);

    // 群场景:从丰富的 `sender` 子对象合成 MemberInfo,使 bot 不必再发一次
    // get_group_member_info 就能读到 nickname/card/role/title。
    let member = if is_group {
        ev.sender
            .as_ref()
            .map(|s| member_from_sender(s, sender, Uin(ev.group_id.unwrap_or(0))))
    } else {
        None
    };
    // 私聊场景:从 `sender` 子对象合成 FriendInfo(user_id/nickname/sex;age/其余留在 raw),
    // 使 bot 不必再发一次 get_stranger_info 就能读到结构化好友字段。群场景 → None。
    let friend = if is_group {
        None
    } else {
        ev.sender.as_ref().map(|s| friend_from_sender(s, sender))
    };
    let anonymous = ev.anonymous.as_ref().map(|a| Anonymous {
        id: a.id,
        name: a.name.clone(),
        flag: a.flag.clone(),
    });

    // LLOneBot/NapCat 私聊临时会话的来源对端 `target_id`（群消息无此字段）。
    let target_id = ev.target_id.filter(|_| !is_group).map(Uin);
    // Lagrange 群/私聊消息的气泡样式块 `message_style`（其余协议为 None）。
    let message_style = ev.message_style.as_ref().map(|s| MessageStyle {
        bubble_id: s.bubble_id,
        pendant_id: s.pendant_id,
        pal_type: s.pal_type,
        raw: serde_json::to_value(s).unwrap_or(Value::Null),
    });

    Event::Message(Box::new(MessageEvent {
        id,
        peer,
        sender,
        self_id,
        time: ev.time,
        content,
        is_self,
        // OneBot 消息事件不带群实体(只有 group_id,已在 `peer` 里);半空的 GroupInfo 会误导,
        // 故此处 `group` 保持 None。
        group: None,
        member,
        friend,
        anonymous,
        font: ev.font,
        target_id,
        message_style,
        raw,
    }))
}

/// 从群消息的 `sender` 子对象合成 `MemberInfo`。
/// OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/event/message.md (§群消息 sender)
fn member_from_sender(s: &crate::wire::WireSender, user: Uin, group: Uin) -> MemberInfo {
    MemberInfo {
        user,
        group,
        nickname: s.nickname.clone().unwrap_or_default(),
        card: s.card.clone().unwrap_or_default(),
        title: s.title.clone().unwrap_or_default(),
        level: s.level.as_ref().and_then(|l| l.parse().ok()).unwrap_or(0),
        role: match s.role.as_deref() {
            Some("owner") => Role::Owner,
            Some("admin") => Role::Admin,
            _ => Role::Member,
        },
        sex: match s.sex.as_deref() {
            Some("male") => Sex::Male,
            Some("female") => Sex::Female,
            _ => Sex::Unknown,
        },
        age: s.age,
        join_time: 0,
        last_sent_time: None,
        mute_end_time: None,
        // OneBot sender 带 `area`;另外四个扩展字段默认 None/空。
        area: s.area.clone().filter(|a| !a.is_empty()),
        unfriendly: None,
        title_expire_time: None,
        card_changeable: None,
        // 消息 sender 子对象不带 LLOneBot 的 qq_level/is_robot/qage。
        qq_level: None,
        is_robot: None,
        qage: None,
        raw: serde_json::to_value(s).unwrap_or(Value::Null),
    }
}

/// 从私聊消息的 `sender` 子对象合成 `FriendInfo`,使 bot 不必再发一次 get_stranger_info 就能读到
/// 结构化的 nickname/sex。OneBot `PrivateSender` 带 user_id/nickname/sex/age(临时会话还带
/// group_id);`FriendInfo` 没有 `age` 字段,故 age 留在 `raw` 里(无损)。`remark`/`qid`/
/// `category` 在消息 sender 上不存在 → 默认值/None。
/// OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/event/message.md (§私聊消息 sender)
fn friend_from_sender(s: &crate::wire::WireSender, user: Uin) -> FriendInfo {
    FriendInfo {
        user,
        nickname: s.nickname.clone().unwrap_or_default(),
        sex: match s.sex.as_deref() {
            Some("male") => Sex::Male,
            Some("female") => Sex::Female,
            _ => Sex::Unknown,
        },
        // 消息 sender 没有 remark/qid/category/group;完整 sender(含 age)留在 raw。丰富的好友
        // 扩展(birthday/phone/email/login_days/long_nick)只出现在 get_friend_list,消息 sender
        // 上没有 → None。
        remark: String::new(),
        qid: None,
        category: None,
        group: None,
        birthday: None,
        phone: None,
        email: None,
        login_days: None,
        long_nick: None,
        raw: serde_json::to_value(s).unwrap_or(Value::Null),
    }
}
