//! 统一 Bot 句柄：组合 `Actions`（= 通用 `ActionInvoker` + 两协议独有扩展），
//! 把全部跨协议的类型化动作暴露给业务。
use crate::invoker::Actions;
use nagisa_types::event::ReactionKind;
use nagisa_types::prelude::*;
use serde_json::Value;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// 「出站消息」日志器:拿到 `(peer, 段, self_id, 已发出的 MessageId)`。nagisa-log 装一个据此合成
/// 一条 `is_self` 的 `MessageEvent`、走与入站完全相同的记录/渲染管线(同一 `render_line`、同一
/// `NameStore`、同一最近消息缓存、同一开关/级别);业务侧也可再装(如把出站消息落库)。
///
/// 多订阅:每个 `Bot::send` 成功后,按注册顺序挨个调用全部日志器。一个都没装时退回 debug 记一行
/// (仅 peer + 段数)。日志器都在启动时注册,运行期只读遍历。
type OutgoingLogger = Box<dyn Fn(&Peer, &[Segment], Uin, &MessageId) + Send + Sync>;
static OUTGOING_LOGGERS: OnceLock<Mutex<Vec<OutgoingLogger>>> = OnceLock::new();

fn loggers() -> &'static Mutex<Vec<OutgoingLogger>> {
    OUTGOING_LOGGERS.get_or_init(|| Mutex::new(Vec::new()))
}

/// 追加一个出站消息日志器。多次调用按顺序累积(多订阅:nagisa-log、业务落库可并存)。
pub fn add_outgoing_logger(f: OutgoingLogger) {
    if let Ok(mut v) = loggers().lock() {
        v.push(f);
    }
}

/// [`add_outgoing_logger`] 的同义入口(历史名,nagisa-log 在用)。同样是追加,不再独占。
pub fn set_outgoing_logger(f: OutgoingLogger) {
    add_outgoing_logger(f);
}

/// 记一条**已成功发出**的出站消息:有日志器就逐个调用,一个都没装时 debug 兜底(旧行为)。
fn log_outgoing(peer: &Peer, message: &[Segment], self_id: Uin, id: &MessageId) {
    let guard = match loggers().lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if guard.is_empty() {
        tracing::debug!(peer = ?peer, segments = message.len(), "发送消息");
        return;
    }
    for log in guard.iter() {
        log(peer, message, self_id, id);
    }
}

/// 廉价可克隆的 bot 句柄（内部 `Arc<dyn Actions>`）。
///
/// `self_id` 存在 `Arc<AtomicI64>` 中，所有克隆共享同一个原子值，
/// `set_self_id` 的更新对所有副本立即可见。
#[derive(Clone)]
pub struct Bot {
    inner: Arc<dyn Actions>,
    self_id: Arc<AtomicI64>,
}

impl Bot {
    pub fn new(inner: Arc<dyn Actions>, self_id: Uin) -> Self {
        Self {
            inner,
            self_id: Arc::new(AtomicI64::new(self_id.0)),
        }
    }

    pub fn self_id(&self) -> Uin {
        Uin(self.self_id.load(Ordering::Relaxed))
    }

    /// 更新机器人自身 UIN（从消息事件的 `self_id` 字段学习）。
    pub fn set_self_id(&self, id: Uin) {
        self.self_id.store(id.0, Ordering::Relaxed);
    }

    pub fn protocol(&self) -> Protocol {
        self.inner.protocol()
    }
    pub fn vendor(&self) -> nagisa_types::vendor::Vendor {
        self.inner.vendor()
    }
    pub fn supports(&self, cap: Capability) -> bool {
        self.inner.supports(cap)
    }

    pub async fn send(&self, peer: &Peer, message: &[Segment]) -> Result<MessageId> {
        // 发成功后再记(失败的不记;拿到真实 MessageId 供合成事件 + 防撤回缓存)。
        let id = self.inner.send(peer, message).await?;
        log_outgoing(peer, message, self.self_id(), &id);
        Ok(id)
    }
    pub async fn recall(&self, id: &MessageId) -> Result<()> {
        self.inner.recall(id).await
    }
    pub async fn get_message(&self, id: &MessageId) -> Result<MessageEvent> {
        self.inner.get_message(id).await
    }
    pub async fn get_login_info(&self) -> Result<(Uin, String)> {
        self.inner.get_login_info().await
    }
    pub async fn get_group_info(&self, group: Uin, no_cache: bool) -> Result<GroupInfo> {
        self.inner.get_group_info(group, no_cache).await
    }
    pub async fn get_group_list(&self, no_cache: bool) -> Result<Vec<GroupInfo>> {
        self.inner.get_group_list(no_cache).await
    }
    pub async fn get_group_member_info(
        &self,
        group: Uin,
        user: Uin,
        no_cache: bool,
    ) -> Result<MemberInfo> {
        self.inner.get_group_member_info(group, user, no_cache).await
    }
    pub async fn get_group_member_list(&self, group: Uin, no_cache: bool) -> Result<Vec<MemberInfo>> {
        self.inner.get_group_member_list(group, no_cache).await
    }
    pub async fn get_friend_list(&self, no_cache: bool) -> Result<Vec<FriendInfo>> {
        self.inner.get_friend_list(no_cache).await
    }
    pub async fn set_group_member_mute(&self, group: Uin, user: Uin, duration: u32) -> Result<()> {
        self.inner.set_group_member_mute(group, user, duration).await
    }
    pub async fn set_group_whole_mute(&self, group: Uin, enable: bool) -> Result<()> {
        self.inner.set_group_whole_mute(group, enable).await
    }
    pub async fn set_group_admin(&self, group: Uin, user: Uin, enable: bool) -> Result<()> {
        self.inner.set_group_admin(group, user, enable).await
    }
    pub async fn set_group_member_card(&self, group: Uin, user: Uin, card: &str) -> Result<()> {
        self.inner.set_group_member_card(group, user, card).await
    }
    pub async fn set_group_name(&self, group: Uin, name: &str) -> Result<()> {
        self.inner.set_group_name(group, name).await
    }
    pub async fn kick_group_member(&self, group: Uin, user: Uin, reject_add: bool) -> Result<()> {
        self.inner.kick_group_member(group, user, reject_add).await
    }
    pub async fn handle_request(
        &self,
        token: &RequestToken,
        approve: bool,
        reason: Option<&str>,
    ) -> Result<()> {
        self.inner.handle_request(token, approve, reason).await
    }
    pub async fn send_reaction(
        &self,
        group: Uin,
        seq: i64,
        face_id: &str,
        kind: ReactionKind,
        is_add: bool,
    ) -> Result<()> {
        self.inner.send_reaction(group, seq, face_id, kind, is_add).await
    }
    pub async fn send_nudge(&self, peer: &Peer, target: Uin) -> Result<()> {
        self.inner.send_nudge(peer, target).await
    }
    /// 上传群文件，返回服务端分配的文件 id。
    /// `parent_folder_id`：目标文件夹（`None` 表示根 "/"）。
    pub async fn upload_group_file(
        &self,
        group: Uin,
        src: ResourceSource,
        name: &str,
        parent_folder_id: Option<&str>,
    ) -> Result<String> {
        self.inner.upload_group_file(group, src, name, parent_folder_id).await
    }
    /// 上传私聊（好友）文件，返回服务端分配的文件 id。
    pub async fn upload_private_file(
        &self,
        user: Uin,
        src: ResourceSource,
        name: &str,
    ) -> Result<String> {
        self.inner.upload_private_file(user, src, name).await
    }
    /// 任意用户档案（含陌生人）。
    pub async fn get_user_info(&self, user: Uin, no_cache: bool) -> Result<UserInfo> {
        self.inner.get_user_info(user, no_cache).await
    }
    /// 单个好友信息。
    pub async fn get_friend_info(&self, user: Uin) -> Result<FriendInfo> {
        self.inner.get_friend_info(user).await
    }
    /// 拉取历史消息（从 `start`（含）向前 `count` 条；`start=None` 表示最新）。
    pub async fn get_message_history(
        &self,
        peer: &Peer,
        start: Option<&MessageId>,
        count: u32,
    ) -> Result<Vec<MessageEvent>> {
        self.inner.get_message_history(peer, start, count).await
    }
    /// 分页拉取历史消息，返回 `(消息列表, 下一页起始 seq)`。
    /// `start_seq=None` 从最新消息开始；`next` 为 `None` 表示无更多。
    pub async fn get_history_messages_paged(
        &self,
        peer: &Peer,
        start_seq: Option<i64>,
        limit: u32,
    ) -> Result<(Vec<MessageEvent>, Option<i64>)> {
        self.inner
            .get_history_messages_paged(peer, start_seq, limit)
            .await
    }
    /// 退出（`dismiss=true` 时解散）群。
    pub async fn leave_group(&self, group: Uin, dismiss: bool) -> Result<()> {
        self.inner.leave_group(group, dismiss).await
    }
    /// 设置群成员专属头衔。`duration`: 有效期秒数；`-1` 表示永久。
    pub async fn set_group_member_special_title(
        &self,
        group: Uin,
        user: Uin,
        title: &str,
        duration: i64,
    ) -> Result<()> {
        self.inner
            .set_group_member_special_title(group, user, title, duration)
            .await
    }
    /// 标记消息已读。
    pub async fn mark_message_as_read(&self, peer: &Peer, id: &MessageId) -> Result<()> {
        self.inner.mark_message_as_read(peer, id).await
    }
    /// 设置/取消精华消息。
    pub async fn set_essence(&self, group: Uin, id: &MessageId, enable: bool) -> Result<()> {
        self.inner.set_essence(group, id, enable).await
    }
    /// 把资源 id 解析为可下载 URL。
    pub async fn get_resource_url(&self, resource_id: &str) -> Result<String> {
        self.inner.get_resource_url(resource_id).await
    }
    /// 群文件下载直链。
    pub async fn get_group_file_download_url(
        &self,
        group: Uin,
        file_id: &str,
    ) -> Result<String> {
        self.inner.get_group_file_download_url(group, file_id).await
    }
    /// 删除群文件。
    pub async fn delete_group_file(&self, group: Uin, file_id: &str) -> Result<()> {
        self.inner.delete_group_file(group, file_id).await
    }
    /// 拉取待处理的入群/邀请请求。
    pub async fn get_group_requests(&self) -> Result<Vec<Request>> {
        self.inner.get_group_requests().await
    }
    /// 取指定域的 Cookies。
    pub async fn get_cookies(&self, domain: Option<&str>) -> Result<String> {
        self.inner.get_cookies(domain).await
    }
    /// 展开收到的合并转发为节点列表。
    pub async fn get_forward_messages(&self, forward_id: &str) -> Result<Vec<ForwardNode>> {
        self.inner.get_forward_messages(forward_id).await
    }
    /// 删除好友。
    pub async fn delete_friend(&self, user: Uin) -> Result<()> {
        self.inner.delete_friend(user).await
    }
    /// 设置好友备注。
    pub async fn set_friend_remark(&self, user: Uin, remark: &str) -> Result<()> {
        self.inner.set_friend_remark(user, remark).await
    }
    /// 给好友资料卡点赞。
    pub async fn send_profile_like(&self, user: Uin, count: u32) -> Result<()> {
        self.inner.send_profile_like(user, count).await
    }
    /// 拉取待处理的好友请求。
    pub async fn get_friend_requests(&self) -> Result<Vec<Request>> {
        self.inner.get_friend_requests().await
    }
    /// 发布群公告，返回公告 id。
    pub async fn send_group_announcement(
        &self,
        group: Uin,
        content: &str,
        image: Option<ResourceSource>,
    ) -> Result<String> {
        self.inner.send_group_announcement(group, content, image).await
    }
    /// 读取群公告。
    pub async fn get_group_announcements(&self, group: Uin) -> Result<Vec<Announcement>> {
        self.inner.get_group_announcements(group).await
    }
    /// 删除群公告。
    pub async fn delete_group_announcement(&self, group: Uin, announcement_id: &str) -> Result<()> {
        self.inner.delete_group_announcement(group, announcement_id).await
    }
    /// 列出群精华消息。
    pub async fn get_essence_messages(&self, group: Uin) -> Result<Vec<EssenceMessage>> {
        self.inner.get_essence_messages(group).await
    }
    /// 置顶/取消置顶一个会话。
    pub async fn set_peer_pin(&self, peer: &Peer, pinned: bool) -> Result<()> {
        self.inner.set_peer_pin(peer, pinned).await
    }
    /// 列出已置顶会话，返回 (好友实体, 群实体)。
    /// 返回完整 [`FriendInfo`]/[`GroupInfo`]；仅需 id 时取 `.user` / `.group`。
    pub async fn get_peer_pins(&self) -> Result<(Vec<FriendInfo>, Vec<GroupInfo>)> {
        self.inner.get_peer_pins().await
    }
    /// 查询协议端实现信息（Milky `get_impl_info`，含 `milky_version`）。
    /// OneBot 端无对应动作，返回 `Unsupported`。
    pub async fn get_impl_info(&self) -> Result<crate::ImplInfo> {
        self.inner.get_impl_info().await
    }
    /// Milky 专用好友戳一戳，**显式**设定 `is_self`（`true`=戳 bot 自身；`false`=戳 `user`）。
    /// 区别于通用 [`send_nudge`](Self::send_nudge)（其 `is_self` 由 `target==self` 推断）。
    pub async fn send_friend_nudge(&self, user: Uin, is_self: bool) -> Result<()> {
        self.inner.send_friend_nudge(user, is_self).await
    }
    /// 分页拉取群通知中的非请求变体（admin_change/kick/quit），透出为 [`Notice`]。
    /// 返回 `(通知列表, 下一页起始 seq)`；请求类（join/invited_join）请用
    /// [`MilkyActions::get_group_notifications_paged`](crate::adapter::MilkyActions::get_group_notifications_paged)。
    pub async fn get_group_notices_paged(
        &self,
        start_seq: Option<i64>,
        limit: u32,
        is_filtered: bool,
    ) -> Result<(Vec<Notice>, Option<i64>)> {
        self.inner
            .get_group_notices_paged(start_seq, limit, is_filtered)
            .await
    }
    /// 列出群文件与子文件夹。
    pub async fn get_group_files(
        &self,
        group: Uin,
        folder_id: Option<&str>,
    ) -> Result<GroupFileList> {
        self.inner.get_group_files(group, folder_id).await
    }
    /// 私聊文件下载直链。
    pub async fn get_private_file_download_url(
        &self,
        user: Uin,
        file_id: &str,
        hash: Option<&str>,
    ) -> Result<String> {
        self.inner.get_private_file_download_url(user, file_id, hash).await
    }
    /// 新建群文件夹，返回 folder id。
    pub async fn create_group_folder(&self, group: Uin, name: &str) -> Result<String> {
        self.inner.create_group_folder(group, name).await
    }
    /// 重命名群文件夹。
    pub async fn rename_group_folder(
        &self,
        group: Uin,
        folder_id: &str,
        new_name: &str,
    ) -> Result<()> {
        self.inner.rename_group_folder(group, folder_id, new_name).await
    }
    /// 删除群文件夹。
    pub async fn delete_group_folder(&self, group: Uin, folder_id: &str) -> Result<()> {
        self.inner.delete_group_folder(group, folder_id).await
    }
    /// 移动群文件到目标文件夹。
    /// `source_folder_id`：文件当前所在文件夹（`None` 表示根 "/"）。
    pub async fn move_group_file(
        &self,
        group: Uin,
        file_id: &str,
        source_folder_id: Option<&str>,
        target_folder_id: Option<&str>,
    ) -> Result<()> {
        self.inner.move_group_file(group, file_id, source_folder_id, target_folder_id).await
    }
    /// 重命名群文件。
    /// `source_folder_id`：文件当前所在文件夹（`None` 表示根 "/"）。
    pub async fn rename_group_file(
        &self,
        group: Uin,
        file_id: &str,
        source_folder_id: Option<&str>,
        new_name: &str,
    ) -> Result<()> {
        self.inner.rename_group_file(group, file_id, source_folder_id, new_name).await
    }
    /// 设置群头像。
    pub async fn set_group_avatar(&self, group: Uin, src: ResourceSource) -> Result<()> {
        self.inner.set_group_avatar(group, src).await
    }
    /// 设置机器人自身头像。
    pub async fn set_self_avatar(&self, src: ResourceSource) -> Result<()> {
        self.inner.set_self_avatar(src).await
    }
    /// 设置机器人自身昵称。
    pub async fn set_self_nickname(&self, name: &str) -> Result<()> {
        self.inner.set_self_nickname(name).await
    }
    /// 设置机器人自身签名。
    pub async fn set_self_bio(&self, bio: &str) -> Result<()> {
        self.inner.set_self_bio(bio).await
    }
    /// 协议端运行状态。
    pub async fn get_status(&self) -> Result<ImplStatus> {
        self.inner.get_status().await
    }
    /// 取 CSRF/bkn token。
    pub async fn get_csrf_token(&self) -> Result<String> {
        self.inner.get_csrf_token().await
    }
    /// 重启协议端实现。
    pub async fn set_restart(&self, delay_ms: u32) -> Result<()> {
        self.inner.set_restart(delay_ms).await
    }
    /// 清理缓存。
    pub async fn clean_cache(&self) -> Result<()> {
        self.inner.clean_cache().await
    }
    /// 能否发送图片。
    pub async fn can_send_image(&self) -> Result<bool> {
        self.inner.can_send_image().await
    }
    /// 能否发送语音。
    pub async fn can_send_record(&self) -> Result<bool> {
        self.inner.can_send_record().await
    }
    /// 群荣誉信息（typed `HonorList`）。
    pub async fn get_group_honor_info(&self, group: Uin, kind: HonorKind) -> Result<HonorList> {
        self.inner.get_group_honor_info(group, kind).await
    }
    /// 取语音文件本地路径。
    pub async fn get_record(&self, file: &str, out_format: &str) -> Result<String> {
        self.inner.get_record(file, out_format).await
    }
    /// 取图片文件本地路径。
    pub async fn get_image(&self, file: &str) -> Result<String> {
        self.inner.get_image(file).await
    }
    /// 协议私有动作逃生口。
    pub async fn call_raw(&self, action: &str, params: Value) -> Result<Value> {
        self.inner.call_raw(action, params).await
    }

    /// 协议/厂商专属动作的**直达口**:返回内部 `Actions` 的引用，让业务侧
    /// `bot.actions().<method>()` 直接调用全部 [`OneBotActions`](crate::invoker::OneBotActions)
    /// （含 NapCat / LLOneBot / Lagrange 厂商扩展）与 [`MilkyActions`](crate::invoker::MilkyActions)
    /// 动作——这些动作未在 `Bot` 上铺 inherent forwarder，但仍能经此触达。
    /// 不支持当前协议/厂商的动作沿用默认实现返回 `Error::Unsupported`，故切协议无需改代码。
    pub fn actions(&self) -> &dyn Actions {
        &*self.inner
    }

    /// 一个**空操作** `Bot`:不连任何协议端——三个必需动作(协议/发送/原始调用)给出
    /// 平凡实现,其余动作沿用 trait 默认(多为 `Err(unsupported)`)。给测试 / 占位用:
    /// 无需起真协议端即可拿到一个 `Bot` 句柄(`self_id = Uin(0)`)。
    pub fn noop() -> Self {
        Self::new(Arc::new(NoopActions), Uin(0))
    }
}

/// 空操作 [`Actions`]:仅实现三个必需方法,其余沿用 trait 默认实现。见 [`Bot::noop`]。
struct NoopActions;
#[async_trait::async_trait]
impl crate::invoker::ActionInvoker for NoopActions {
    fn protocol(&self) -> Protocol {
        Protocol::Milky
    }
    async fn send(&self, peer: &Peer, _message: &[Segment]) -> Result<MessageId> {
        Ok(MessageId::from_seq(*peer, 0))
    }
    async fn call_raw(&self, _action: &str, _params: Value) -> Result<Value> {
        Ok(Value::Null)
    }
}
impl crate::invoker::OneBotActions for NoopActions {}
impl crate::invoker::MilkyActions for NoopActions {}
