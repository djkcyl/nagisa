//! 事件触发器：`EventKind` 判别式 + 细粒度 `FromContext` 提取器，使
//! `#[event(Kind)]` handler 能在事件上触发并读到类型化数据，同时启用开关门控统一套用
//! （经 `Event::peer()`）。
use crate::ctx::Ctx;
use crate::extract::{Extracted, FromContext, Reject};
use async_trait::async_trait;
use nagisa_types::event::{Event, Meta, Notice, Request};
use nagisa_types::id::{Peer, Uin};

/// 给 `#[event(Kind)]` 用的友好选择器。覆盖 nagisa 的事件面；若干 `Notice`/`Meta`
/// 变体被合并（如两种 nudge 变体都 → `Nudge`）。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum EventKind {
    Message,
    MemberJoin, MemberLeave, Mute, WholeMute, AdminChange, GroupNameChange,
    Honor, GroupCardChange, Recall, Reaction, EssenceChange, LuckyKing,
    Nudge, FriendAdd, GroupFileUpload, FriendFileUpload, PeerPin,
    GroupDismiss, GroupTitleChange, InputStatus, ProfileLike, GrayTip,
    PokeRecall, OnlineFile, FlashFile,
    FriendRequest, GroupJoinRequest, GroupInvitedJoin, GroupInvite,
    Connect, Disconnect, Ready, Heartbeat, BotOnline, BotOffline,
    Raw,
}

impl EventKind {
    /// 该具体事件匹配的 `EventKind`（如有；`Notice::Other` 及未知的未来变体 → `None`）。
    pub fn of(event: &Event) -> Option<EventKind> {
        use EventKind as K;
        Some(match event {
            Event::Message(_) => K::Message,
            Event::Notice(n) => match n {
                Notice::Recall { .. } => K::Recall,
                Notice::MemberIncrease { .. } => K::MemberJoin,
                Notice::MemberDecrease { .. } => K::MemberLeave,
                Notice::AdminChange { .. } => K::AdminChange,
                Notice::Mute { .. } => K::Mute,
                Notice::WholeMute { .. } => K::WholeMute,
                Notice::GroupNameChange { .. } => K::GroupNameChange,
                Notice::Honor { .. } => K::Honor,
                Notice::GroupCardChange { .. } => K::GroupCardChange,
                Notice::FriendNudge { .. } | Notice::GroupNudge { .. } => K::Nudge,
                Notice::Reaction { .. } => K::Reaction,
                Notice::EssenceChange { .. } => K::EssenceChange,
                Notice::GroupFileUpload { .. } => K::GroupFileUpload,
                Notice::FriendFileUpload { .. } => K::FriendFileUpload,
                Notice::FriendAdd { .. } => K::FriendAdd,
                Notice::PeerPinChange { .. } => K::PeerPin,
                Notice::BotOffline { .. } => K::BotOffline,
                Notice::LuckyKing { .. } => K::LuckyKing,
                Notice::GroupDismiss { .. } => K::GroupDismiss,
                Notice::GroupTitleChange { .. } => K::GroupTitleChange,
                Notice::InputStatus { .. } => K::InputStatus,
                Notice::ProfileLike { .. } => K::ProfileLike,
                Notice::GrayTip { .. } => K::GrayTip,
                Notice::PokeRecall { .. } => K::PokeRecall,
                Notice::OnlineFile { .. } => K::OnlineFile,
                Notice::FlashFile { .. } => K::FlashFile,
                Notice::Other { .. } => return None,
                _ => return None,
            },
            Event::Request(r) => match r {
                Request::Friend { .. } => K::FriendRequest,
                Request::GroupJoin { .. } => K::GroupJoinRequest,
                Request::GroupInvitedJoin { .. } => K::GroupInvitedJoin,
                Request::GroupInvite { .. } => K::GroupInvite,
                _ => return None,
            },
            Event::Meta(m) => match m {
                Meta::Connect => K::Connect,
                Meta::Disconnect { .. } => K::Disconnect,
                Meta::Ready { .. } => K::Ready,
                Meta::Heartbeat { .. } => K::Heartbeat,
                Meta::BotOnline { .. } => K::BotOnline,
                Meta::BotOffline => K::BotOffline,
                _ => return None,
            },
            Event::Raw(_) => K::Raw,
            _ => K::Raw,
        })
    }
}

// ── 细粒度事件提取器 ────────────────────────────────────────────
//
// 每个结构体只匹配某个 EventKind 的底层变体,否则返回 `Reject::Skip`。Nudge 额外在行为主体
// 是 bot 自己时跳过(自事件过滤——防止 bot 对自己的戳一戳作出反应)。

/// 有成员入群（`Notice::MemberIncrease`）。
pub struct MemberJoin { pub group: Uin, pub user: Uin, pub operator: Option<Uin>, pub invitor: Option<Uin> }
#[async_trait]
impl FromContext for MemberJoin {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        match ctx.event().as_ref() {
            Event::Notice(Notice::MemberIncrease { group, user, operator, invitor }) =>
                Ok(MemberJoin { group: *group, user: *user, operator: *operator, invitor: *invitor }),
            _ => Err(Reject::Skip),
        }
    }
}

/// 成员退群或被踢（`Notice::MemberDecrease`）。
pub struct MemberLeave {
    pub group: Uin,
    pub user: Uin,
    pub operator: Option<Uin>,
    pub reason: nagisa_types::event::MemberDecreaseReason,
}
#[async_trait]
impl FromContext for MemberLeave {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        match ctx.event().as_ref() {
            Event::Notice(Notice::MemberDecrease { group, user, operator, reason }) =>
                Ok(MemberLeave { group: *group, user: *user, operator: *operator, reason: *reason }),
            _ => Err(Reject::Skip),
        }
    }
}

/// 群里有成员被禁言（`Notice::Mute`）。
pub struct Mute { pub group: Uin, pub user: Uin, pub operator: Uin, pub duration: i32 }
#[async_trait]
impl FromContext for Mute {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        match ctx.event().as_ref() {
            Event::Notice(Notice::Mute { group, user, operator, duration }) =>
                Ok(Mute { group: *group, user: *user, operator: *operator, duration: *duration }),
            _ => Err(Reject::Skip),
        }
    }
}

/// 一条消息被撤回（`Notice::Recall`）。同时携带原始 `sender`（作者）与撤回它的 `operator`，
/// 使 handler 能套自己的策略（如防撤回类插件可在 `author != bot OR operator == bot` 时触发）。
/// 刻意**不**做自过滤——在这里写死一套自策略会挡掉防撤回功能需要的「bot 自己的消息被管理员
/// 撤回」这一情形。
pub struct Recall {
    pub peer: Peer,
    pub id: nagisa_types::id::MessageId,
    pub sender: Uin,
    pub operator: Uin,
    pub suffix: Option<String>,
}
#[async_trait]
impl FromContext for Recall {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        match ctx.event().as_ref() {
            Event::Notice(Notice::Recall { peer, id, sender, operator, suffix }) =>
                Ok(Recall { peer: *peer, id: id.clone(), sender: *sender, operator: *operator, suffix: suffix.clone() }),
            _ => Err(Reject::Skip),
        }
    }
}

/// 戳一戳事件——统一 `Notice::FriendNudge` 与 `Notice::GroupNudge`，归一为
/// `{ peer, sender, receiver }`。自过滤：`sender == self_id` 时跳过（防止 bot 对自己的戳一戳反应）。
pub struct Nudge { pub peer: Peer, pub sender: Uin, pub receiver: Uin }
#[async_trait]
impl FromContext for Nudge {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        let self_id = ctx.bot().self_id();
        match ctx.event().as_ref() {
            Event::Notice(Notice::FriendNudge { user, is_self_send, .. }) => {
                if *is_self_send {
                    return Err(Reject::Skip);
                }
                // user 是发戳一戳的好友;bot 是被戳者。
                Ok(Nudge { peer: Peer::friend(*user), sender: *user, receiver: self_id })
            }
            Event::Notice(Notice::GroupNudge { group, sender, receiver, .. }) => {
                if *sender == self_id {
                    return Err(Reject::Skip);
                }
                Ok(Nudge { peer: Peer::group(*group), sender: *sender, receiver: *receiver })
            }
            _ => Err(Reject::Skip),
        }
    }
}

/// 新加了一个好友（`Notice::FriendAdd`）。
pub struct FriendAdd { pub user: Uin }
#[async_trait]
impl FromContext for FriendAdd {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        match ctx.event().as_ref() {
            Event::Notice(Notice::FriendAdd { user }) => Ok(FriendAdd { user: *user }),
            _ => Err(Reject::Skip),
        }
    }
}

/// 好友请求（`Request::Friend`）。
pub struct FriendRequest { pub initiator: Uin, pub comment: String, pub token: nagisa_types::event::RequestToken }
#[async_trait]
impl FromContext for FriendRequest {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        match ctx.event().as_ref() {
            Event::Request(Request::Friend { initiator, comment, token, .. }) =>
                Ok(FriendRequest { initiator: *initiator, comment: comment.clone(), token: token.clone() }),
            _ => Err(Reject::Skip),
        }
    }
}

/// 入群请求（`Request::GroupJoin`）。
pub struct GroupJoinRequest { pub group: Uin, pub initiator: Uin, pub comment: String, pub token: nagisa_types::event::RequestToken }
#[async_trait]
impl FromContext for GroupJoinRequest {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        match ctx.event().as_ref() {
            Event::Request(Request::GroupJoin { group, initiator, comment, token, .. }) =>
                Ok(GroupJoinRequest { group: *group, initiator: *initiator, comment: comment.clone(), token: token.clone() }),
            _ => Err(Reject::Skip),
        }
    }
}

/// 群管理员被设置/取消（`Notice::AdminChange`）。`is_set` = 升为管理员。
/// （`user == self_id` 时即 bot 自身权限变更，可据此监听 bot 被设/免管理员。）
pub struct AdminChange { pub group: Uin, pub user: Uin, pub operator: Option<Uin>, pub is_set: bool }
#[async_trait]
impl FromContext for AdminChange {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        match ctx.event().as_ref() {
            Event::Notice(Notice::AdminChange { group, user, operator, is_set }) =>
                Ok(AdminChange { group: *group, user: *user, operator: *operator, is_set: *is_set }),
            _ => Err(Reject::Skip),
        }
    }
}

/// 某成员的群名片（昵称）变更（`Notice::GroupCardChange`）。
/// （可在此对新名片做文本审核等处理。）
pub struct GroupCardChange { pub group: Uin, pub user: Uin, pub old_card: String, pub new_card: String }
#[async_trait]
impl FromContext for GroupCardChange {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        match ctx.event().as_ref() {
            Event::Notice(Notice::GroupCardChange { group, user, old_card, new_card }) =>
                Ok(GroupCardChange { group: *group, user: *user, old_card: old_card.clone(), new_card: new_card.clone() }),
            _ => Err(Reject::Skip),
        }
    }
}

/// 群荣誉变更（龙王/群聊之火/快乐源泉）（`Notice::Honor`）。
pub struct Honor { pub group: Uin, pub user: Uin, pub honor: nagisa_types::event::HonorKind }
#[async_trait]
impl FromContext for Honor {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        match ctx.event().as_ref() {
            Event::Notice(Notice::Honor { group, user, honor }) =>
                Ok(Honor { group: *group, user: *user, honor: *honor }),
            _ => Err(Reject::Skip),
        }
    }
}

/// 群红包运气王结果（`Notice::LuckyKing`）。`user` = 发红包者，`target` = 运气王。
pub struct LuckyKing { pub group: Uin, pub user: Uin, pub target: Uin }
#[async_trait]
impl FromContext for LuckyKing {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        match ctx.event().as_ref() {
            Event::Notice(Notice::LuckyKing { group, user, target }) =>
                Ok(LuckyKing { group: *group, user: *user, target: *target }),
            _ => Err(Reject::Skip),
        }
    }
}

// ── 生命周期事件提取器 ───────────────────────────────────────────────
//
// 每个生命周期 `EventKind` 一个类型化提取器,使 `#[event(Ready)]`/`#[event(Disconnect)]`/… 的
// handler 能直接命名其载荷(如 `async fn(r: Ready, bot: Bot)`),而非匹配裸 `Meta`。其余事件一律
// `Reject::Skip`。不需要载荷的 handler 直接省掉该提取器、只取 `Bot`(或什么都不取)即可。

/// 协议端（传输层）连接成功（[`Meta::Connect`]，框架事件源每次（重）连发出）。
pub struct Connect;
#[async_trait]
impl FromContext for Connect {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        match ctx.event().as_ref() {
            Event::Meta(Meta::Connect) => Ok(Connect),
            _ => Err(Reject::Skip),
        }
    }
}

/// 协议端（传输层）断开（[`Meta::Disconnect`]）。`reason` 为底层错误文案（如有）。
pub struct Disconnect { pub reason: Option<String> }
#[async_trait]
impl FromContext for Disconnect {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        match ctx.event().as_ref() {
            Event::Meta(Meta::Disconnect { reason }) => Ok(Disconnect { reason: reason.clone() }),
            _ => Err(Reject::Skip),
        }
    }
}

/// 框架就绪：已解析出可用账号（[`Meta::Ready`]，每次 `run_*` 仅一次）。`self_id` 为机器人账号，
/// `nickname` 为其昵称（来自 `get_login_info`，未知时为空串）。
pub struct Ready { pub self_id: Uin, pub nickname: String }
#[async_trait]
impl FromContext for Ready {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        match ctx.event().as_ref() {
            Event::Meta(Meta::Ready { self_id, nickname }) => {
                Ok(Ready { self_id: *self_id, nickname: nickname.clone() })
            }
            _ => Err(Reject::Skip),
        }
    }
}

/// 机器人账号上线（[`Meta::BotOnline`]）。`reason` 为上线原因文案（Lagrange 提供，OneBot 为 None）。
pub struct BotOnline { pub reason: Option<String> }
#[async_trait]
impl FromContext for BotOnline {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        match ctx.event().as_ref() {
            Event::Meta(Meta::BotOnline { reason }) => Ok(BotOnline { reason: reason.clone() }),
            _ => Err(Reject::Skip),
        }
    }
}

/// 机器人账号掉线（[`Meta::BotOffline`]）。注意与传输层 [`Disconnect`] 区分。
pub struct BotOffline;
#[async_trait]
impl FromContext for BotOffline {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        match ctx.event().as_ref() {
            Event::Meta(Meta::BotOffline) => Ok(BotOffline),
            _ => Err(Reject::Skip),
        }
    }
}

/// 心跳（[`Meta::Heartbeat`]）。`interval` 为心跳间隔（ms），`online`/`good` 取自 status。
pub struct Heartbeat { pub interval: i64, pub online: bool, pub good: bool }
#[async_trait]
impl FromContext for Heartbeat {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        match ctx.event().as_ref() {
            Event::Meta(Meta::Heartbeat { interval, status }) =>
                Ok(Heartbeat { interval: *interval, online: status.online, good: status.good }),
            _ => Err(Reject::Skip),
        }
    }
}
