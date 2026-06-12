//! 请求事件(friend/group)+ meta 事件(lifecycle/heartbeat)解码。
use super::*;

pub(super) fn decode_request(ev: RawEventJson) -> Event {
    let token = RequestToken::onebot_flag(ev.flag.clone().unwrap_or_default());
    let comment = ev.comment.clone().unwrap_or_default();
    let initiator = Uin(ev.user_id.unwrap_or(0));
    match ev.request_type.as_deref() {
        Some("friend") => Event::Request(Request::Friend {
            initiator,
            initiator_uid: None,
            comment,
            via: "onebot".to_string(),
            // go-cqhttp/Lagrange 扩展:来自群的好友请求在 `group_id`(或 `source_group`)下携带
            // 来源群;否则 None。
            source_group: ev.group_id.or_else(|| ev.extra.get("source_group").and_then(Value::as_i64)).map(Uin),
            // Milky 才有的 FriendRequest 丰富字段:OneBot 没有 target/state/is_filtered,
            // 请求时间戳落在事件外层封包上。
            target_user_id: None,
            state: RequestState::Unknown,
            time: (ev.time != 0).then_some(ev.time),
            is_filtered: false,
            token,
        }),
        Some("group") => {
            let group = Uin(ev.group_id.unwrap_or(0));
            if ev.sub_type.as_deref() == Some("invite") {
                Event::Request(Request::GroupInvite { group, initiator, comment, source_group: None, token })
            } else {
                Event::Request(Request::GroupJoin {
                    group,
                    initiator,
                    comment,
                    // Lagrange：经邀请链接申请入群时携带邀请人 invitor_id。
                    invitor: ev.invitor_id.map(Uin),
                    is_filtered: false,
                    token,
                })
            }
        }
        _ => raw_event(&ev, "request"),
    }
}

pub(super) fn decode_meta(ev: RawEventJson) -> Event {
    match ev.meta_event_type.as_deref() {
        Some("lifecycle") => match ev.sub_type.as_deref() {
            // OneBot lifecycle enable/disable 无 reason 字段。
            Some("enable") => Event::Meta(Meta::BotOnline { reason: None }),
            Some("disable") => Event::Meta(Meta::BotOffline),
            // lifecycle.connect（协议端「已连接」帧）与框架统一发的 Meta::Connect 语义重复
            // ——框架事件源在 socket 连上时就发 Meta::Connect（跨协议口径一致、重连也发）。
            // 故打上独立 kind 标签 "lifecycle_connect"，由适配器转发层据此**丢弃**、不进事件流
            // （见 `crate::adapter::dispatch_event`）；其余未知 lifecycle 子类型仍保留为 Raw。
            // 直接构造 Raw（而非 raw_event）：raw_event 的 kind 取 post_type（恒为 "meta_event"），
            // 那样标签会被盖掉，故此处显式写死 kind。
            Some("connect") => Event::Raw(RawEvent {
                protocol: PROTO,
                kind: "lifecycle_connect".to_string(),
                raw: event_raw_value(&ev),
            }),
            _ => raw_event(&ev, "meta_event"),
        },
        Some("heartbeat") => Event::Meta(Meta::Heartbeat {
            interval: ev.interval.unwrap_or(0),
            status: heartbeat_status(ev.status.as_ref()),
        }),
        _ => raw_event(&ev, "meta_event"),
    }
}

fn heartbeat_status(v: Option<&Value>) -> ImplStatus {
    // 心跳 status 与 `get_status` 同构；统计子块（stat）复用 adapter 侧的既有解析。
    let raw = v.cloned().unwrap_or(Value::Null);
    ImplStatus {
        online: raw.get("online").and_then(Value::as_bool).unwrap_or(false),
        good: raw.get("good").and_then(Value::as_bool).unwrap_or(false),
        stat: crate::adapter::impl_stat_from(&raw),
        raw,
    }
}
