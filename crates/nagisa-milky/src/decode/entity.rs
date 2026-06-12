//! 实体映射：Milky wire 实体 → 统一 `nagisa-types` 实体。性别/角色枚举、群/成员/好友信息
//! （`GroupEntity` / `GroupMemberEntity` / `FriendEntity`），以及动作返回里的群公告/精华消息/
//! 群文件/群文件夹（`*_from_value`）。Milky 未暴露的 LLOneBot/NapCat 扩展字段一律置 None，
//! 所有 wire 字段缺失均降级，绝不 panic。
use super::*;

pub fn sex_from_wire(s: WireSex) -> Sex {
    match s {
        WireSex::Male => Sex::Male,
        WireSex::Female => Sex::Female,
        WireSex::Unknown => Sex::Unknown,
    }
}

pub fn role_from_wire(r: WireRole) -> Role {
    match r {
        WireRole::Owner => Role::Owner,
        WireRole::Admin => Role::Admin,
        WireRole::Member | WireRole::Unknown => Role::Member,
    }
}

pub fn group_info(g: &GroupEntity) -> GroupInfo {
    GroupInfo {
        group: Uin(g.group_id),
        name: g.group_name.clone(),
        member_count: g.member_count,
        max_member_count: g.max_member_count,
        remark: g.remark.clone(),
        created_time: g.created_time,
        description: g.description.clone(),
        announcement: g.announcement.clone(),
        question: g.question.clone(),
        // Milky GroupEntity 不暴露 LLOneBot 的 is_freeze/active_member_count/
        // is_top/owner_id/shut_up_* 等扩展字段。
        is_freeze: None,
        active_member_count: None,
        is_top: None,
        owner_id: None,
        shut_up_all_time: None,
        shut_up_me_time: None,
        raw: serde_json::to_value(GroupEntityRaw(g)).unwrap_or(Value::Null),
    }
}

pub fn member_info(m: &GroupMemberEntity) -> MemberInfo {
    MemberInfo {
        user: Uin(m.user_id),
        group: Uin(m.group_id),
        nickname: m.nickname.clone(),
        card: m.card.clone(),
        title: m.title.clone(),
        level: m.level,
        role: role_from_wire(m.role),
        sex: sex_from_wire(m.sex),
        age: None,
        join_time: m.join_time,
        last_sent_time: m.last_sent_time,
        mute_end_time: m.shut_up_end_time,
        area: None,
        unfriendly: None,
        title_expire_time: None,
        card_changeable: None,
        // Milky GroupMemberEntity 不暴露 LLOneBot 的 qq_level/is_robot/qage。
        qq_level: None,
        is_robot: None,
        qage: None,
        raw: Value::Null,
    }
}

pub fn friend_info(f: &FriendEntity) -> FriendInfo {
    FriendInfo {
        user: Uin(f.user_id),
        nickname: f.nickname.clone(),
        sex: sex_from_wire(f.sex),
        remark: f.remark.clone(),
        qid: (!f.qid.is_empty()).then(|| f.qid.clone()),
        category: f.category.as_ref().map(|c| FriendCategory { id: c.category_id, name: c.category_name.clone() }),
        // Milky IR FriendEntity 既无 Lagrange 那种 `group`,也无 NapCat/LLOneBot 的
        // birthday/phone/email/login_days/long_nick 扩展字段。
        group: None,
        birthday: None,
        phone: None,
        email: None,
        login_days: None,
        long_nick: None,
        raw: Value::Null,
    }
}

/// 把一条 `GroupAnnouncementEntity`（来自 `get_group_announcements`）映射为
/// `Announcement`。字段宽松解析；缺省图片 URL → None。
/// OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/group.ts (get_group_announcements)
pub fn announcement_from_value(data: &Value) -> nagisa_types::entity::Announcement {
    let image_url = get_str(data, "image_url");
    nagisa_types::entity::Announcement {
        // IR GroupAnnouncementEntity: announcement_id / group_id / user_id(发送者) / time / content / image_url?。
        id: get_str(data, "announcement_id"),
        group: Uin(get_i64(data, "group_id")),
        sender: Uin(get_i64(data, "user_id")),
        content: get_str(data, "content"),
        time: get_i64(data, "time"),
        image_url: (!image_url.is_empty()).then_some(image_url),
        raw: data.clone(),
    }
}

/// 把一条 `GroupEssenceMessage`（来自 `get_group_essence_messages`）映射为
/// `EssenceMessage`。`segments` 经既有段 decode 转为统一内容。
/// OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/group.ts (get_group_essence_messages)
pub fn essence_message_from_value(data: &Value) -> nagisa_types::entity::EssenceMessage {
    let group = Uin(get_i64(data, "group_id"));
    let peer = Peer { scene: Scene::Group, id: group };
    let message_seq = get_i64(data, "message_seq");
    // segments 是 IncomingSegment 列表；解析失败则降级为空内容（宽松）。
    let segments: Vec<IncomingSegment> =
        data.get("segments").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
    nagisa_types::entity::EssenceMessage {
        group,
        message_id: MessageId::from_seq(peer, message_seq),
        sender: Uin(get_i64(data, "sender_id")),
        sender_nick: get_str(data, "sender_name"),
        operator: Uin(get_i64(data, "operator_id")),
        operator_time: get_i64(data, "operation_time"),
        content: decode_segments(&segments, peer),
        raw: data.clone(),
    }
}

/// 把一条 `GroupFileEntity`（来自 `get_group_files`）映射为 `FileMeta`。
/// OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/file.ts (get_group_files)
pub fn file_meta_from_group_file(data: &Value) -> nagisa_types::entity::FileMeta {
    // GroupFileEntity 无 file_hash/busid 字段，故 hash/busid = None。
    // IR 字段：parent_folder_id/uploaded_time/expire_time?(可选)/uploader_id/downloaded_times。
    nagisa_types::entity::FileMeta {
        id: get_str(data, "file_id"),
        name: get_str(data, "file_name"),
        size: get_i64(data, "file_size").max(0) as u64,
        hash: None,
        busid: None,
        uploader: get_opt_i64(data, "uploader_id").map(Uin),
        upload_time: get_opt_i64(data, "uploaded_time"),
        dead_time: get_opt_i64(data, "expire_time"),
        download_times: get_opt_i64(data, "downloaded_times").map(|x| x as i32),
        parent_folder_id: get_opt_str(data, "parent_folder_id"),
    }
}

/// 把一条 `GroupFolderEntity`（来自 `get_group_files`）映射为 `GroupFolder`。
/// OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/file.ts (get_group_files)
pub fn group_folder_from_value(data: &Value) -> nagisa_types::entity::GroupFolder {
    nagisa_types::entity::GroupFolder {
        id: get_str(data, "folder_id"),
        name: get_str(data, "folder_name"),
        file_count: data.get("file_count").and_then(Value::as_i64).map(|n| n.max(0) as u32),
        create_time: get_opt_i64(data, "created_time"),
        // IR GroupFolderEntity: parent_folder_id/last_modified_time/creator_id。
        parent_folder_id: get_opt_str(data, "parent_folder_id"),
        last_modified_time: get_opt_i64(data, "last_modified_time"),
        creator_id: get_opt_i64(data, "creator_id").map(Uin),
        raw: data.clone(),
    }
}

/// 仅为给 `GroupInfo.raw` 一个完整 JSON 视图（避免 GroupEntity 派生 Serialize）。
struct GroupEntityRaw<'a>(&'a GroupEntity);
impl serde::Serialize for GroupEntityRaw<'_> {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let g = self.0;
        let mut m = ser.serialize_map(None)?;
        m.serialize_entry("group_id", &g.group_id)?;
        m.serialize_entry("group_name", &g.group_name)?;
        m.serialize_entry("member_count", &g.member_count)?;
        m.serialize_entry("max_member_count", &g.max_member_count)?;
        m.serialize_entry("remark", &g.remark)?;
        m.serialize_entry("created_time", &g.created_time)?;
        m.serialize_entry("description", &g.description)?;
        m.serialize_entry("question", &g.question)?;
        m.serialize_entry("announcement", &g.announcement)?;
        m.end()
    }
}
