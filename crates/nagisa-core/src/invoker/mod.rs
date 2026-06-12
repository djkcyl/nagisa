//! 统一动作接口,分三层组织:
//!
//! - [`ActionInvoker`]（本文件 **根**）:**两协议通用**的动作(send / 群管理 /
//!   成员·好友·群信息 / 请求 / 文件 / 公告 / 精华 / 头像资料 …),约 50 个。
//! - [`OneBotActions`](onebot):**OneBot 协议独有**动作(get_status / set_restart /
//!   get_record …),含 OneBot v11 官方动作与各 OneBot 厂商(NapCat / LLOneBot / Lagrange)
//!   的私有/扩展动作(按厂商分节注释组织)。
//! - [`MilkyActions`](milky):**Milky 协议独有**动作(set_peer_pin / get_resource_url …)。
//! - [`Actions`]:三者的组合 marker;[`Bot`](crate::Bot) 持 `Arc<dyn Actions>`,**暴露全部**。在不支持该动作的协议上调用独有动作,默认返回 `Error::Unsupported`
//!   (与 `supports()` 探测一致)——故切协议时业务代码一字不改。
//!
//! 未实现的动作默认返回 `Error::Unsupported`,于是 V1(OneBot,富) 与 V2(Milky,稀疏)
//! 的能力差异天然由默认实现兜住——adapter 只 override 自己真正支持的动作。
use async_trait::async_trait;
use nagisa_types::event::ReactionKind;
use nagisa_types::prelude::*;
use nagisa_types::vendor::Vendor;
use serde_json::Value;

pub mod milky;
pub mod onebot;

pub use milky::MilkyActions;
pub use onebot::OneBotActions;

#[inline]
pub(crate) fn unsupported(action: &str) -> Error {
    Error::Unsupported(action.to_string())
}

/// 统一动作接口——**两协议通用**的动作。方法参数/返回都是 `nagisa-types`
/// 类型;adapter 在内部完成 编码(Message→wire) + 动作名映射 + 响应解包 + 错误归一。
///
/// 协议独有动作见 [`OneBotActions`] / [`MilkyActions`];三者由 [`Actions`] 组合。
///
/// 每个动作的 doc 标注它在各协议下的 wire 名（如 `OneBot <wire> / Milky <wire>`），据此
/// 判定归位：两协议都有对应 wire 动作 → 此「两协议通用」trait；仅单协议有 → 对应的独有
/// trait。[`OneBotActions`] 内再用 `// ===== <vendor> 专属 =====` 分节注释按厂商分组。
/// 动作的规范 URL 与 wire 参数/响应形态归 adapter 侧（编码所在地），不在此重复。
#[async_trait]
pub trait ActionInvoker: Send + Sync + 'static {
    /// 该 adapter 的协议。
    fn protocol(&self) -> Protocol;

    /// 探测到的实现厂商（用于 per-vendor 动作名 aliasing）。默认 `Vendor::Other`；
    /// OneBot adapter 在连接时由 `get_version_info.app_name` 覆写。
    fn vendor(&self) -> Vendor {
        Vendor::Other
    }

    /// 能力探测。默认全不支持;adapter override 返回真实能力。
    fn supports(&self, _cap: Capability) -> bool {
        false
    }

    // —— 必须实现 ——
    /// 向 `peer` 发送一条消息,返回消息 ID。
    /// OneBot `send_private_msg`/`send_group_msg`（按 `peer.scene` 派发）、
    /// Milky `send_private_message`/`send_group_message`。
    async fn send(&self, peer: &Peer, message: &[Segment]) -> Result<MessageId>;

    /// 协议私有动作逃生口:直接传 action 名 + JSON params,返回 data。
    ///
    /// **命名变体未利用（固有取舍，功能无碍）**：OneBot v11 允许在动作名后缀 `_async`
    /// （异步执行，立即返回 ack）与 `_rate_limited`（限速队列）两种变体。nagisa 的所有 typed
    /// 动作与 adapter 始终发**裸 `<name>`**（同步、非限速形），不暴露也不利用这两个后缀
    /// 变体。这在功能上无碍（裸形是 spec 基准、所有端必支持），仅意味着「异步/限速」语义
    /// 不在统一面上可达。如确需，调用方可经此 `call_raw` 手工拼 `"<name>_async"` 透传——
    /// 这是有意的接口收敛，非缺陷。
    async fn call_raw(&self, action: &str, params: Value) -> Result<Value>;

    // —— 可选实现(默认 Unsupported)——
    /// 撤回一条消息。OneBot `delete_msg`、Milky `recall_private_message`/`recall_group_message`
    /// （按 `id.peer.scene` 派发）。
    async fn recall(&self, _id: &MessageId) -> Result<()> {
        Err(unsupported("recall"))
    }
    /// 按 id 取回单条消息。OneBot `get_msg`、Milky `get_message`。
    async fn get_message(&self, _id: &MessageId) -> Result<MessageEvent> {
        Err(unsupported("get_message"))
    }
    /// 返回 (登录 QQ 号, 昵称)。OneBot `get_login_info`、Milky `get_login_info`。
    async fn get_login_info(&self) -> Result<(Uin, String)> {
        Err(unsupported("get_login_info"))
    }
    /// 群信息。OneBot `get_group_info`、Milky `get_group_info`。
    async fn get_group_info(&self, _group: Uin, _no_cache: bool) -> Result<GroupInfo> {
        Err(unsupported("get_group_info"))
    }
    /// 群列表。OneBot `get_group_list`、Milky `get_group_list`。
    async fn get_group_list(&self, _no_cache: bool) -> Result<Vec<GroupInfo>> {
        Err(unsupported("get_group_list"))
    }
    /// 群成员信息。OneBot `get_group_member_info`、Milky `get_group_member_info`。
    async fn get_group_member_info(&self, _group: Uin, _user: Uin, _no_cache: bool) -> Result<MemberInfo> {
        Err(unsupported("get_group_member_info"))
    }
    /// 群成员列表。OneBot `get_group_member_list`、Milky `get_group_member_list`。
    async fn get_group_member_list(&self, _group: Uin, _no_cache: bool) -> Result<Vec<MemberInfo>> {
        Err(unsupported("get_group_member_list"))
    }
    /// 好友列表。OneBot `get_friend_list`、Milky `get_friend_list`。
    async fn get_friend_list(&self, _no_cache: bool) -> Result<Vec<FriendInfo>> {
        Err(unsupported("get_friend_list"))
    }
    /// 禁言群成员 `duration` 秒(0 = 解除)。OneBot `set_group_ban`、Milky `set_group_member_mute`。
    async fn set_group_member_mute(&self, _group: Uin, _user: Uin, _duration: u32) -> Result<()> {
        Err(unsupported("set_group_member_mute"))
    }
    /// 全员禁言开关。OneBot `set_group_whole_ban`、Milky `set_group_whole_mute`。
    async fn set_group_whole_mute(&self, _group: Uin, _enable: bool) -> Result<()> {
        Err(unsupported("set_group_whole_mute"))
    }
    /// 设/撤群管理员。OneBot `set_group_admin`、Milky `set_group_member_admin`。
    async fn set_group_admin(&self, _group: Uin, _user: Uin, _enable: bool) -> Result<()> {
        Err(unsupported("set_group_admin"))
    }
    /// 设置群名片。OneBot `set_group_card`、Milky `set_group_member_card`。
    async fn set_group_member_card(&self, _group: Uin, _user: Uin, _card: &str) -> Result<()> {
        Err(unsupported("set_group_member_card"))
    }
    /// 设置群名。OneBot `set_group_name`、Milky `set_group_name`。
    async fn set_group_name(&self, _group: Uin, _name: &str) -> Result<()> {
        Err(unsupported("set_group_name"))
    }
    /// 踢出群成员。OneBot `set_group_kick`、Milky `kick_group_member`。
    async fn kick_group_member(&self, _group: Uin, _user: Uin, _reject_add: bool) -> Result<()> {
        Err(unsupported("kick_group_member"))
    }
    /// 处理(同意/拒绝)好友/入群请求。`token` 来自对应的 `Event::Request`。
    /// OneBot `set_friend_add_request`/`set_group_add_request`（按请求类型派发）、
    /// Milky `accept_friend_request`/`reject_friend_request` /
    /// `accept_group_request`/`reject_group_request` /
    /// `accept_group_invitation`/`reject_group_invitation`（按 token 内变体派发）。
    async fn handle_request(&self, _token: &RequestToken, _approve: bool, _reason: Option<&str>) -> Result<()> {
        Err(unsupported("handle_request"))
    }
    /// 群消息表情回应（贴/撤表情）。OneBot `set_group_reaction`、Milky `send_group_message_reaction`。
    async fn send_reaction(
        &self,
        _group: Uin,
        _seq: i64,
        _face_id: &str,
        _kind: ReactionKind,
        _is_add: bool,
    ) -> Result<()> {
        Err(unsupported("send_reaction"))
    }
    /// 戳一戳。OneBot `group_poke`/`friend_poke`（按 `peer.scene` 派发）、
    /// Milky `send_group_nudge`/`send_friend_nudge`（`is_self` 推断为 `target == self`，
    /// 显式 `is_self` 见 [`MilkyActions::send_friend_nudge`](MilkyActions::send_friend_nudge)）。
    async fn send_nudge(&self, _peer: &Peer, _target: Uin) -> Result<()> {
        Err(unsupported("send_nudge"))
    }
    /// 上传群文件,返回服务端分配的文件 id。
    /// `parent_folder_id` 为目标文件夹 id（`None` 表示根 "/"）。
    /// OneBot `upload_group_file`、Milky `upload_group_file`。
    async fn upload_group_file(
        &self,
        _group: Uin,
        _src: ResourceSource,
        _name: &str,
        _parent_folder_id: Option<&str>,
    ) -> Result<String> {
        Err(unsupported("upload_group_file"))
    }
    /// 上传私聊(好友)文件,返回服务端分配的文件 id。
    /// OneBot `upload_private_file`、Milky `upload_private_file`。
    async fn upload_private_file(&self, _user: Uin, _src: ResourceSource, _name: &str) -> Result<String> {
        Err(unsupported("upload_private_file"))
    }
    /// 任意用户档案(含陌生人)。OneBot `get_stranger_info` / Milky `get_user_profile`。
    async fn get_user_info(&self, _user: Uin, _no_cache: bool) -> Result<UserInfo> {
        Err(unsupported("get_user_info"))
    }
    /// 拉取历史消息(从 `start`(含)向前 `count` 条;`start=None` 表示最新)。
    /// OneBot `get_group_msg_history`/`get_friend_msg_history`、Milky `get_history_messages`。
    async fn get_message_history(
        &self,
        _peer: &Peer,
        _start: Option<&MessageId>,
        _count: u32,
    ) -> Result<Vec<MessageEvent>> {
        Err(unsupported("get_message_history"))
    }
    /// 退出(`dismiss=true` 时解散)群。OneBot `set_group_leave`、Milky `quit_group`。
    async fn leave_group(&self, _group: Uin, _dismiss: bool) -> Result<()> {
        Err(unsupported("leave_group"))
    }
    /// 设置群成员专属头衔。OneBot `set_group_special_title`、Milky `set_group_member_special_title`。
    /// `duration`: 有效期秒数；`-1` 表示永久（OneBot 线格式 `-1`；Milky 忽略此参数）。
    async fn set_group_member_special_title(
        &self,
        _group: Uin,
        _user: Uin,
        _title: &str,
        _duration: i64,
    ) -> Result<()> {
        Err(unsupported("set_group_member_special_title"))
    }
    /// 标记消息已读。OneBot `mark_msg_as_read`、Milky `mark_message_as_read`。
    async fn mark_message_as_read(&self, _peer: &Peer, _id: &MessageId) -> Result<()> {
        Err(unsupported("mark_message_as_read"))
    }
    /// 设置/取消精华消息(`Capability::Essence`)。
    /// OneBot `set_essence_msg`/`delete_essence_msg`、Milky `set_group_essence_message`。
    async fn set_essence(&self, _group: Uin, _id: &MessageId, _enable: bool) -> Result<()> {
        Err(unsupported("set_essence"))
    }
    /// 群文件下载直链。OneBot `get_group_file_url`、Milky `get_group_file_download_url`。
    async fn get_group_file_download_url(&self, _group: Uin, _file_id: &str) -> Result<String> {
        Err(unsupported("get_group_file_download_url"))
    }
    /// 删除群文件。OneBot/Milky `delete_group_file`。
    async fn delete_group_file(&self, _group: Uin, _file_id: &str) -> Result<()> {
        Err(unsupported("delete_group_file"))
    }
    /// 拉取待处理的入群/邀请请求(事件流之外的对账)。
    /// Milky `get_group_notifications`、OneBot `get_group_requests`。
    async fn get_group_requests(&self) -> Result<Vec<Request>> {
        Err(unsupported("get_group_requests"))
    }
    /// 取指定域的 Cookies(用于 web-API 集成)。OneBot/Milky `get_cookies`。
    async fn get_cookies(&self, _domain: Option<&str>) -> Result<String> {
        Err(unsupported("get_cookies"))
    }
    /// 取 CSRF/bkn token。OneBot `get_csrf_token` / Milky `get_csrf_token`。
    ///
    /// 契约取舍（**非协议缺陷**）：OneBot v11 spec 将 `token` 字段定义为 `int32`，但
    /// 各厂商实测既有发整数也有发字符串者（NapCat/LLOneBot 数值，部分端字符串化），
    /// Milky 直接给 `csrf_token` 字符串。为同时容忍 int/string 两形输入，本动作**统一返回
    /// `String`**——数值 token 会被字符串化，调用方需自行 `parse::<u32>()` 还原数值形。
    /// 这是有意的契约取舍（以宽松输入换取调用方一次 parse），不视作有损或缺陷。
    async fn get_csrf_token(&self) -> Result<String> {
        Err(unsupported("get_csrf_token"))
    }
    /// 取指定域的完整凭证 (cookies, csrf_token)。
    /// OneBot `get_credentials`（单次返回二者）;其它协议默认由 `get_cookies` +
    /// `get_csrf_token` 组合而成（两次调用）。
    async fn get_credentials(&self, domain: Option<&str>) -> Result<(String, String)> {
        let cookies = self.get_cookies(domain).await?;
        let csrf = self.get_csrf_token().await?;
        Ok((cookies, csrf))
    }
    /// 展开收到的合并转发为节点列表。
    /// OneBot `get_forward_msg` / Milky `get_forwarded_messages`。
    async fn get_forward_messages(&self, _forward_id: &str) -> Result<Vec<ForwardNode>> {
        Err(unsupported("get_forward_messages"))
    }
    /// 删除好友。OneBot(go-cqhttp) `delete_friend` / Milky `delete_friend`。
    async fn delete_friend(&self, _user: Uin) -> Result<()> {
        Err(unsupported("delete_friend"))
    }
    /// 给好友资料卡点赞 `count` 次。OneBot `send_like` / Milky `send_profile_like`。
    async fn send_profile_like(&self, _user: Uin, _count: u32) -> Result<()> {
        Err(unsupported("send_profile_like"))
    }
    /// 发布群公告,返回公告 id。Milky `send_group_announcement` / OneBot `_send_group_notice`。
    async fn send_group_announcement(
        &self,
        _group: Uin,
        _content: &str,
        _image: Option<ResourceSource>,
    ) -> Result<String> {
        Err(unsupported("send_group_announcement"))
    }
    /// 读取群公告。Milky `get_group_announcements` / OneBot `_get_group_notice`。
    async fn get_group_announcements(&self, _group: Uin) -> Result<Vec<Announcement>> {
        Err(unsupported("get_group_announcements"))
    }
    /// 删除群公告。Milky `delete_group_announcement` / OneBot `_del_group_notice`。
    async fn delete_group_announcement(&self, _group: Uin, _announcement_id: &str) -> Result<()> {
        Err(unsupported("delete_group_announcement"))
    }
    /// 列出群精华消息。OneBot `get_essence_msg_list` / Milky `get_group_essence_messages`。
    async fn get_essence_messages(&self, _group: Uin) -> Result<Vec<EssenceMessage>> {
        Err(unsupported("get_essence_messages"))
    }
    /// 列出群文件与子文件夹(`folder_id=None` 为根)。
    /// OneBot `get_group_root_files`/`get_group_files_by_folder` / Milky `get_group_files`。
    async fn get_group_files(&self, _group: Uin, _folder_id: Option<&str>) -> Result<GroupFileList> {
        Err(unsupported("get_group_files"))
    }
    /// 私聊文件下载直链。OneBot `get_private_file_url` / Milky `get_private_file_download_url`。
    async fn get_private_file_download_url(&self, _user: Uin, _file_id: &str, _hash: Option<&str>) -> Result<String> {
        Err(unsupported("get_private_file_download_url"))
    }
    /// 新建群文件夹,返回 folder id。OneBot `create_group_file_folder` / Milky `create_group_folder`。
    async fn create_group_folder(&self, _group: Uin, _name: &str) -> Result<String> {
        Err(unsupported("create_group_folder"))
    }
    /// 重命名群文件夹。OneBot `rename_group_file_folder` / Milky `rename_group_folder`。
    async fn rename_group_folder(&self, _group: Uin, _folder_id: &str, _new_name: &str) -> Result<()> {
        Err(unsupported("rename_group_folder"))
    }
    /// 删除群文件夹。OneBot `delete_group_folder` / Milky `delete_group_folder`。
    async fn delete_group_folder(&self, _group: Uin, _folder_id: &str) -> Result<()> {
        Err(unsupported("delete_group_folder"))
    }
    /// 移动群文件到目标文件夹（`None` 为根 "/"）。
    /// `source_folder_id`: 文件当前所在文件夹（`None` 表示根 "/"）。
    /// OneBot `move_group_file` / Milky `move_group_file`。
    async fn move_group_file(
        &self,
        _group: Uin,
        _file_id: &str,
        _source_folder_id: Option<&str>,
        _target_folder_id: Option<&str>,
    ) -> Result<()> {
        Err(unsupported("move_group_file"))
    }
    /// 重命名群文件。
    /// `source_folder_id`: 文件当前所在文件夹（`None` 表示根 "/"）。
    /// OneBot `rename_group_file` / Milky `rename_group_file`。
    async fn rename_group_file(
        &self,
        _group: Uin,
        _file_id: &str,
        _source_folder_id: Option<&str>,
        _new_name: &str,
    ) -> Result<()> {
        Err(unsupported("rename_group_file"))
    }
    /// 设置群头像。OneBot `set_group_portrait` / Milky `set_group_avatar`。
    async fn set_group_avatar(&self, _group: Uin, _src: ResourceSource) -> Result<()> {
        Err(unsupported("set_group_avatar"))
    }
    /// 设置机器人自身头像。OneBot `set_qq_avatar` / Milky `set_avatar`。
    async fn set_self_avatar(&self, _src: ResourceSource) -> Result<()> {
        Err(unsupported("set_self_avatar"))
    }
    /// 设置机器人自身昵称。OneBot `set_qq_profile` / Milky `set_nickname`。
    async fn set_self_nickname(&self, _name: &str) -> Result<()> {
        Err(unsupported("set_self_nickname"))
    }
    /// 设置机器人自身签名。OneBot `set_qq_profile` / Milky `set_bio`。
    async fn set_self_bio(&self, _bio: &str) -> Result<()> {
        Err(unsupported("set_self_bio"))
    }
}

/// 三层动作接口的组合 marker。[`Bot`](crate::Bot) 持 `Arc<dyn Actions>`,暴露全部
/// 动作(通用 [`ActionInvoker`] + OneBot 独有/厂商扩展 [`OneBotActions`] + Milky 独有
/// [`MilkyActions`])。任何同时实现三者的类型都自动实现 `Actions`。
///
/// **按厂商的别名接缝**:同一逻辑动作在不同厂商下 wire 名分歧(如 nudge →
/// Lagrange `group_poke`/`friend_poke` vs NapCat `send_poke`)时,由 adapter 内部用
/// [`ActionInvoker::vendor()`] 分支、配合 `call_alias` 回退解决;helper 见 `OneBotAdapter`。
pub trait Actions: ActionInvoker + OneBotActions + MilkyActions {}
impl<T: ActionInvoker + OneBotActions + MilkyActions> Actions for T {}
