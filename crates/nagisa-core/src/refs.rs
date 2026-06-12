//! 类型化目标句柄：`bot.group(g).member(u).mute(secs)`。
//!
//! 一个句柄既**命名**目标、又是**操作**它的入口。
//! 这些 ref 是 [`Bot`] 上既有扁平方法的薄包装（`Bot` 仍是实现底座），但把按目标分组的、
//! 受 `Capability` 约束的操作聚到一起，发现性远好于一长串 `bot.set_group_member_*`。
use crate::bot::Bot;
use nagisa_types::prelude::*;

/// 群句柄。
#[derive(Clone)]
pub struct GroupRef {
    bot: Bot,
    group: Uin,
}

/// 群成员句柄。
#[derive(Clone)]
pub struct MemberRef {
    bot: Bot,
    group: Uin,
    user: Uin,
}

/// 好友句柄。
#[derive(Clone)]
pub struct FriendRef {
    bot: Bot,
    user: Uin,
}

impl Bot {
    /// 取一个群句柄。
    pub fn group(&self, id: impl Into<Uin>) -> GroupRef {
        GroupRef { bot: self.clone(), group: id.into() }
    }
    /// 取一个好友句柄。
    pub fn friend(&self, id: impl Into<Uin>) -> FriendRef {
        FriendRef { bot: self.clone(), user: id.into() }
    }
}

impl GroupRef {
    /// 该群的对端寻址。
    pub fn peer(&self) -> Peer {
        Peer::group(self.group)
    }
    /// 该群的群号。
    pub fn id(&self) -> Uin {
        self.group
    }
    /// 取该群某成员的句柄。
    pub fn member(&self, user: impl Into<Uin>) -> MemberRef {
        MemberRef { bot: self.bot.clone(), group: self.group, user: user.into() }
    }
    pub async fn send(&self, message: &[Segment]) -> Result<MessageId> {
        self.bot.send(&self.peer(), message).await
    }
    pub async fn info(&self, no_cache: bool) -> Result<GroupInfo> {
        self.bot.get_group_info(self.group, no_cache).await
    }
    pub async fn members(&self) -> Result<Vec<MemberInfo>> {
        self.bot.get_group_member_list(self.group, false).await
    }
    pub async fn set_name(&self, name: &str) -> Result<()> {
        self.bot.set_group_name(self.group, name).await
    }
    pub async fn whole_mute(&self, enable: bool) -> Result<()> {
        self.bot.set_group_whole_mute(self.group, enable).await
    }
    pub async fn leave(&self, dismiss: bool) -> Result<()> {
        self.bot.leave_group(self.group, dismiss).await
    }
    pub async fn set_avatar(&self, src: ResourceSource) -> Result<()> {
        self.bot.set_group_avatar(self.group, src).await
    }
    pub async fn send_announcement(&self, content: &str, image: Option<ResourceSource>) -> Result<String> {
        self.bot.send_group_announcement(self.group, content, image).await
    }
    pub async fn announcements(&self) -> Result<Vec<Announcement>> {
        self.bot.get_group_announcements(self.group).await
    }
    pub async fn essence_messages(&self) -> Result<Vec<EssenceMessage>> {
        self.bot.get_essence_messages(self.group).await
    }
    pub async fn files(&self, folder_id: Option<&str>) -> Result<GroupFileList> {
        self.bot.get_group_files(self.group, folder_id).await
    }
}

impl MemberRef {
    pub fn group(&self) -> Uin {
        self.group
    }
    pub fn uin(&self) -> Uin {
        self.user
    }
    pub async fn info(&self, no_cache: bool) -> Result<MemberInfo> {
        self.bot.get_group_member_info(self.group, self.user, no_cache).await
    }
    /// 禁言 `seconds` 秒。
    pub async fn mute(&self, seconds: u32) -> Result<()> {
        self.bot.set_group_member_mute(self.group, self.user, seconds).await
    }
    /// 解除禁言。
    pub async fn unmute(&self) -> Result<()> {
        self.bot.set_group_member_mute(self.group, self.user, 0).await
    }
    pub async fn kick(&self, reject_add: bool) -> Result<()> {
        self.bot.kick_group_member(self.group, self.user, reject_add).await
    }
    pub async fn set_card(&self, card: &str) -> Result<()> {
        self.bot.set_group_member_card(self.group, self.user, card).await
    }
    pub async fn set_admin(&self, enable: bool) -> Result<()> {
        self.bot.set_group_admin(self.group, self.user, enable).await
    }
    pub async fn set_title(&self, title: &str) -> Result<()> {
        self.bot.set_group_member_special_title(self.group, self.user, title, -1).await
    }
    /// 戳一戳该成员。
    pub async fn poke(&self) -> Result<()> {
        self.bot.send_nudge(&Peer::group(self.group), self.user).await
    }
}

impl FriendRef {
    pub fn peer(&self) -> Peer {
        Peer::friend(self.user)
    }
    pub fn uin(&self) -> Uin {
        self.user
    }
    pub async fn send(&self, message: &[Segment]) -> Result<MessageId> {
        self.bot.send(&self.peer(), message).await
    }
    /// 该用户档案（陌生人也可）。
    pub async fn profile(&self, no_cache: bool) -> Result<UserInfo> {
        self.bot.get_user_info(self.user, no_cache).await
    }
    pub async fn info(&self) -> Result<FriendInfo> {
        self.bot.get_friend_info(self.user).await
    }
    pub async fn delete(&self) -> Result<()> {
        self.bot.delete_friend(self.user).await
    }
    pub async fn set_remark(&self, remark: &str) -> Result<()> {
        self.bot.set_friend_remark(self.user, remark).await
    }
    /// 资料卡点赞 `count` 次。
    pub async fn like(&self, count: u32) -> Result<()> {
        self.bot.send_profile_like(self.user, count).await
    }
    pub async fn poke(&self) -> Result<()> {
        self.bot.send_nudge(&self.peer(), self.user).await
    }
}
