//! Milky 出站动作映射：三个动作 trait 在 [`MilkyAdapter`] 上的实现。
//!
//! 每个方法把统一 IR 参数编码成 Milky wire 形状，经 [`MilkyAdapter::call`] `POST` 到对应
//! action，再把 `data` decode 回统一类型；三块 impl 分工：
//! - [`ActionInvoker`](nagisa_core::adapter::ActionInvoker)：通用动作（发消息 / 撤回 / 群管理 /
//!   请求处理 / 文件 / 转发 / 资料 / 系统）。`protocol()` 返回 [`Protocol::Milky`]，`supports()`
//!   给出能力集——并对探测到的 Lagrange.Milky 已知缺口诚实返回 `false`。
//! - [`MilkyActions`](nagisa_core::adapter::MilkyActions)：Milky 协议专属动作（impl_info /
//!   好友资料 / 分页拉取请求·通知·历史消息 / 置顶 等）。
//! - [`OneBotActions`](nagisa_core::adapter::OneBotActions)：**空 impl**——OneBot 独有/厂商扩展动作
//!   全走 trait 默认实现返回 `Error::Unsupported`，不在 Milky 上伪造。
//!
//! 参数/响应的 wire 字段映射与 decode 辅助见 [`crate::decode`] / [`crate::encode`]；多数动作的
//! 行内注释带 `OFFICIAL:` 溯源 URL（指向 Milky IR 定义）。
use async_trait::async_trait;
use nagisa_core::adapter::{ActionInvoker, MilkyActions, OneBotActions};
use nagisa_core::ImplInfo;
use nagisa_types::error::{Error, Result};
use nagisa_types::event::{Notice, ReactionKind, RequestToken, RequestTokenInner};
use nagisa_types::prelude::*;
use serde_json::{json, Value};

use crate::decode::{
    announcement_from_value, essence_message_from_value, file_meta_from_group_file, forward_node_from_value,
    friend_info, friend_request_to_request, group_folder_from_value, group_info, member_info,
    message_event_from_incoming, notification_to_notice, notification_to_request, sex_from_wire,
};
use crate::encode::{encode, source_to_uri};
use crate::transport::MilkyAdapter;
use crate::wire::{FriendEntity, GroupEntity, IncomingMessage, WireSex};

/// 把私聊/群聊 scene 映射到动作名后缀。
fn scene_action(peer: &Peer, private: &'static str, group: &'static str) -> &'static str {
    if peer.is_group() {
        group
    } else {
        private
    }
}

/// [`Scene`] → Milky wire `message_scene` 串（friend/group/temp）。多处动作正文需要它
/// （get_message / mark_message_as_read / get_history_messages / set_peer_pin），收敛一处避免漂移。
fn scene_str(scene: Scene) -> &'static str {
    match scene {
        Scene::Friend => "friend",
        Scene::Group => "group",
        Scene::Temp => "temp",
    }
}

#[async_trait]
impl ActionInvoker for MilkyAdapter {
    fn protocol(&self) -> Protocol {
        Protocol::Milky
    }

    fn supports(&self, cap: Capability) -> bool {
        // 标准 Milky 1.x 能力集（其他实现如 LLOneBot 按标准全实现）。
        // FileOps/Nudge/ProfileLike/SelfProfile/MessageHistory/Cookies 均在 Milky 标准集内；
        // Ocr/Ai Milky 不支持。
        let standard = matches!(
            cap,
            Capability::GroupMute
                | Capability::GroupAdmin
                | Capability::GroupKick
                | Capability::HandleRequest
                | Capability::Essence
                | Capability::Announcement
                | Capability::PeerPin
                | Capability::Reaction
                | Capability::Forward
                | Capability::FileOps
                | Capability::Nudge
                | Capability::ProfileLike
                | Capability::SelfProfile
                | Capability::MessageHistory
                | Capability::Cookies
        );

        // Lagrange.Milky 缺约 35 个动作（kick/mute/admin/handle_request/announcement/
        // essence/peer_pin），它们虽在标准集里却总是 404。若连接时拿到的 impl_info
        // 表明是 Lagrange，则对这些能力诚实返回 false；否则（含未知/未连接）保持乐观。
        if let Some(info) = self.impl_info.get() {
            let is_lagrange = info.name.to_ascii_lowercase().contains("lagrange");
            if is_lagrange {
                let lacking = matches!(
                    cap,
                    Capability::GroupMute
                        | Capability::GroupAdmin
                        | Capability::GroupKick
                        | Capability::HandleRequest
                        | Capability::Announcement
                        | Capability::Essence
                        | Capability::PeerPin
                );
                let decision = standard && !lacking;
                tracing::debug!(
                    impl_name = %info.name,
                    ?cap,
                    decision,
                    "milky supports() honored Lagrange.Milky capability gaps"
                );
                return decision;
            }
        }

        standard
    }

    async fn send(&self, peer: &Peer, message: &[Segment]) -> Result<MessageId> {
        let segments = encode(message);
        let action = scene_action(peer, "send_private_message", "send_group_message");
        let id_field = if peer.is_group() { "group_id" } else { "user_id" };
        let params = json!({ id_field: peer.id.0, "message": segments });
        let data = self.call(action, params).await?;
        let seq = data.get("message_seq").and_then(Value::as_i64).unwrap_or_default();
        // 响应还带 `time`,但 MessageId 没有 time 槽(需要时可经 get_message(id).time 取回),
        // 故此处刻意不透出。
        Ok(MessageId::from_seq(*peer, seq))
    }

    async fn call_raw(&self, action: &str, params: Value) -> Result<Value> {
        self.call(action, params).await
    }

    async fn recall(&self, id: &MessageId) -> Result<()> {
        let action = scene_action(&id.peer, "recall_private_message", "recall_group_message");
        let id_field = if id.peer.is_group() { "group_id" } else { "user_id" };
        let params = json!({ id_field: id.peer.id.0, "message_seq": id.seq });
        self.call(action, params).await.map(drop)
    }

    async fn get_message(&self, id: &MessageId) -> Result<MessageEvent> {
        let scene = scene_str(id.peer.scene);
        let params = json!({
            "message_scene": scene,
            "peer_id": id.peer.id.0,
            "message_seq": id.seq,
        });
        let data = self.call("get_message", params).await?;
        let raw = data.get("message").cloned().unwrap_or(Value::Null);
        let msg: IncomingMessage = serde_json::from_value(raw.clone())?;
        let self_uin = self.self_id.get().copied().unwrap_or(Uin(0));
        Ok(message_event_from_incoming(msg, self_uin, raw))
    }

    async fn get_login_info(&self) -> Result<(Uin, String)> {
        let data = self.call("get_login_info", json!({})).await?;
        let uin = data.get("uin").and_then(Value::as_i64).unwrap_or_default();
        let nickname = data.get("nickname").and_then(Value::as_str).unwrap_or_default().to_string();
        let self_uin = Uin(uin);
        // 首次成功时缓存自身 uin（OnceLock：已设置则忽略）。
        let _ = self.self_id.set(self_uin);
        Ok((self_uin, nickname))
    }

    async fn get_group_info(&self, group: Uin, no_cache: bool) -> Result<GroupInfo> {
        let params = json!({ "group_id": group.0, "no_cache": no_cache });
        let data = self.call("get_group_info", params).await?;
        let g = data.get("group").cloned().unwrap_or(data);
        let entity = serde_json::from_value(g)?;
        Ok(group_info(&entity))
    }

    async fn get_group_list(&self, no_cache: bool) -> Result<Vec<GroupInfo>> {
        let data = self.call("get_group_list", json!({ "no_cache": no_cache })).await?;
        let arr = data.get("groups").and_then(Value::as_array).cloned().unwrap_or_default();
        let mut out = Vec::with_capacity(arr.len());
        for g in arr {
            let entity = serde_json::from_value(g)?;
            out.push(group_info(&entity));
        }
        Ok(out)
    }

    async fn get_group_member_info(&self, group: Uin, user: Uin, no_cache: bool) -> Result<MemberInfo> {
        let params = json!({ "group_id": group.0, "user_id": user.0, "no_cache": no_cache });
        let data = self.call("get_group_member_info", params).await?;
        let m = data.get("member").cloned().unwrap_or(data);
        let entity = serde_json::from_value(m)?;
        Ok(member_info(&entity))
    }

    async fn get_group_member_list(&self, group: Uin, no_cache: bool) -> Result<Vec<MemberInfo>> {
        let params = json!({ "group_id": group.0, "no_cache": no_cache });
        let data = self.call("get_group_member_list", params).await?;
        let arr = data.get("members").and_then(Value::as_array).cloned().unwrap_or_default();
        let mut out = Vec::with_capacity(arr.len());
        for m in arr {
            let entity = serde_json::from_value(m)?;
            out.push(member_info(&entity));
        }
        Ok(out)
    }

    async fn get_friend_list(&self, no_cache: bool) -> Result<Vec<FriendInfo>> {
        let data = self.call("get_friend_list", json!({ "no_cache": no_cache })).await?;
        let arr = data.get("friends").and_then(Value::as_array).cloned().unwrap_or_default();
        let mut out = Vec::with_capacity(arr.len());
        for f in arr {
            let entity = serde_json::from_value(f)?;
            out.push(friend_info(&entity));
        }
        Ok(out)
    }

    async fn set_group_member_mute(&self, group: Uin, user: Uin, duration: u32) -> Result<()> {
        let params = json!({ "group_id": group.0, "user_id": user.0, "duration": duration });
        self.call("set_group_member_mute", params).await.map(drop)
    }

    async fn set_group_whole_mute(&self, group: Uin, enable: bool) -> Result<()> {
        let params = json!({ "group_id": group.0, "is_mute": enable });
        self.call("set_group_whole_mute", params).await.map(drop)
    }

    async fn set_group_admin(&self, group: Uin, user: Uin, enable: bool) -> Result<()> {
        let params = json!({ "group_id": group.0, "user_id": user.0, "is_set": enable });
        self.call("set_group_member_admin", params).await.map(drop)
    }

    async fn set_group_member_card(&self, group: Uin, user: Uin, card: &str) -> Result<()> {
        let params = json!({ "group_id": group.0, "user_id": user.0, "card": card });
        self.call("set_group_member_card", params).await.map(drop)
    }

    async fn set_group_name(&self, group: Uin, name: &str) -> Result<()> {
        let params = json!({ "group_id": group.0, "new_group_name": name });
        self.call("set_group_name", params).await.map(drop)
    }

    async fn kick_group_member(&self, group: Uin, user: Uin, reject_add: bool) -> Result<()> {
        let params = json!({
            "group_id": group.0,
            "user_id": user.0,
            "reject_add_request": reject_add,
        });
        self.call("kick_group_member", params).await.map(drop)
    }

    async fn handle_request(&self, token: &RequestToken, approve: bool, reason: Option<&str>) -> Result<()> {
        match &token.0 {
            RequestTokenInner::MilkyFriend { initiator_uid, is_filtered } => {
                let action = if approve { "accept_friend_request" } else { "reject_friend_request" };
                let mut params = json!({
                    "initiator_uid": initiator_uid,
                    "is_filtered": is_filtered,
                });
                if !approve {
                    if let Some(r) = reason {
                        params["reason"] = Value::String(r.to_string());
                    }
                }
                self.call(action, params).await.map(drop)
            }
            RequestTokenInner::MilkyGroupNotification {
                notification_seq,
                notification_type,
                group_id,
                is_filtered,
            } => {
                let action = if approve { "accept_group_request" } else { "reject_group_request" };
                let mut params = json!({
                    "notification_seq": notification_seq,
                    "notification_type": notification_type,
                    "group_id": group_id.0,
                    "is_filtered": is_filtered,
                });
                if !approve {
                    if let Some(r) = reason {
                        params["reason"] = Value::String(r.to_string());
                    }
                }
                self.call(action, params).await.map(drop)
            }
            RequestTokenInner::MilkyInvitation { group_id, invitation_seq } => {
                let action = if approve { "accept_group_invitation" } else { "reject_group_invitation" };
                let params = json!({
                    "group_id": group_id.0,
                    "invitation_seq": invitation_seq,
                });
                self.call(action, params).await.map(drop)
            }
            // OneBot flag 与 Milky 无关：报不支持。
            RequestTokenInner::OneBotFlag(_) => Err(Error::Unsupported("handle_request".into())),
        }
    }

    async fn send_reaction(&self, group: Uin, seq: i64, face_id: &str, kind: ReactionKind, is_add: bool) -> Result<()> {
        let reaction_type = match kind {
            ReactionKind::Face => "face",
            ReactionKind::Emoji => "emoji",
        };
        let params = json!({
            "group_id": group.0,
            "message_seq": seq,
            "reaction": face_id,
            "reaction_type": reaction_type,
            "is_add": is_add,
        });
        self.call("send_group_message_reaction", params).await.map(drop)
    }

    async fn send_nudge(&self, peer: &Peer, target: Uin) -> Result<()> {
        if peer.is_group() {
            let params = json!({ "group_id": peer.id.0, "user_id": target.0 });
            self.call("send_group_nudge", params).await.map(drop)
        } else {
            // Lagrange send_friend_nudge(user_id, is_self):
            //   is_self=false → 戳好友；is_self=true → 戳机器人自己。
            // 因此：is_self = (target == bot_self_uin)。
            // 若 self_id 尚未缓存（未调用过 get_login_info），默认 false（戳好友）。
            let is_self = self.self_id.get().is_some_and(|cached| *cached == target);
            let params = json!({ "user_id": peer.id.0, "is_self": is_self });
            self.call("send_friend_nudge", params).await.map(drop)
        }
    }

    async fn upload_group_file(
        &self,
        group: Uin,
        src: ResourceSource,
        name: &str,
        parent_folder_id: Option<&str>,
    ) -> Result<String> {
        let params = json!({
            "group_id": group.0,
            "parent_folder_id": parent_folder_id.unwrap_or("/"),
            "file_uri": source_to_uri(&src),
            "file_name": name,
        });
        let data = self.call("upload_group_file", params).await?;
        Ok(data.get("file_id").and_then(Value::as_str).unwrap_or_default().to_string())
    }

    async fn upload_private_file(&self, user: Uin, src: ResourceSource, name: &str) -> Result<String> {
        let params = json!({
            "user_id": user.0,
            "file_uri": source_to_uri(&src),
            "file_name": name,
        });
        let data = self.call("upload_private_file", params).await?;
        Ok(data.get("file_id").and_then(Value::as_str).unwrap_or_default().to_string())
    }

    // ───────── Milky 1.2 专属 override（按 IR shape 直接 POST 对应 action）─────────

    async fn get_user_info(&self, user: Uin, _no_cache: bool) -> Result<UserInfo> {
        // Milky get_user_profile 无 no_cache 参数（IR 仅取 user_id）。
        let params = json!({ "user_id": user.0 });
        let data = self.call("get_user_profile", params).await?;
        let sex = data
            .get("sex")
            .and_then(|v| serde_json::from_value::<WireSex>(v.clone()).ok())
            .map(sex_from_wire)
            .unwrap_or(nagisa_types::entity::Sex::Unknown);
        // 字符串字段：空串视为缺省（None）。数值字段：缺省为 None。
        let opt_str = |key: &str| -> Option<String> {
            data.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(|s| s.to_string())
        };
        let opt_i32 = |key: &str| -> Option<i32> { data.get(key).and_then(Value::as_i64).map(|n| n as i32) };
        Ok(UserInfo {
            user,
            nickname: data.get("nickname").and_then(Value::as_str).unwrap_or_default().to_string(),
            sex,
            age: opt_i32("age"),
            level: opt_i32("level"),
            qid: opt_str("qid"),
            bio: opt_str("bio"),
            country: opt_str("country"),
            city: opt_str("city"),
            school: opt_str("school"),
            remark: opt_str("remark"),
            // Milky get_user_profile IR 无 status/business/register_time/avatar——
            // 这些是 Lagrange stranger-info 专属扩展 → None。
            status: None,
            business: Vec::new(),
            register_time: None,
            avatar: None,
            raw: data,
        })
    }

    async fn get_message_history(
        &self,
        peer: &Peer,
        start: Option<&MessageId>,
        count: u32,
    ) -> Result<Vec<MessageEvent>> {
        // 唯一的 wire 调用点在 `get_history_messages_paged`;非分页入口只是丢掉
        // `next_message_seq` 游标。
        self.get_history_messages_paged(peer, start.map(|m| m.seq), count).await.map(|(msgs, _next)| msgs)
    }

    async fn leave_group(&self, group: Uin, _dismiss: bool) -> Result<()> {
        // Milky quit_group 无 dismiss 标志（IR 仅 group_id）。
        let params = json!({ "group_id": group.0 });
        self.call("quit_group", params).await.map(drop)
    }

    async fn set_group_member_special_title(
        &self,
        group: Uin,
        user: Uin,
        title: &str,
        _duration: i64, // Milky IR 无 duration 字段，忽略
    ) -> Result<()> {
        let params = json!({
            "group_id": group.0,
            "user_id": user.0,
            "special_title": title,
        });
        self.call("set_group_member_special_title", params).await.map(drop)
    }

    async fn mark_message_as_read(&self, _peer: &Peer, id: &MessageId) -> Result<()> {
        // IR mark_message_as_read: message_scene / peer_id / message_seq。
        // scene/peer/seq 均取自 MessageId 三元组（peer 参数与 id.peer 同一会话）。
        let scene = scene_str(id.peer.scene);
        let params = json!({
            "message_scene": scene,
            "peer_id": id.peer.id.0,
            "message_seq": id.seq,
        });
        self.call("mark_message_as_read", params).await.map(drop)
    }

    async fn set_essence(&self, group: Uin, id: &MessageId, enable: bool) -> Result<()> {
        // IR set_group_essence_message: group_id / message_seq / is_set。
        let params = json!({
            "group_id": group.0,
            "message_seq": id.seq,
            "is_set": enable,
        });
        self.call("set_group_essence_message", params).await.map(drop)
    }

    async fn get_group_file_download_url(&self, group: Uin, file_id: &str) -> Result<String> {
        let params = json!({ "group_id": group.0, "file_id": file_id });
        let data = self.call("get_group_file_download_url", params).await?;
        // IR/Lagrange 响应字段为 `download_url`（非 `url`）。
        Ok(data.get("download_url").and_then(Value::as_str).unwrap_or_default().to_string())
    }

    async fn delete_group_file(&self, group: Uin, file_id: &str) -> Result<()> {
        let params = json!({ "group_id": group.0, "file_id": file_id });
        self.call("delete_group_file", params).await.map(drop)
    }

    async fn get_group_requests(&self) -> Result<Vec<Request>> {
        // 委托给分页版,用 Milky 规范默认值(start_seq=None, limit=20, is_filtered=false)。
        Ok(self.get_group_notifications_paged(None, 20, false).await?.0)
    }

    async fn get_cookies(&self, domain: Option<&str>) -> Result<String> {
        // IR get_cookies：domain 必填。无 domain 时传空串（让服务端按需报错/兜底）。
        let params = json!({ "domain": domain.unwrap_or_default() });
        let data = self.call("get_cookies", params).await?;
        Ok(data.get("cookies").and_then(Value::as_str).unwrap_or_default().to_string())
    }

    // ───────── Milky 1.2 批次：转发 / 好友 / 公告 / 精华 / 置顶 / 文件 / 资料 / 系统 ─────────
    // 标准 Milky IR 全集（LLOneBot / LuckyLilliaBot 全实现；Lagrange.Milky 子集会 404）。

    async fn get_forward_messages(&self, forward_id: &str) -> Result<Vec<ForwardNode>> {
        // IR get_forwarded_messages: {forward_id} → messages: IncomingForwardedMessage[]。
        // OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/message.ts
        let params = json!({ "forward_id": forward_id });
        let data = self.call("get_forwarded_messages", params).await?;
        let arr = data.get("messages").and_then(Value::as_array).cloned().unwrap_or_default();
        Ok(arr.iter().map(forward_node_from_value).collect())
    }

    async fn delete_friend(&self, user: Uin) -> Result<()> {
        // IR delete_friend (since 1.1): {user_id}。无其他参数。
        // OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/friend.ts
        let params = json!({ "user_id": user.0 });
        self.call("delete_friend", params).await.map(drop)
    }

    async fn send_profile_like(&self, user: Uin, count: u32) -> Result<()> {
        // IR send_profile_like: {user_id, count}。
        // OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/friend.ts
        let params = json!({ "user_id": user.0, "count": count });
        self.call("send_profile_like", params).await.map(drop)
    }

    async fn send_group_announcement(
        &self,
        group: Uin,
        content: &str,
        image: Option<ResourceSource>,
    ) -> Result<String> {
        // IR send_group_announcement: {group_id, content, image_uri?}。注意字段名 image_uri
        // （非 image），经 source_to_uri 编码。IR 未定义响应体，故防御性读取 announcement_id
        // （部分实现如 LLOneBot 会回传），缺省则空串。
        // OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/group.ts
        let mut params = json!({ "group_id": group.0, "content": content });
        if let Some(src) = image {
            params["image_uri"] = Value::String(source_to_uri(&src));
        }
        let data = self.call("send_group_announcement", params).await?;
        Ok(data.get("announcement_id").and_then(Value::as_str).unwrap_or_default().to_string())
    }

    async fn get_group_announcements(&self, group: Uin) -> Result<Vec<Announcement>> {
        // IR get_group_announcements: {group_id} → announcements: GroupAnnouncementEntity[]
        // （字段 announcement_id / group_id / user_id / time / content / image_url?）。
        // OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/group.ts
        let params = json!({ "group_id": group.0 });
        let data = self.call("get_group_announcements", params).await?;
        let arr = data.get("announcements").and_then(Value::as_array).cloned().unwrap_or_default();
        Ok(arr.iter().map(announcement_from_value).collect())
    }

    async fn delete_group_announcement(&self, group: Uin, announcement_id: &str) -> Result<()> {
        // IR delete_group_announcement: {group_id, announcement_id}。
        // OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/group.ts
        let params = json!({ "group_id": group.0, "announcement_id": announcement_id });
        self.call("delete_group_announcement", params).await.map(drop)
    }

    async fn get_essence_messages(&self, group: Uin) -> Result<Vec<EssenceMessage>> {
        // IR get_group_essence_messages: {group_id, page_index, page_size} → messages:
        // GroupEssenceMessage[] + is_end。循环 is_end 直到末页，拼接全部精华消息。
        // OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/group.ts
        let mut out = Vec::new();
        let mut page = 0;
        loop {
            let params = json!({ "group_id": group.0, "page_index": page, "page_size": 30 });
            let data = self.call("get_group_essence_messages", params).await?;
            let arr = data.get("messages").and_then(Value::as_array).cloned().unwrap_or_default();
            out.extend(arr.iter().map(essence_message_from_value));
            let is_end = data.get("is_end").and_then(Value::as_bool).unwrap_or(true);
            if is_end || arr.is_empty() {
                break;
            }
            page += 1;
            if page > 100 {
                break; // 翻页安全上限
            }
        }
        Ok(out)
    }

    async fn get_group_files(&self, group: Uin, folder_id: Option<&str>) -> Result<GroupFileList> {
        // IR get_group_files: {group_id, parent_folder_id?='/'} → files: GroupFileEntity[],
        // folders: GroupFolderEntity[]。folder_id=None → 根目录 "/"。
        // OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/file.ts
        let params = json!({
            "group_id": group.0,
            "parent_folder_id": folder_id.unwrap_or("/"),
        });
        let data = self.call("get_group_files", params).await?;
        let files = data
            .get("files")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(file_meta_from_group_file).collect())
            .unwrap_or_default();
        let folders = data
            .get("folders")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(group_folder_from_value).collect())
            .unwrap_or_default();
        Ok(GroupFileList { files, folders })
    }

    async fn get_private_file_download_url(&self, user: Uin, file_id: &str, hash: Option<&str>) -> Result<String> {
        // IR get_private_file_download_url: {user_id, file_id, file_hash} → download_url。
        // file_hash 是 TriSHA1；IR 标必填，缺省时传空串（防御）。
        // OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/file.ts
        let params = json!({
            "user_id": user.0,
            "file_id": file_id,
            "file_hash": hash.unwrap_or_default(),
        });
        let data = self.call("get_private_file_download_url", params).await?;
        Ok(data.get("download_url").and_then(Value::as_str).unwrap_or_default().to_string())
    }

    async fn create_group_folder(&self, group: Uin, name: &str) -> Result<String> {
        // IR create_group_folder: {group_id, folder_name} → folder_id。
        // OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/file.ts
        let params = json!({ "group_id": group.0, "folder_name": name });
        let data = self.call("create_group_folder", params).await?;
        Ok(data.get("folder_id").and_then(Value::as_str).unwrap_or_default().to_string())
    }

    async fn rename_group_folder(&self, group: Uin, folder_id: &str, new_name: &str) -> Result<()> {
        // IR rename_group_folder: {group_id, folder_id, new_folder_name}。
        // OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/file.ts
        let params = json!({
            "group_id": group.0,
            "folder_id": folder_id,
            "new_folder_name": new_name,
        });
        self.call("rename_group_folder", params).await.map(drop)
    }

    async fn delete_group_folder(&self, group: Uin, folder_id: &str) -> Result<()> {
        // IR delete_group_folder: {group_id, folder_id}。
        // OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/file.ts
        let params = json!({ "group_id": group.0, "folder_id": folder_id });
        self.call("delete_group_folder", params).await.map(drop)
    }

    async fn move_group_file(
        &self,
        group: Uin,
        file_id: &str,
        source_folder_id: Option<&str>,
        target_folder_id: Option<&str>,
    ) -> Result<()> {
        // IR move_group_file: {group_id, file_id, parent_folder_id?='/', target_folder_id?='/'}。
        // source_folder_id → wire 上的 parent_folder_id;target=None → 根目录 "/"。
        // OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/file.ts
        let params = json!({
            "group_id": group.0,
            "file_id": file_id,
            "parent_folder_id": source_folder_id.unwrap_or("/"),
            "target_folder_id": target_folder_id.unwrap_or("/"),
        });
        self.call("move_group_file", params).await.map(drop)
    }

    async fn rename_group_file(
        &self,
        group: Uin,
        file_id: &str,
        source_folder_id: Option<&str>,
        new_name: &str,
    ) -> Result<()> {
        // IR rename_group_file: {group_id, file_id, parent_folder_id?='/', new_file_name}。
        // source_folder_id → wire 上的 parent_folder_id;None → 根目录 "/"。
        // OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/file.ts
        let params = json!({
            "group_id": group.0,
            "file_id": file_id,
            "parent_folder_id": source_folder_id.unwrap_or("/"),
            "new_file_name": new_name,
        });
        self.call("rename_group_file", params).await.map(drop)
    }

    async fn set_group_avatar(&self, group: Uin, src: ResourceSource) -> Result<()> {
        // IR set_group_avatar: {group_id, image_uri}（注意字段名 image_uri）。
        // OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/group.ts
        let params = json!({ "group_id": group.0, "image_uri": source_to_uri(&src) });
        self.call("set_group_avatar", params).await.map(drop)
    }

    async fn set_self_avatar(&self, src: ResourceSource) -> Result<()> {
        // IR set_avatar (since 1.1): {uri}。
        // OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/system.ts
        let params = json!({ "uri": source_to_uri(&src) });
        self.call("set_avatar", params).await.map(drop)
    }

    async fn set_self_nickname(&self, name: &str) -> Result<()> {
        // IR set_nickname (since 1.1): {new_nickname}（注意字段名 new_nickname）。
        // OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/system.ts
        let params = json!({ "new_nickname": name });
        self.call("set_nickname", params).await.map(drop)
    }

    async fn set_self_bio(&self, bio: &str) -> Result<()> {
        // IR set_bio (since 1.1): {new_bio}（注意字段名 new_bio）。
        // OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/system.ts
        let params = json!({ "new_bio": bio });
        self.call("set_bio", params).await.map(drop)
    }

    async fn get_csrf_token(&self) -> Result<String> {
        // IR get_csrf_token: {} → csrf_token。
        // OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/system.ts
        let data = self.call("get_csrf_token", json!({})).await?;
        Ok(data.get("csrf_token").and_then(Value::as_str).unwrap_or_default().to_string())
    }
}

#[async_trait]
impl MilkyActions for MilkyAdapter {
    async fn get_impl_info(&self) -> Result<ImplInfo> {
        // 主动重新拉取实现信息（连接时的 best-effort 探测之外，业务可按需刷新）。
        // 同时把最新结果写入缓存（OnceLock：已设置则忽略，保持首探结果稳定）。
        let data = self.call("get_impl_info", json!({})).await?;
        let info = Self::parse_impl_info(&data);
        let _ = self.impl_info.set(info.clone());
        Ok(info)
    }

    async fn send_friend_nudge(&self, user: Uin, is_self: bool) -> Result<()> {
        // Milky send_friend_nudge(user_id, is_self)：is_self=true → 戳 bot 自身；
        // is_self=false → 戳 user。与通用 send_nudge 不同，此处 is_self 由调用方显式给定，
        // 可对他人代戳，也可在 user != self 时独立触发「戳 bot 自己」。
        let params = json!({ "user_id": user.0, "is_self": is_self });
        self.call("send_friend_nudge", params).await.map(drop)
    }

    async fn get_friend_info(&self, user: Uin) -> Result<FriendInfo> {
        let params = json!({ "user_id": user.0, "no_cache": false });
        let data = self.call("get_friend_info", params).await?;
        // 响应内层 `friend`（IR refField），回退整 data 兜底。
        let f = data.get("friend").cloned().unwrap_or(data);
        let entity: FriendEntity = serde_json::from_value(f)?;
        Ok(friend_info(&entity))
    }

    async fn get_resource_url(&self, resource_id: &str) -> Result<String> {
        let params = json!({ "resource_id": resource_id });
        let data = self.call("get_resource_temp_url", params).await?;
        Ok(data.get("url").and_then(Value::as_str).unwrap_or_default().to_string())
    }

    async fn get_friend_requests(&self) -> Result<Vec<Request>> {
        // 委托给分页版,用 Milky 规范默认值(limit=20, is_filtered=false)。
        self.get_friend_requests_paged(20, false).await
    }

    async fn get_custom_face_url_list(&self) -> Result<Vec<String>> {
        // OFFICIAL: https://milky.ntqqrev.org/api/system (get_custom_face_url_list) → {urls}.
        let data = self.call("get_custom_face_url_list", json!({})).await?;
        Ok(data
            .get("urls")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default())
    }

    async fn get_friend_requests_paged(&self, limit: u32, is_filtered: bool) -> Result<Vec<Request>> {
        // OFFICIAL: https://milky.ntqqrev.org/api/friend (get_friend_requests) {limit, is_filtered}.
        let params = json!({ "limit": limit, "is_filtered": is_filtered });
        let data = self.call("get_friend_requests", params).await?;
        let arr = data.get("requests").and_then(Value::as_array).cloned().unwrap_or_default();
        Ok(arr.iter().map(friend_request_to_request).collect())
    }

    async fn get_group_notifications_paged(
        &self,
        start_seq: Option<i64>,
        limit: u32,
        is_filtered: bool,
    ) -> Result<(Vec<Request>, Option<i64>)> {
        // OFFICIAL: https://milky.ntqqrev.org/api/group (get_group_notifications)
        //   {start_notification_seq?, is_filtered, limit} → {notifications, next_notification_seq?}.
        let mut params = json!({ "limit": limit, "is_filtered": is_filtered });
        if let Some(s) = start_seq {
            params["start_notification_seq"] = Value::from(s);
        }
        let data = self.call("get_group_notifications", params).await?;
        let arr = data.get("notifications").and_then(Value::as_array).cloned().unwrap_or_default();
        let mut out = Vec::with_capacity(arr.len());
        for n in &arr {
            let ntype = n.get("type").and_then(Value::as_str).unwrap_or_default();
            if let Some(req) = notification_to_request(n, ntype) {
                out.push(req);
            }
        }
        let next = data.get("next_notification_seq").and_then(Value::as_i64);
        Ok((out, next))
    }

    async fn get_history_messages_paged(
        &self,
        peer: &Peer,
        start_seq: Option<i64>,
        limit: u32,
    ) -> Result<(Vec<MessageEvent>, Option<i64>)> {
        // IR get_history_messages: message_scene / peer_id / start_message_seq? / limit(≤30)
        //   → {messages, next_message_seq?}。
        // OFFICIAL: https://milky.ntqqrev.org/api/message (get_history_messages)
        let scene = scene_str(peer.scene);
        let mut params = json!({
            "message_scene": scene,
            "peer_id": peer.id.0,
            "limit": limit,
        });
        if let Some(s) = start_seq {
            params["start_message_seq"] = Value::from(s);
        }
        let data = self.call("get_history_messages", params).await?;
        let arr = data.get("messages").and_then(Value::as_array).cloned().unwrap_or_default();
        let self_uin = self.self_id.get().copied().unwrap_or(Uin(0));
        let mut out = Vec::with_capacity(arr.len());
        for raw in arr {
            let msg: IncomingMessage = serde_json::from_value(raw.clone())?;
            out.push(message_event_from_incoming(msg, self_uin, raw));
        }
        let next = data.get("next_message_seq").and_then(Value::as_i64);
        Ok((out, next))
    }

    async fn set_peer_pin(&self, peer: &Peer, pinned: bool) -> Result<()> {
        // IR set_peer_pin (since 1.2): {message_scene, peer_id, is_pinned}。
        // OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/system.ts
        let scene = scene_str(peer.scene);
        let params = json!({
            "message_scene": scene,
            "peer_id": peer.id.0,
            "is_pinned": pinned,
        });
        self.call("set_peer_pin", params).await.map(drop)
    }

    async fn get_peer_pins(&self) -> Result<(Vec<FriendInfo>, Vec<GroupInfo>)> {
        // IR get_peer_pins (since 1.2): {} → friends: FriendEntity[], groups: GroupEntity[]。
        // 反序列化完整实体并映射为统一 FriendInfo/GroupInfo（保留昵称/备注/成员数等字段）。
        // 单个实体解析失败 → 跳过该条（其余照常返回），整体不因一条坏数据失败。
        // OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/system.ts
        let data = self.call("get_peer_pins", json!({})).await?;
        let friends = data
            .get("friends")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|e| serde_json::from_value::<FriendEntity>(e.clone()).ok())
                    .map(|f| friend_info(&f))
                    .collect()
            })
            .unwrap_or_default();
        let groups = data
            .get("groups")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|e| serde_json::from_value::<GroupEntity>(e.clone()).ok())
                    .map(|g| group_info(&g))
                    .collect()
            })
            .unwrap_or_default();
        Ok((friends, groups))
    }

    async fn get_group_notices_paged(
        &self,
        start_seq: Option<i64>,
        limit: u32,
        is_filtered: bool,
    ) -> Result<(Vec<Notice>, Option<i64>)> {
        // 与 get_group_notifications_paged 同一 wire 动作，但只取 admin_change/kick/quit
        // 这些「非请求」变体并映射为 Notice（join/invited_join 留给 *_paged 当 Request）。
        // OFFICIAL: https://milky.ntqqrev.org/api/group (get_group_notifications)。
        let mut params = json!({ "limit": limit, "is_filtered": is_filtered });
        if let Some(s) = start_seq {
            params["start_notification_seq"] = Value::from(s);
        }
        let data = self.call("get_group_notifications", params).await?;
        let arr = data.get("notifications").and_then(Value::as_array).cloned().unwrap_or_default();
        let mut out = Vec::with_capacity(arr.len());
        for n in &arr {
            let ntype = n.get("type").and_then(Value::as_str).unwrap_or_default();
            if let Some(notice) = notification_to_notice(n, ntype) {
                out.push(notice);
            }
        }
        let next = data.get("next_notification_seq").and_then(Value::as_i64);
        Ok((out, next))
    }
}

// Milky 对 OneBot 独有/厂商扩展动作留单个空 impl(全走默认 unsupported)。
impl OneBotActions for MilkyAdapter {}
