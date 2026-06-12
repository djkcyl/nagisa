//! Milky 1.2 wire 类型（宽松 serde）。
//!
//! 收发段结构不对称，故保留双结构：入站 [`IncomingSegment`] 与出站 [`OutgoingSegment`]。
//! 另含响应/事件封包（[`ResponseEnvelope`] / [`EventEnvelope`]）、
//! 枚举值类型与实体（friend/group/member 等），供 [`decode`](crate::decode) /
//! [`encode`](crate::encode) 两侧映射。
//!
//! 宽松原则：所有闭合值枚举都带 `#[serde(other)] Unknown` 兜底，未知段经手写
//! `Deserialize` 落到 [`IncomingSegment::Unknown`]（保留 type + data），**绝不**
//! `deny_unknown_fields`。这样入站结构性破坏才会降级为 `Event::Raw`/`Segment::Raw` 而非整条
//! 丢弃。
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ───────────────────────── 响应封包 ─────────────────────────

/// Milky 动作响应封包。ok=`{status,retcode,data}`、failed=`{status,retcode,message}`。
/// 失败时无 `data`，故两者皆 `Option`，宽松解析。
#[derive(Debug, Clone, Deserialize)]
pub struct ResponseEnvelope {
    pub status: String,
    #[serde(default)]
    pub retcode: i64,
    #[serde(default)]
    pub data: Option<Value>,
    #[serde(default)]
    pub message: Option<String>,
}

// ───────────────────────── 事件封包 ─────────────────────────

/// 事件外层封包 `{time, self_id, event_type, data}`（邻接 tag）。
#[derive(Debug, Clone, Deserialize)]
pub struct EventEnvelope {
    pub time: i64,
    pub self_id: i64,
    pub event_type: String,
    /// `data` 因事件类型而异；先取原始 `Value`，再按 `event_type` 分发。
    #[serde(default)]
    pub data: Value,
}

// ───────────────────────── 枚举值类型 ─────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageScene {
    Friend,
    Group,
    Temp,
    /// 未知场景兜底，绝不 panic。
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireSex {
    Male,
    Female,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireRole {
    Owner,
    Admin,
    Member,
    /// Lagrange 对未知 role 抛错；这里降级兜底（按 member 处理）。
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireReactionType {
    Face,
    Emoji,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireImageSubType {
    Normal,
    Sticker,
    #[serde(other)]
    Unknown,
}

// ───────────────────────── 实体 ─────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct FriendCategoryEntity {
    #[serde(default)]
    pub category_id: i32,
    #[serde(default)]
    pub category_name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FriendEntity {
    pub user_id: i64,
    #[serde(default)]
    pub nickname: String,
    #[serde(default = "default_sex")]
    pub sex: WireSex,
    #[serde(default)]
    pub qid: String,
    #[serde(default)]
    pub remark: String,
    #[serde(default)]
    pub category: Option<FriendCategoryEntity>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GroupEntity {
    pub group_id: i64,
    #[serde(default)]
    pub group_name: String,
    #[serde(default)]
    pub member_count: i32,
    #[serde(default)]
    pub max_member_count: i32,
    #[serde(default)]
    pub remark: Option<String>,
    #[serde(default)]
    pub created_time: Option<i64>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub question: Option<String>,
    #[serde(default)]
    pub announcement: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GroupMemberEntity {
    pub user_id: i64,
    #[serde(default)]
    pub nickname: String,
    #[serde(default = "default_sex")]
    pub sex: WireSex,
    pub group_id: i64,
    #[serde(default)]
    pub card: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub level: i32,
    #[serde(default = "default_role")]
    pub role: WireRole,
    #[serde(default)]
    pub join_time: i64,
    #[serde(default)]
    pub last_sent_time: Option<i64>,
    #[serde(default)]
    pub shut_up_end_time: Option<i64>,
}

fn default_sex() -> WireSex {
    WireSex::Unknown
}
fn default_role() -> WireRole {
    WireRole::Member
}

// ───────────────────────── 接收消息 ─────────────────────────

/// 接收消息（按 `message_scene` 分发的 plain union）。
/// 自定义反序列化：先到 `Value`，按 sibling `message_scene` 选择变体，
/// **propagate serde 错误**（不吞错）。
#[derive(Debug, Clone)]
pub enum IncomingMessage {
    Friend(FriendMessage),
    Group(GroupMessage),
    Temp(TempMessage),
}

#[derive(Debug, Clone, Deserialize)]
pub struct IncomingMessageBase {
    pub peer_id: i64,
    pub message_seq: i64,
    pub sender_id: i64,
    pub time: i64,
    #[serde(default)]
    pub segments: Vec<IncomingSegment>,
    pub message_scene: MessageScene,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FriendMessage {
    #[serde(flatten)]
    pub base: IncomingMessageBase,
    pub friend: FriendEntity,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GroupMessage {
    #[serde(flatten)]
    pub base: IncomingMessageBase,
    pub group: GroupEntity,
    pub group_member: GroupMemberEntity,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TempMessage {
    #[serde(flatten)]
    pub base: IncomingMessageBase,
    #[serde(default)]
    pub group: Option<GroupEntity>,
}

impl IncomingMessage {
    pub fn base(&self) -> &IncomingMessageBase {
        match self {
            IncomingMessage::Friend(m) => &m.base,
            IncomingMessage::Group(m) => &m.base,
            IncomingMessage::Temp(m) => &m.base,
        }
    }
}

impl<'de> Deserialize<'de> for IncomingMessage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;
        // Value 暂存后按 message_scene 分发——错误向上 propagate，绝不吞掉。
        let value = Value::deserialize(deserializer)?;
        let scene = value.get("message_scene").and_then(Value::as_str);
        match scene {
            Some("friend") => serde_json::from_value(value).map(IncomingMessage::Friend).map_err(D::Error::custom),
            Some("group") => serde_json::from_value(value).map(IncomingMessage::Group).map_err(D::Error::custom),
            Some("temp") => serde_json::from_value(value).map(IncomingMessage::Temp).map_err(D::Error::custom),
            other => Err(D::Error::custom(format!("unknown or missing message_scene: {other:?}"))),
        }
    }
}

// ───────────────────────── 接收消息段 ─────────────────────────

/// 接收消息段（snake_case，`{type, data}`）。未知段 → `Unknown`（保留 type + data）。
#[derive(Debug, Clone)]
pub enum IncomingSegment {
    Text {
        text: String,
    },
    Mention {
        user_id: i64,
        name: Option<String>,
    },
    MentionAll {},
    Face {
        face_id: String,
        is_large: bool,
    },
    Reply {
        message_seq: i64,
        sender_id: Option<i64>,
        sender_name: Option<String>,
        time: Option<i64>,
        /// IR 1.2:被引用消息的内容。
        segments: Vec<IncomingSegment>,
    },
    Image {
        resource_id: String,
        temp_url: String,
        width: i32,
        height: i32,
        summary: String,
        sub_type: WireImageSubType,
    },
    Record {
        resource_id: String,
        temp_url: String,
        duration: i32,
    },
    Video {
        resource_id: String,
        temp_url: String,
        width: i32,
        height: i32,
        duration: i32,
    },
    File {
        file_id: String,
        file_name: String,
        file_size: i64,
        file_hash: Option<String>,
    },
    Forward {
        forward_id: String,
        title: Option<String>,
        preview: Vec<String>,
        summary: Option<String>,
    },
    MarketFace {
        emoji_package_id: i32,
        emoji_id: String,
        key: String,
        summary: String,
        url: String,
    },
    LightApp {
        app_name: String,
        json_payload: String,
    },
    Xml {
        service_id: i32,
        xml_payload: String,
    },
    /// 未知段兜底：保留原始 type 字符串和 data，decode 时产生 `Segment::Raw`。
    Unknown {
        kind: String,
        data: Value,
    },
}

/// 内部辅助枚举：仅用于已知变体的 serde 派生（`{type, data}`）。
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
enum KnownIncomingSegment {
    Text {
        #[serde(default)]
        text: String,
    },
    Mention {
        user_id: i64,
        #[serde(default)]
        name: Option<String>,
    },
    MentionAll {},
    Face {
        face_id: String,
        #[serde(default)]
        is_large: bool,
    },
    Reply {
        message_seq: i64,
        #[serde(default)]
        sender_id: Option<i64>,
        #[serde(default)]
        sender_name: Option<String>,
        #[serde(default)]
        time: Option<i64>,
        #[serde(default)]
        segments: Vec<IncomingSegment>,
    },
    Image {
        #[serde(default)]
        resource_id: String,
        #[serde(default)]
        temp_url: String,
        #[serde(default)]
        width: i32,
        #[serde(default)]
        height: i32,
        #[serde(default)]
        summary: String,
        #[serde(default = "default_image_sub_type")]
        sub_type: WireImageSubType,
    },
    Record {
        #[serde(default)]
        resource_id: String,
        #[serde(default)]
        temp_url: String,
        #[serde(default)]
        duration: i32,
    },
    Video {
        #[serde(default)]
        resource_id: String,
        #[serde(default)]
        temp_url: String,
        #[serde(default)]
        width: i32,
        #[serde(default)]
        height: i32,
        #[serde(default)]
        duration: i32,
    },
    File {
        file_id: String,
        #[serde(default)]
        file_name: String,
        #[serde(default)]
        file_size: i64,
        #[serde(default)]
        file_hash: Option<String>,
    },
    Forward {
        forward_id: String,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        preview: Vec<String>,
        #[serde(default)]
        summary: Option<String>,
    },
    MarketFace {
        #[serde(default)]
        emoji_package_id: i32,
        #[serde(default)]
        emoji_id: String,
        #[serde(default)]
        key: String,
        #[serde(default)]
        summary: String,
        #[serde(default)]
        url: String,
    },
    LightApp {
        #[serde(default)]
        app_name: String,
        #[serde(default)]
        json_payload: String,
    },
    Xml {
        #[serde(default)]
        service_id: i32,
        #[serde(default)]
        xml_payload: String,
    },
}

/// 已知变体 type 字符串集合。
const KNOWN_SEGMENT_TYPES: &[&str] = &[
    "text",
    "mention",
    "mention_all",
    "face",
    "reply",
    "image",
    "record",
    "video",
    "file",
    "forward",
    "market_face",
    "light_app",
    "xml",
];

impl<'de> Deserialize<'de> for IncomingSegment {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        let raw = Value::deserialize(deserializer)?;
        let type_str = raw.get("type").and_then(Value::as_str).unwrap_or_default();

        if KNOWN_SEGMENT_TYPES.contains(&type_str) {
            let known = serde_json::from_value::<KnownIncomingSegment>(raw).map_err(D::Error::custom)?;
            return Ok(known.into());
        }

        // 未知段：捕获 type 和 data，保留完整信息。
        let kind = type_str.to_string();
        let data = raw.get("data").cloned().unwrap_or(Value::Null);
        Ok(IncomingSegment::Unknown { kind, data })
    }
}

impl From<KnownIncomingSegment> for IncomingSegment {
    fn from(k: KnownIncomingSegment) -> Self {
        match k {
            KnownIncomingSegment::Text { text } => IncomingSegment::Text { text },
            KnownIncomingSegment::Mention { user_id, name } => IncomingSegment::Mention { user_id, name },
            KnownIncomingSegment::MentionAll {} => IncomingSegment::MentionAll {},
            KnownIncomingSegment::Face { face_id, is_large } => IncomingSegment::Face { face_id, is_large },
            KnownIncomingSegment::Reply { message_seq, sender_id, sender_name, time, segments } => {
                IncomingSegment::Reply { message_seq, sender_id, sender_name, time, segments }
            }
            KnownIncomingSegment::Image { resource_id, temp_url, width, height, summary, sub_type } => {
                IncomingSegment::Image { resource_id, temp_url, width, height, summary, sub_type }
            }
            KnownIncomingSegment::Record { resource_id, temp_url, duration } => {
                IncomingSegment::Record { resource_id, temp_url, duration }
            }
            KnownIncomingSegment::Video { resource_id, temp_url, width, height, duration } => {
                IncomingSegment::Video { resource_id, temp_url, width, height, duration }
            }
            KnownIncomingSegment::File { file_id, file_name, file_size, file_hash } => {
                IncomingSegment::File { file_id, file_name, file_size, file_hash }
            }
            KnownIncomingSegment::Forward { forward_id, title, preview, summary } => {
                IncomingSegment::Forward { forward_id, title, preview, summary }
            }
            KnownIncomingSegment::MarketFace { emoji_package_id, emoji_id, key, summary, url } => {
                IncomingSegment::MarketFace { emoji_package_id, emoji_id, key, summary, url }
            }
            KnownIncomingSegment::LightApp { app_name, json_payload } => {
                IncomingSegment::LightApp { app_name, json_payload }
            }
            KnownIncomingSegment::Xml { service_id, xml_payload } => IncomingSegment::Xml { service_id, xml_payload },
        }
    }
}

fn default_image_sub_type() -> WireImageSubType {
    WireImageSubType::Normal
}

// ───────────────────────── 发送消息段 ─────────────────────────

/// 发送消息段（snake_case，`{type, data}`）。encode 把统一 `Segment` 翻译成它。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum OutgoingSegment {
    Text {
        text: String,
    },
    Mention {
        user_id: i64,
    },
    MentionAll {},
    Face {
        face_id: String,
        is_large: bool,
    },
    Reply {
        message_seq: i64,
    },
    Image {
        uri: String,
        sub_type: WireOutImageSubType,
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },
    Record {
        uri: String,
    },
    Video {
        uri: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        thumb_uri: Option<String>,
    },
    Forward {
        messages: Vec<OutgoingForwardedMessage>,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        preview: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        prompt: Option<String>,
    },
    LightApp {
        json_payload: String,
    },
}

/// 发送侧图片子类型（无 `Unknown`，发送只产生合法值）。
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WireOutImageSubType {
    Normal,
    Sticker,
}

#[derive(Debug, Clone, Serialize)]
pub struct OutgoingForwardedMessage {
    pub user_id: i64,
    pub sender_name: String,
    pub segments: Vec<OutgoingSegment>,
}
