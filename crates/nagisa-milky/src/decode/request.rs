//! Request 类事件 decode：好友请求（`friend_request`）、入群/被邀入群/邀请请求
//! （`group_join_request` / `group_invited_join_request` / `group_invitation`）→ 统一
//! [`Request`]，并构造与动作路径一致的 `RequestTokenInner` token。
//!
//! 另透出动作侧复用的 `pub` 辅助：[`friend_request_to_request`]（好友请求实体）、
//! [`notification_to_request`]（群通知 → 请求）、[`notification_to_notice`]（非请求群通知
//! `admin_change`/`kick`/`quit` → [`Notice`]，避免静默丢弃）。事件与动作两条路径共用同一套
//! 映射，所有 wire 字段缺失均降级，绝不 panic。
use super::*;

pub(super) fn decode_friend_request(data: &Value, _time: i64) -> Option<Request> {
    Some(friend_request_to_request(data))
}

/// 把一条好友请求（`friend_request` 事件 data，或 `get_friend_requests` 返回的
/// `FriendRequest` 实体）映射为 `Request::Friend`，并构造与事件路径一致的
/// `RequestTokenInner::MilkyFriend` token。事件与动作两条路径共用此函数。
/// OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/friend.ts (get_friend_requests)
pub fn friend_request_to_request(data: &Value) -> Request {
    // IR: initiator_id 是 uin(int64)，initiator_uid 是 string。
    let initiator = Uin(get_i64(data, "initiator_id"));
    let initiator_uid = get_str(data, "initiator_uid");
    let is_filtered = get_bool(data, "is_filtered");
    let token = Token(RequestTokenInner::MilkyFriend { initiator_uid: initiator_uid.clone(), is_filtered });
    Request::Friend {
        initiator,
        initiator_uid: (!initiator_uid.is_empty()).then_some(initiator_uid),
        comment: get_str(data, "comment"),
        via: get_str(data, "via"),
        // Milky friend_request 没有「来源群」概念。
        source_group: None,
        // IR FriendRequest 富字段：target_user_id/state/time/is_filtered。
        target_user_id: get_opt_i64(data, "target_user_id").map(Uin),
        state: request_state_from(data.get("state").and_then(Value::as_str)),
        time: get_opt_i64(data, "time"),
        is_filtered,
        token,
    }
}

/// 把 Milky `FriendRequest.state` 字符串枚举映射为 `RequestState`。
/// 未知/缺失值 → `RequestState::Unknown`（绝不 panic）。
fn request_state_from(s: Option<&str>) -> RequestState {
    match s {
        Some("pending") => RequestState::Pending,
        Some("accepted") => RequestState::Accepted,
        Some("rejected") => RequestState::Rejected,
        Some("ignored") => RequestState::Ignored,
        _ => RequestState::Unknown,
    }
}

pub(super) fn decode_group_join_request(data: &Value) -> Option<Request> {
    notification_to_request(data, "join_request")
}

pub(super) fn decode_group_invited_join_request(data: &Value) -> Option<Request> {
    notification_to_request(data, "invited_join_request")
}

/// 把单条群通知（`group_join_request`/`group_invited_join_request` 事件 data，或
/// `get_group_notifications` 返回的 `GroupNotification` 实体）映射为 `Request`。
///
/// `notification_type` 为通知类型（`join_request` / `invited_join_request`）；
/// 由调用方提供（事件路径硬编码、动作路径取自实体 `type` 字段）。两条路径共用此函数，
/// 保证产出的 `Request` 与 `RequestTokenInner::MilkyGroupNotification` 一致。
/// 非请求类通知（`admin_change`/`kick`/`quit`）或未知类型 → `None`。
pub fn notification_to_request(data: &Value, notification_type: &str) -> Option<Request> {
    let group = Uin(get_i64(data, "group_id"));
    let notification_seq = get_i64(data, "notification_seq");
    match notification_type {
        "join_request" => {
            let is_filtered = get_bool(data, "is_filtered");
            let token = Token(RequestTokenInner::MilkyGroupNotification {
                notification_seq,
                notification_type: "join_request".into(),
                group_id: group,
                is_filtered,
            });
            Some(Request::GroupJoin {
                group,
                initiator: Uin(get_i64(data, "initiator_id")),
                comment: get_str(data, "comment"),
                // Milky join_request 无邀请人字段。
                invitor: None,
                is_filtered,
                token,
            })
        }
        "invited_join_request" => {
            let token = Token(RequestTokenInner::MilkyGroupNotification {
                notification_seq,
                notification_type: "invited_join_request".into(),
                group_id: group,
                is_filtered: false,
            });
            Some(Request::GroupInvitedJoin {
                group,
                initiator: Uin(get_i64(data, "initiator_id")),
                target: Uin(get_i64(data, "target_user_id")),
                token,
            })
        }
        // admin_change / kick / quit / 未知类型不是请求：忽略。
        // 其中 admin_change/kick/quit 是「已发生的事实」，经 `notification_to_notice`
        // 透出为 Notice（见动作路径 get_group_notices_paged），不在此当 Request 丢弃。
        _ => None,
    }
}

/// 把一条**非请求**群通知（`admin_change` / `kick` / `quit`）映射为 [`Notice`]。
///
/// 这三种 `GroupNotification` 变体描述的是已发生的事实（设/撤管理、被踢、主动退群），
/// 不是待处理请求，因此 `notification_to_request` 对它们返回 `None`。
/// 为避免静默丢弃，动作路径（`get_group_notifications` 返回的实体）用此函数把它们
/// 映射为与事件流同名的 `Notice`：
///   - `admin_change` → [`Notice::AdminChange`]（group/user/operator/is_set）；
///   - `kick`         → [`Notice::MemberDecrease`]（reason=`Kick`，operator 为执行者）；
///   - `quit`         → [`Notice::MemberDecrease`]（reason=`Leave`，无 operator）。
///
/// `join_request` / `invited_join_request` / 未知类型 → `None`（请求经
/// `notification_to_request` 处理）。所有 wire 字段缺失均降级（绝不 panic）。
/// OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/common.ts (GroupNotification)
pub fn notification_to_notice(data: &Value, notification_type: &str) -> Option<Notice> {
    let group = Uin(get_i64(data, "group_id"));
    match notification_type {
        "admin_change" => Some(Notice::AdminChange {
            group,
            // 被设/撤管理者用 `target_user_id`，回退 `user_id`（IR 字段命名差异容错）。
            user: Uin(get_opt_i64(data, "target_user_id").unwrap_or_else(|| get_i64(data, "user_id"))),
            operator: get_opt_i64(data, "operator_id").map(Uin),
            is_set: get_bool(data, "is_set"),
        }),
        "kick" => Some(Notice::MemberDecrease {
            group,
            // kick 变体的被踢者用 `target_user_id`，回退 `user_id`（IR 字段命名差异容错）。
            user: Uin(get_opt_i64(data, "target_user_id").unwrap_or_else(|| get_i64(data, "user_id"))),
            operator: get_opt_i64(data, "operator_id").map(Uin),
            reason: MemberDecreaseReason::Kick,
        }),
        "quit" => Some(Notice::MemberDecrease {
            group,
            // 主动退群者用 `target_user_id`，回退 `user_id`（IR 字段命名差异容错）。
            user: Uin(get_opt_i64(data, "target_user_id").unwrap_or_else(|| get_i64(data, "user_id"))),
            // 主动退群无操作者。
            operator: None,
            reason: MemberDecreaseReason::Leave,
        }),
        // join_request / invited_join_request 是请求；未知类型忽略。
        _ => None,
    }
}

pub(super) fn decode_group_invitation(data: &Value) -> Option<Request> {
    let group = Uin(get_i64(data, "group_id"));
    let invitation_seq = get_i64(data, "invitation_seq");
    let token = Token(RequestTokenInner::MilkyInvitation { group_id: group, invitation_seq });
    Some(Request::GroupInvite {
        group,
        initiator: Uin(get_i64(data, "initiator_id")),
        comment: String::new(),
        source_group: get_opt_i64(data, "source_group_id").map(Uin),
        token,
    })
}
