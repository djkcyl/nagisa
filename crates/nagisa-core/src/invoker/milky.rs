//! Milky 协议**独有**动作(OneBot 无对应)——12 个方法,Milky IR 有而 OneBot 无对应者。
//!
//! 在 OneBot adapter 上默认返回 `Unsupported`;Milky adapter override 真实实现。
//! 见 [`ActionInvoker`](super::ActionInvoker)(两协议通用)与 [`Actions`](super::Actions)(组合)。
use super::unsupported;
use async_trait::async_trait;
use nagisa_types::prelude::*;

/// Milky 独有动作。Milky 1.2 IR 中有、但 OneBot 无对应者。
#[async_trait]
pub trait MilkyActions: Send + Sync + 'static {
    /// 查询协议端实现信息（`impl_name`/`impl_version`/`milky_version`/qq_protocol_*）。
    ///
    /// 这是一个**显式 typed 动作**：连接时的 best-effort 能力探测（缓存到
    /// `adapter.impl_info()`）之外，业务侧也能主动按需重新拉取。OneBot 端无 `get_impl_info`
    /// 对应动作，默认 `Unsupported`。
    async fn get_impl_info(&self) -> Result<crate::ImplInfo> {
        Err(unsupported("get_impl_info"))
    }
    /// 单个好友信息。
    /// (OneBot 无单好友查询动作。)
    async fn get_friend_info(&self, _user: Uin) -> Result<FriendInfo> {
        Err(unsupported("get_friend_info"))
    }
    /// Milky 专用好友戳一戳，**显式**设定 `is_self`。
    ///
    /// 区别于通用 [`ActionInvoker::send_nudge`](super::ActionInvoker::send_nudge)（后者把
    /// `is_self` 推断为 `target == self`，因而无法对**他人**代戳，也无法在 `target != self`
    /// 时单独触发「戳 bot 自己」）。`is_self=true` → 戳机器人自身；`is_self=false` → 戳 `user`。
    async fn send_friend_nudge(&self, _user: Uin, _is_self: bool) -> Result<()> {
        Err(unsupported("send_friend_nudge"))
    }
    /// 拉取待处理的好友请求。
    async fn get_friend_requests(&self) -> Result<Vec<Request>> {
        Err(unsupported("get_friend_requests"))
    }
    /// 把资源 id(图片/语音/视频/文件)解析为可下载 URL。
    async fn get_resource_url(&self, _resource_id: &str) -> Result<String> {
        Err(unsupported("get_resource_url"))
    }
    /// 置顶/取消置顶一个会话(`Capability::PeerPin`)。
    async fn set_peer_pin(&self, _peer: &Peer, _pinned: bool) -> Result<()> {
        Err(unsupported("set_peer_pin"))
    }
    /// 列出已置顶会话,返回 (好友实体, 群实体)。
    ///
    /// 返回**完整** [`FriendInfo`]/[`GroupInfo`]（昵称/备注/成员数等），而非裸 id 列表——
    /// wire 给的就是 `FriendEntity[]`/`GroupEntity[]`，塌缩成 id 会丢字段。仅需 id 时取
    /// `.user` / `.group` 即可。
    async fn get_peer_pins(&self) -> Result<(Vec<FriendInfo>, Vec<GroupInfo>)> {
        Err(unsupported("get_peer_pins"))
    }
    /// 拉取账号收藏的自定义表情 URL 列表。
    async fn get_custom_face_url_list(&self) -> Result<Vec<String>> {
        Err(unsupported("get_custom_face_url_list"))
    }
    /// 分页拉取好友请求。`limit`/`is_filtered` 均为必填(无默认);非分页的
    /// [`get_friend_requests`](Self::get_friend_requests) 才用 Milky 约定默认
    /// limit=20、is_filtered=false 调用本方法。`is_filtered=true` 取被过滤(风险)请求。
    async fn get_friend_requests_paged(&self, _limit: u32, _is_filtered: bool) -> Result<Vec<Request>> {
        Err(unsupported("get_friend_requests"))
    }
    /// 分页拉取群通知，返回 (请求列表, 下一页起始 seq)。
    async fn get_group_notifications_paged(
        &self,
        _start_seq: Option<i64>,
        _limit: u32,
        _is_filtered: bool,
    ) -> Result<(Vec<Request>, Option<i64>)> {
        Err(unsupported("get_group_notifications"))
    }
    /// 分页拉取群通知中的**非请求**变体（`admin_change`/`kick`/`quit`），透出为 [`Notice`]。
    ///
    /// `get_group_notifications` wire 同时返回 join/invited_join（请求，经
    /// [`get_group_notifications_paged`](Self::get_group_notifications_paged) 映射为
    /// `Request`）与 admin_change/kick/quit（**已发生的事实**，非待处理请求）。后者既不该
    /// 当 `Request` 也不该被静默丢弃，故在此映射为 `Notice::AdminChange` /
    /// `Notice::MemberDecrease`，与事件流的同名 Notice 对齐。返回 `(notices, 下一页起始 seq)`。
    async fn get_group_notices_paged(
        &self,
        _start_seq: Option<i64>,
        _limit: u32,
        _is_filtered: bool,
    ) -> Result<(Vec<Notice>, Option<i64>)> {
        Err(unsupported("get_group_notifications"))
    }
    /// 分页拉取历史消息，返回 (消息列表, 下一页起始 seq)。
    async fn get_history_messages_paged(
        &self,
        _peer: &Peer,
        _start_seq: Option<i64>,
        _limit: u32,
    ) -> Result<(Vec<MessageEvent>, Option<i64>)> {
        Err(unsupported("get_history_messages"))
    }
}
