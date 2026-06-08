//! 统一实体层：好友、用户档案、群、群成员、文件/文件夹、公告、精华、荣誉、运行状态/版本，
//! 以及若干厂商扩展结果（OCR、点赞、AI 音色、rkey 等）。每个实体取各协议字段的并集，本协议
//! 没有的字段为 `Option`（或空 `Vec`），并保留 `raw` 原始 JSON 以防协议追加字段。适配器把各自
//! wire 实体映射进来；同一字段在不同实现端的来源差异写在各结构体/字段的 `///` 上。
use crate::id::{MessageId, Uin};
use crate::message::Message;
use serde_json::Value;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sex {
    Male,
    Female,
    /// 未知或协议未提供——adapter 对未知值降级到此，绝不 panic。
    Unknown,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Owner,
    Admin,
    Member,
}

#[derive(Clone, Debug)]
pub struct FriendCategory {
    pub id: i32,
    pub name: String,
}

/// 好友分组（Lagrange `get_friend_list` 元素的 `group` 子对象）。
/// 与 `FriendCategory` 区别：`category` 来自 NapCat/LLOneBot 的 `categoryId/categroyName`
/// 扁平字段或 Milky 的嵌套 entity；`group` 是 Lagrange 私有的 `{group_id, group_name}`。
/// OFFICIAL: Lagrange.OneBot get_friend_list（friend.group → FriendGroup）。
#[derive(Clone, Debug)]
pub struct FriendGroup {
    pub group_id: i32,
    pub group_name: String,
}

#[derive(Clone, Debug)]
pub struct FriendInfo {
    pub user: Uin,
    pub nickname: String,
    pub sex: Sex,
    pub remark: String,
    pub qid: Option<String>,
    pub category: Option<FriendCategory>,
    /// Lagrange 好友分组 `{group_id, group_name}`（其余实现 → None）。
    pub group: Option<FriendGroup>,
    /// NapCat/LLOneBot 扩展：生日（YYYY-MM-DD，由 `birthday_year/month/day` 拼装；
    /// 任一为 0/缺 → None）。
    pub birthday: Option<String>,
    /// NapCat/LLOneBot 扩展：电话（wire `phoneNum`，空串/`-` → None）。
    pub phone: Option<String>,
    /// NapCat/LLOneBot 扩展：邮箱（wire `email`，空串 → None）。
    pub email: Option<String>,
    /// NapCat/LLOneBot 扩展：登录天数（wire `login_days`）。
    pub login_days: Option<i32>,
    /// NapCat/LLOneBot 扩展：个性签名长文（wire `longNick`/`long_nick`，空串 → None）。
    pub long_nick: Option<String>,
    pub raw: Value,
}

impl FriendInfo {
    /// 好友显示名：优先备注（remark），其次昵称（皆按 trim 后非空取）；两者皆空 → 空串。
    pub fn display_name(&self) -> &str {
        let remark = self.remark.trim();
        if !remark.is_empty() {
            remark
        } else {
            self.nickname.trim()
        }
    }
}

/// 用户在线/VIP 状态（Lagrange `get_stranger_info.status`）。
/// `status`/`ext_status` 为协议原始枚举值（不解释，原样透出）；
/// `battery_status` 为电量百分比。缺字段 → None（绝不 panic）。
/// OFFICIAL: Lagrange.OneBot get_stranger_info（stranger.status → FriendStatus）。
#[derive(Clone, Debug)]
pub struct FriendStatus {
    pub status: Option<i32>,
    pub ext_status: Option<i32>,
    pub battery_status: Option<i32>,
}

/// VIP / 业务徽章条目（Lagrange `get_stranger_info.Business[]`）。
/// `kind` 为业务类型（wire `type`），`level` 等级，`icon` 徽章图标 URL，
/// `is_pro`/`is_year` 为大会员/年费标识。缺字段 → None（绝不 panic）。
/// OFFICIAL: Lagrange.OneBot get_stranger_info（stranger.Business[] → Business）。
#[derive(Clone, Debug)]
pub struct Business {
    pub kind: Option<i32>,
    pub name: Option<String>,
    pub level: Option<i32>,
    pub icon: Option<String>,
    pub is_pro: Option<bool>,
    pub is_year: Option<bool>,
}

/// 任意用户档案（含陌生人）。OneBot `get_stranger_info` /
/// Milky `get_user_profile` 字段并集，缺的为 `Option`。
#[derive(Clone, Debug)]
pub struct UserInfo {
    pub user: Uin,
    pub nickname: String,
    pub sex: Sex,
    pub age: Option<i32>,
    pub level: Option<i32>,
    pub qid: Option<String>,
    /// 个性签名 / bio。
    pub bio: Option<String>,
    pub country: Option<String>,
    pub city: Option<String>,
    pub school: Option<String>,
    pub remark: Option<String>,
    /// 在线/VIP 状态（Lagrange `status`；其余实现 → None）。
    pub status: Option<FriendStatus>,
    /// VIP / 业务徽章列表（Lagrange `Business[]`；其余实现 → 空 Vec）。
    pub business: Vec<Business>,
    /// 注册时间戳（Lagrange `RegisterTime`，秒；缺 → None）。
    pub register_time: Option<i64>,
    /// 头像 URL（Lagrange `avatar`；其余实现 → None）。
    pub avatar: Option<String>,
    pub raw: Value,
}

#[derive(Clone, Debug)]
pub struct GroupInfo {
    pub group: Uin,
    pub name: String,
    pub member_count: i32,
    pub max_member_count: i32,
    pub remark: Option<String>,
    pub created_time: Option<i64>,
    pub description: Option<String>,
    pub announcement: Option<String>,
    /// 加群验证问题（Milky 1.2）。
    pub question: Option<String>,
    /// 群是否被冻结（LLOneBot `is_freeze`；其余实现无 → None）。
    pub is_freeze: Option<bool>,
    /// 活跃成员数（LLOneBot `active_member_count`；其余实现无 → None）。
    pub active_member_count: Option<i32>,
    /// 群是否置顶（LLOneBot `is_top`；其余实现无 → None）。
    pub is_top: Option<bool>,
    /// 群主 QQ 号（LLOneBot `owner_id`；其余实现无 → None）。
    pub owner_id: Option<Uin>,
    /// 全员禁言截止时间戳（LLOneBot `shut_up_all_timestamp`；其余实现无 → None）。
    pub shut_up_all_time: Option<i64>,
    /// 自身被禁言截止时间戳（LLOneBot `shut_up_me_timestamp`；其余实现无 → None）。
    pub shut_up_me_time: Option<i64>,
    pub raw: Value,
}

#[derive(Clone, Debug)]
pub struct MemberInfo {
    pub user: Uin,
    pub group: Uin,
    pub nickname: String,
    pub card: String,
    pub title: String,
    pub level: i32,
    pub role: Role,
    pub sex: Sex,
    /// 群成员年龄（OneBot `sender.age` / `get_group_member_info.age`；Milky 无 → None）。
    pub age: Option<i32>,
    pub join_time: i64,
    pub last_sent_time: Option<i64>,
    pub mute_end_time: Option<i64>,
    /// 群成员地区（OneBot `area`；Milky 无 → None）。
    pub area: Option<String>,
    /// 是否不良记录成员（OneBot `unfriendly`；Milky 无 → None）。
    pub unfriendly: Option<bool>,
    /// 专属头衔过期时间戳（OneBot `title_expire_time`；Milky 无 → None）。
    pub title_expire_time: Option<i64>,
    /// 是否允许修改群名片（OneBot `card_changeable`；Milky 无 → None）。
    pub card_changeable: Option<bool>,
    /// QQ 等级（LLOneBot `qq_level`；其余实现无 → None）。
    /// 注意：与 `level`（群等级）不同，这是账号 QQ 等级。
    pub qq_level: Option<i32>,
    /// 是否为机器人（LLOneBot `is_robot`；其余实现无 → None）。
    pub is_robot: Option<bool>,
    /// QQ 注册年龄/Q 龄（LLOneBot `qage`；其余实现无 → None）。
    pub qage: Option<i32>,
    pub raw: Value,
}

impl MemberInfo {
    /// 群成员显示名：优先群名片（card），其次昵称（皆按 trim 后非空取）；两者皆空 → 空串。
    pub fn display_name(&self) -> &str {
        let card = self.card.trim();
        if !card.is_empty() {
            card
        } else {
            self.nickname.trim()
        }
    }
}

#[derive(Clone, Debug)]
pub struct FileMeta {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub hash: Option<String>,
    /// 群文件业务 ID（OneBot `group_upload.file.busid` / `busid`；Milky 无 → None）。
    pub busid: Option<i64>,
    /// 上传者 QQ 号（OneBot go-cqhttp `uploader` / Milky `uploader_id`；未提供 → None）。
    pub uploader: Option<Uin>,
    /// 上传时间戳（OneBot go-cqhttp `upload_time` / Milky `uploaded_time`，秒；未提供 → None）。
    pub upload_time: Option<i64>,
    /// 过期/失效时间戳（OneBot go-cqhttp `dead_time`，永久文件为 0 / Milky `expire_time`，
    /// 秒；未提供 → None）。
    pub dead_time: Option<i64>,
    /// 下载次数（OneBot go-cqhttp `download_times` / Milky `downloaded_times`；未提供 → None）。
    pub download_times: Option<i32>,
    /// 父文件夹 ID（Milky `parent_folder_id`；OneBot 群文件元素无 → None）。
    pub parent_folder_id: Option<String>,
}

/// 群文件夹。
/// OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/file.ts (get_group_files)
#[derive(Clone, Debug)]
pub struct GroupFolder {
    pub id: String,
    pub name: String,
    pub file_count: Option<u32>,
    pub create_time: Option<i64>,
    /// 父文件夹 ID（Milky `parent_folder_id`；OneBot 无 → None）。
    pub parent_folder_id: Option<String>,
    /// 最后修改时间戳（Milky `last_modified_time`；OneBot 无 → None）。
    pub last_modified_time: Option<i64>,
    /// 创建者 QQ 号（Milky `creator_id`；OneBot 无 → None）。
    pub creator_id: Option<Uin>,
    pub raw: Value,
}

/// 群文件列表(某文件夹下的文件 + 子文件夹)。
#[derive(Clone, Debug)]
pub struct GroupFileList {
    pub files: Vec<FileMeta>,
    pub folders: Vec<GroupFolder>,
}

/// 群公告。
/// OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/group.ts (get_group_announcements)
/// ENDPOINT(OneBot): go-cqhttp `_get_group_notice`
#[derive(Clone, Debug)]
pub struct Announcement {
    pub id: String,
    pub group: Uin,
    pub sender: Uin,
    pub content: String,
    pub time: i64,
    pub image_url: Option<String>,
    pub raw: Value,
}

/// 精华消息。
/// OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/group.ts (get_group_essence_messages)
/// ENDPOINT(OneBot): go-cqhttp `get_essence_msg_list`
#[derive(Clone, Debug)]
pub struct EssenceMessage {
    pub group: Uin,
    pub message_id: MessageId,
    pub sender: Uin,
    pub sender_nick: String,
    pub operator: Uin,
    pub operator_time: i64,
    pub content: Message,
    pub raw: Value,
}

/// 收发统计（`get_status.stat`，LLOneBot/Lagrange/go-cqhttp 共有）。
/// `message_received`/`message_sent` 为累计收/发消息数；缺字段 → None（绝不 panic）。
/// OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/api/public.md
///   （go-cqhttp / LLOneBot 扩展 `stat` 块）。
#[derive(Clone, Debug)]
pub struct ImplStat {
    /// 累计收到消息数（`message_received`；缺 → None）。
    pub message_received: Option<i64>,
    /// 累计发送消息数（`message_sent`；缺 → None）。
    pub message_sent: Option<i64>,
}

/// 协议端运行状态(OneBot `get_status`)。
/// OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/api/public.md (get_status)
#[derive(Clone, Debug)]
pub struct ImplStatus {
    pub online: bool,
    pub good: bool,
    /// 收发统计（LLOneBot/Lagrange `stat{message_received,message_sent,...}`；
    /// 无该块的实现 → None，其余统计字段仍保留于 `raw`）。
    pub stat: Option<ImplStat>,
    pub raw: Value,
}

/// 协议端版本信息（OneBot `get_version_info`）。
/// OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/api/public.md (get_version_info)
#[derive(Clone, Debug)]
pub struct VersionInfo {
    pub app_name: String,
    pub app_version: String,
    pub protocol_version: String,
    pub raw: Value,
}

/// 合并转发发送结果（`send_forward_msg` / `send_group_forward_msg` /
/// `send_private_forward_msg`）。除落地消息 `message_id` 外，Lagrange 等端还会返回
/// 合并转发的 `forward_id`（即 resId），下游可凭此**二次引用**该合并转发（构造
/// `Forward::Ref` 再发）。OneBot v11 标准只规定返回 `message_id`，故 `forward_id`
/// 为 `Option`——端未回传 resId 时为 `None`（非错误）。
/// ENDPOINT: NapCat action/go-cqhttp/SendForwardMsg.ts；Lagrange.OneBot
///   Core/Operation/Message/SendForwardOperation.cs（resp 旁带 `forward_id`/`res_id`）。
#[derive(Clone, Debug)]
pub struct ForwardSendResult {
    /// 落地消息 ID（合并转发卡片本身）。
    pub message_id: MessageId,
    /// 合并转发引用 ID（Lagrange resId）。端未回传时为 `None`。
    pub forward_id: Option<String>,
}

/// OCR 识别出的一段文本（OneBot/NapCat `ocr_image`）。
/// ENDPOINT: NapCat packages/napcat-onebot/action/extends/OCRImage.ts (ocr_image)
///   (https://github.com/NapNeko/NapCatQQ)
#[derive(Clone, Debug)]
pub struct OcrText {
    pub text: String,
    /// 完整结果项（坐标/置信度等实现相关字段）。
    pub raw: Value,
}

/// 对某条消息做出表情回应的人（NapCat `fetch_emoji_like`）。
/// ENDPOINT: NapCat packages/napcat-onebot/action/extends/FetchEmojiLike.ts
///   (https://github.com/NapNeko/NapCatQQ)
#[derive(Clone, Debug)]
pub struct EmojiLiker {
    pub tiny_id: String,
    pub nickname: String,
    pub head_url: String,
}

/// 富媒体下载鉴权密钥（Lagrange/NapCat/LLOneBot `get_rkey`）。
/// ENDPOINT: LagrangeDev/Lagrange.Core Lagrange.OneBot/Core/Operation/Generic/GetRkey.cs
/// Wire response: `{"rkeys":[{"type":"private"|"group","rkey":"...","created_at":u32,"ttl":u64}]}`
#[derive(Clone, Debug)]
pub struct Rkey {
    /// "private" / "group"。
    pub kind: String,
    pub rkey: String,
    /// 创建时间戳（`created_at`）。
    pub create_time: Option<i64>,
    /// 有效期秒数（`ttl`）。
    pub ttl: Option<i64>,
    pub raw: Value,
}

/// `get_file` 取回的富媒体文件（NapCat / LLOneBot 共有）。
/// ENDPOINT: NapNeko/NapCatQQ packages/napcat-onebot/action/file/GetFile.ts (get_file)；
///   亦见 LLOneBot/LLOneBot src/onebot11/action/types.ts `get_file`。
/// Wire 响应：`{file?(本地路径), url?, file_size?(字符串), file_name?, base64?}`。
#[derive(Clone, Debug)]
pub struct FileFetch {
    /// 下载 URL（`url`）。
    pub url: Option<String>,
    /// 本地路径（`file`）。
    pub path: Option<String>,
    /// 文件名（`file_name`）。
    pub name: String,
    /// 文件大小字节数（`file_size`，wire 为字符串，解析为整数；无则 0）。
    pub size: u64,
    /// Base64 内容（`base64`，仅在服务端开启 local-file-to-url 时返回）。
    pub base64: Option<String>,
    pub raw: Value,
}

/// AI 语音音色（`get_ai_characters`）。
/// ENDPOINT: LagrangeDev/Lagrange.Core Lagrange.OneBot/Core/Operation/Generic/GetAiCharacters.cs
///   (https://github.com/LagrangeDev/Lagrange.Core); 亦见 NapCat/LLOneBot `get_ai_characters`。
/// Wire 元素：`{character_id, character_name, preview_url?}`。
#[derive(Clone, Debug)]
pub struct AiCharacter {
    pub id: String,
    pub name: String,
    pub preview_url: Option<String>,
    pub raw: Value,
}

/// 一组 AI 音色（按类型分组；`get_ai_characters` 的顶层数组元素）。
/// Wire 元素：`{type, characters:[AiCharacter]}`。
#[derive(Clone, Debug)]
pub struct AiCharacterGroup {
    pub kind: String,
    pub characters: Vec<AiCharacter>,
    pub raw: Value,
}

/// 对我点赞的用户（`get_profile_like`，NapCat/LLOneBot 共有）。
/// ENDPOINT: NapCat packages/napcat-onebot/action/user/GetProfileLike.ts (get_profile_like)
///   (https://github.com/NapNeko/NapCatQQ). Also: LLOneBot src/onebot11/action/types.ts.
#[derive(Clone, Debug)]
pub struct ProfileLiker {
    pub user: Uin,
    pub nickname: String,
    /// 点赞次数。
    pub times: i32,
    pub raw: Value,
}

/// 带分类的好友列表（`get_friends_with_category`，NapCat/LLOneBot 共有）。
/// ENDPOINT: NapCat packages/napcat-onebot/action/user/GetFriendsWithCategory.ts
///   (get_friends_with_category) (https://github.com/NapNeko/NapCatQQ).
///   Wire: `[{categoryId, categoryName, buddyList:[FriendInfo]}]`。
#[derive(Clone, Debug)]
pub struct FriendCategoryList {
    pub category_id: i32,
    pub category_name: String,
    pub friends: Vec<FriendInfo>,
}

/// 群荣誉成员条目（current_talkative / *_list 元素）。
/// OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/api/public.md (get_group_honor_info)
#[derive(Clone, Debug)]
pub struct HonorMember {
    pub user: Uin,
    pub nickname: String,
    pub avatar: Option<String>,
    pub description: Option<String>,
    pub day_count: Option<i32>,
    pub raw: Value,
}

/// 群荣誉信息（`get_group_honor_info`）。
/// OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/api/public.md (get_group_honor_info)
#[derive(Clone, Debug, Default)]
pub struct HonorList {
    pub group: Uin,
    pub current_talkative: Option<HonorMember>,
    pub talkative_list: Vec<HonorMember>,
    pub performer_list: Vec<HonorMember>,
    pub legend_list: Vec<HonorMember>,
    pub strong_newbie_list: Vec<HonorMember>,
    pub emotion_list: Vec<HonorMember>,
    pub raw: Value,
}

impl MemberInfo {
    /// 是否为群主或管理员。
    pub fn is_operator(&self) -> bool {
        matches!(self.role, Role::Owner | Role::Admin)
    }
}
