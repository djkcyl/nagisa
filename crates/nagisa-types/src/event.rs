//! 统一事件 [`Event`] 及其各类载荷：[`MessageEvent`]（消息）、[`Notice`]（通知）、
//! [`Request`]（请求，配 [`RequestToken`] 回传同意/拒绝）、[`Meta`]（连接/就绪/心跳等元事件），
//! 外加逃生口 [`Event::Raw`]。事件按细粒度变体建模（OneBot 的重载哨兵在适配器侧拆成具体变体），
//! 各变体字段的跨协议来源/缺口写在对应 `///` 上。[`Event`] 上的 [`peer`](Event::peer) /
//! [`group`](Event::group) / [`sender`](Event::sender) 是跨变体抽取寻址信息的便捷方法。
use crate::capability::Protocol;
use crate::entity::{FileMeta, FriendInfo, GroupInfo, ImplStatus, MemberInfo};
use crate::id::{MessageId, Peer, Scene, Uin};
use crate::message::Message;
use serde_json::Value;

#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Event {
    /// `MessageEvent` 较大且是热路径；装箱以避免 `Event` 整体膨胀
    /// （`Event` 通常以 `Arc<Event>` 传递，模式匹配会自动穿透 `Box`）。
    Message(Box<MessageEvent>),
    Notice(Notice),
    Request(Request),
    Meta(Meta),
    /// 逃生口：未知/协议私有事件，绝不丢弃。
    Raw(RawEvent),
}

/// 群匿名消息发送者标识（OneBot 群消息 `sub_type=anonymous` 的 `anonymous` 子对象）。
/// `flag` 用于 `set_group_anonymous_ban`。OneBot-only；Milky 为 None。
/// OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/event/message.md (§群消息 anonymous)
#[derive(Clone, Debug)]
pub struct Anonymous {
    pub id: i64,
    pub name: String,
    pub flag: String,
}

/// 消息气泡样式（Lagrange.OneBot 群/私聊消息事件的 `message_style` 块）。
/// 仅在 Lagrange 端出现；其余协议为 None。`bubble_id`/`pendant_id` 为气泡/挂件
/// 资源 id，`pal_type` 为伙伴类型；`raw` 保留完整原始块以防协议追加字段（绝不丢弃）。
/// ENDPOINT: Lagrange.OneBot Lagrange.OneBot/Message/MessageStyle（message_style 块）
///   (https://github.com/LagrangeDev/Lagrange.Core)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageStyle {
    pub bubble_id: Option<i64>,
    pub pendant_id: Option<i64>,
    pub pal_type: Option<i64>,
    pub raw: Value,
}

#[derive(Clone, Debug)]
pub struct MessageEvent {
    pub id: MessageId,
    pub peer: Peer,
    pub sender: Uin,
    pub self_id: Uin,
    pub time: i64,
    pub content: Message,
    pub is_self: bool,
    pub group: Option<GroupInfo>,
    pub member: Option<MemberInfo>,
    /// 好友场景消息附带的好友实体（Milky friend-scene 给实体；OneBot 为 None）。
    pub friend: Option<FriendInfo>,
    /// 群匿名消息的匿名发送者标识（OneBot-only；非匿名/Milky 为 None）。
    pub anonymous: Option<Anonymous>,
    /// 消息字体（OneBot `font`，遗留字段；Milky 为 None）。
    pub font: Option<i32>,
    /// 私聊临时会话的来源对端（OneBot/LLOneBot 私聊消息的 `target_id`：本号给
    /// 对端发私聊时对端 uin）；群消息/Milky 为 None。
    /// ENDPOINT: LLOneBot/NapCat 私聊消息事件 `target_id` 字段。
    pub target_id: Option<Uin>,
    /// 消息气泡样式（Lagrange `message_style` 块）；其余协议为 None。
    pub message_style: Option<MessageStyle>,
    pub raw: Value,
}

#[derive(Clone, Debug)]
pub struct NudgeDisplay {
    pub action: String,
    pub suffix: String,
    pub action_img_url: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReactionKind {
    Face,
    Emoji,
}

/// 群荣誉类型。
/// OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/event/notice.md (§群成员荣誉变更)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HonorKind {
    /// 龙王。
    Talkative,
    /// 群聊之火。
    Performer,
    /// 快乐源泉。
    Emotion,
    /// 协议未知值——降级于此,绝不 panic。
    Unknown,
}

/// 群成员减少的原因(OneBot `group_decrease` 的 sub_type)。
/// OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/event/notice.md (§群成员减少)
///
/// **跨协议非对称（固有限制）**：[`KickMe`](Self::KickMe) 仅 **OneBot** wire 能产出
/// （`group_decrease` sub_type=`kick_me`）。Milky 的 `GroupMemberDecreaseNotification`
/// 不区分「被踢的是不是 bot 自己」，只能解码为 [`Leave`](Self::Leave)/[`Kick`](Self::Kick)
/// ——故下游「bot 自身被踢」的 gating 逻辑**不得依赖 Milky 后端下的 `KickMe`**
/// （Milky 永不产出它）。这是 Milky wire 的天然缺口，非解码缺陷。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MemberDecreaseReason {
    /// 主动退群。
    Leave,
    /// 被管理员踢出。
    Kick,
    /// 机器人自身被踢出。**仅 OneBot 可产出**（Milky wire 无此区分，详见枚举级文档）。
    KickMe,
    /// 群解散（NapCat `group_decrease` sub_type=`disband`，群主解散群时对每个成员产出）。
    /// ENDPOINT: NapCat packages/napcat-onebot/event/notice/OB11GroupDecreaseEvent.ts
    ///   (https://github.com/NapNeko/NapCatQQ)。
    Disband,
    /// 协议未提供/未知。
    Unknown,
}

/// 单条表情回应（`group_msg_emoji_like` 的 `likes[]` 元素 / Milky/Lagrange 单 reaction）。
/// `count` 为该 emoji 的累计回应人数（缺省 None）。
/// ENDPOINT: NapCat packages/napcat-onebot/event/notice/OB11MsgEmojiLikeEvent.ts
///   (https://github.com/NapNeko/NapCatQQ)；cross-checked LLOneBot
///   src/onebot11/event/notice/OB11MsgEmojiLikeEvent.ts。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmojiLike {
    pub face_id: String,
    pub count: Option<i64>,
}

/// 在线（临时）文件 notice 的方向（NapCat `online_file_send`/`online_file_receive`）。
/// ENDPOINT: NapCat（在线文件收发 notice）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OnlineFileDirection {
    /// 我方/对端发出在线文件（`online_file_send`）。
    Send,
    /// 收到在线文件（`online_file_receive`）。
    Receive,
}

/// 闪传（flash transfer）notice 的进度阶段（LLOneBot `flash_file` 的 sub_type）。
/// ENDPOINT: LLOneBot src/onebot11/event/notice/OB11FlashTransferNoticeEvent.ts
///   (https://github.com/LLOneBot/LLOneBot)。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FlashFilePhase {
    Downloading,
    Downloaded,
    Uploading,
    Uploaded,
    /// 协议未知阶段——降级于此，绝不 panic。
    Unknown,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Notice {
    Recall { peer: Peer, id: MessageId, sender: Uin, operator: Uin, suffix: Option<String> },
    MemberIncrease { group: Uin, user: Uin, operator: Option<Uin>, invitor: Option<Uin> },
    MemberDecrease { group: Uin, user: Uin, operator: Option<Uin>, reason: MemberDecreaseReason },
    AdminChange { group: Uin, user: Uin, operator: Option<Uin>, is_set: bool },
    Mute { group: Uin, user: Uin, operator: Uin, duration: i32 },
    WholeMute { group: Uin, operator: Uin, is_mute: bool },
    GroupNameChange { group: Uin, new_name: String, operator: Uin },
    /// 群红包运气王（OneBot `notify/lucky_king`）。`user` = 发红包者，`target` = 运气王。
    /// OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/event/notice.md (§群红包运气王)
    LuckyKing { group: Uin, user: Uin, target: Uin },
    /// 群成员荣誉变更（龙王/群聊之火/快乐源泉）。
    /// OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/event/notice.md (§群成员荣誉变更)
    Honor { group: Uin, user: Uin, honor: HonorKind },
    /// 群名片变更。
    /// ENDPOINT: NapCat https://github.com/NapNeko/NapCatQQ/blob/main/src/onebot/event/notice/OB11GroupCardEvent.ts
    GroupCardChange { group: Uin, user: Uin, old_card: String, new_card: String },
    FriendNudge { user: Uin, is_self_send: bool, is_self_receive: bool, display: NudgeDisplay },
    GroupNudge { group: Uin, sender: Uin, receiver: Uin, display: NudgeDisplay },
    /// 表情回应。`face_id`/`count` 反映首个 emoji（向后兼容单事件语义）；
    /// `likes` 携带本次 notice 的**全部** emoji（NapCat/LLOneBot `group_msg_emoji_like`
    /// 可一次回应多个 emoji），其余协议为单元素。绝不丢弃非首 like。
    /// `sub_type` 为原始 wire 子类型（Lagrange `reaction` 的 `sub_type`：`add`/`remove`），
    /// `is_add` 由其派生；非 Lagrange 来源（Milky/emoji_like 等无独立 sub_type）为 None。
    Reaction { group: Uin, user: Uin, seq: i64, face_id: String, kind: ReactionKind, is_add: bool, sub_type: Option<String>, count: Option<i64>, likes: Vec<EmojiLike> },
    /// 群精华消息变更。`sender` 为被设/取消精华消息的原作者（Lagrange `essence`
    /// 的 `sender_id`）；OneBot v11/Milky 无该字段时为 None。
    EssenceChange { group: Uin, seq: i64, sender: Option<Uin>, operator: Option<Uin>, is_set: bool },
    GroupFileUpload { group: Uin, user: Uin, file: FileMeta },
    FriendFileUpload { user: Uin, file: FileMeta, is_self: bool },
    /// 新增好友成功（OneBot `friend_add`）。常用于自动打招呼。
    FriendAdd { user: Uin },
    PeerPinChange { peer: Peer, is_pinned: bool },
    /// 机器人离线（Lagrange `bot_offline`）。`reason` 为离线原因文案，`tag` 为
    /// Lagrange 区分离线种类的标签（与 `reason` 不同维度，如踢下线/网络等）；
    /// 无 tag 的来源为 None。
    BotOffline { reason: String, tag: Option<String> },
    /// 群解散（LLOneBot `group_dismiss`：群主解散整个群时下发一次，区别于逐成员的
    /// `group_decrease`/`disband`）。`operator` 为解散者（通常为群主）。
    /// ENDPOINT: LLOneBot src/onebot11/event/notice/（群解散 notice）
    ///   (https://github.com/LLOneBot/LLOneBot)。
    GroupDismiss { group: Uin, operator: Uin },
    /// 群头衔变更（NapCat/LLOneBot `notify` sub_type=`title`）。`title` 为新头衔。
    /// ENDPOINT: NapCat packages/napcat-onebot/event/notice/OB11GroupTitleEvent.ts
    ///   (https://github.com/NapNeko/NapCatQQ)；cross-checked LLOneBot。
    GroupTitleChange { group: Uin, user: Uin, title: String },
    /// 输入状态（对端正在输入，NapCat/LLOneBot `notify` sub_type=`input_status`）。
    /// 群场景 `group` 为 Some；私聊为 None。`status_text` 为展示文案，`event_type` 为
    /// 原始状态码（0=输入中等，端相关）。
    /// ENDPOINT: NapCat packages/napcat-onebot/event/notice/OB11InputStatusEvent.ts
    ///   (https://github.com/NapNeko/NapCatQQ)；cross-checked LLOneBot。
    InputStatus { user: Uin, group: Option<Uin>, status_text: String, event_type: i64 },
    /// 资料卡点赞通知（NapCat/LLOneBot `notify` sub_type=`profile_like`）。
    /// `operator` 为点赞者，`times` 为本次点赞次数，`operator_nick` 为点赞者昵称（可空）。
    /// ENDPOINT: NapCat packages/napcat-onebot/event/notice/OB11ProfileLikeEvent.ts
    ///   (https://github.com/NapNeko/NapCatQQ)；cross-checked LLOneBot。
    ProfileLike { operator: Uin, operator_nick: String, times: i64 },
    /// 灰字提示（NapCat/LLOneBot `notify` sub_type=`gray_tip`）。承载群/好友灰条系统提示
    /// 文本；具体子类繁多，统一以 `content` 透出文本，结构细节保留在事件 `raw`。
    /// ENDPOINT: NapCat packages/napcat-onebot/event/notice/（gray tip notice）
    ///   (https://github.com/NapNeko/NapCatQQ)；cross-checked LLOneBot。
    GrayTip { group: Option<Uin>, user: Option<Uin>, content: String },
    /// 戳一戳撤回（LLOneBot `notify` sub_type=`poke_recall`）。对端撤回了一次戳一戳。
    /// `group` 群场景为 Some，私聊为 None。
    /// ENDPOINT: LLOneBot src/onebot11/event/notice/（poke_recall notice）
    ///   (https://github.com/LLOneBot/LLOneBot)。
    PokeRecall { group: Option<Uin>, user: Uin },
    /// 在线（临时）文件收发 notice（NapCat `online_file_send`/`online_file_receive`）。
    /// `direction` 区分发出/接收；群场景 `group` 为 Some，私聊为 None。
    /// ENDPOINT: NapCat（在线文件 notice）。
    OnlineFile { direction: OnlineFileDirection, user: Uin, group: Option<Uin> },
    /// 闪传进度 notice（LLOneBot `flash_file`：downloading/downloaded/uploading/uploaded）。
    /// `phase` 为进度阶段；`group` 群场景为 Some。完整闪传载荷保留在事件 `raw`。
    /// ENDPOINT: LLOneBot src/onebot11/event/notice/OB11FlashTransferNoticeEvent.ts
    ///   (https://github.com/LLOneBot/LLOneBot)。
    FlashFile { phase: FlashFilePhase, user: Uin, group: Option<Uin> },
    /// 单侧/未知 notice：OneBot 的 group_card/honor 等也归此。
    Other { protocol: Protocol, kind: String, raw: Value },
}

/// 不透明请求令牌——同意/拒绝时 round-trip；内部按协议打包不同字段。
/// 业务侧视为不透明（只在事件里收到、回传给 `Bot::handle_request`）；
/// 内部 `RequestTokenInner` 标 `#[doc(hidden)]`，仅供各 adapter crate 构造/解构。
#[derive(Clone, Debug)]
pub struct RequestToken(#[doc(hidden)] pub RequestTokenInner);

#[doc(hidden)]
#[derive(Clone, Debug)]
pub enum RequestTokenInner {
    /// OneBot：不透明 flag 字符串。
    OneBotFlag(String),
    /// Milky 好友请求：按 initiator_uid + is_filtered。
    MilkyFriend { initiator_uid: String, is_filtered: bool },
    /// Milky 入群/邀请他人入群：notification_seq + 类型 + group_id + is_filtered。
    MilkyGroupNotification { notification_seq: i64, notification_type: String, group_id: Uin, is_filtered: bool },
    /// Milky 邀请自身入群：group_id + invitation_seq。
    MilkyInvitation { group_id: Uin, invitation_seq: i64 },
}

/// 请求处理状态（Milky `FriendRequest.state` 枚举）。
/// OFFICIAL: https://github.com/SaltifyDev/milky/blob/main/protocol/src/ir/api/friend.ts
///   (get_friend_requests → FriendRequest.state)。OneBot 无此字段 → `Unknown`。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RequestState {
    /// 待处理。
    Pending,
    /// 已同意。
    Accepted,
    /// 已拒绝。
    Rejected,
    /// 已忽略。
    Ignored,
    /// 协议未提供或为未知枚举值（绝不 panic）。
    Unknown,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Request {
    /// 好友请求。`target_user_id`/`state`/`time`/`is_filtered` 为 Milky
    /// `FriendRequest` 富字段（OneBot 无：`target_user_id`/`time` → None，
    /// `state` → `RequestState::Unknown`，`is_filtered` → false）。
    Friend { initiator: Uin, initiator_uid: Option<String>, comment: String, via: String, source_group: Option<Uin>, target_user_id: Option<Uin>, state: RequestState, time: Option<i64>, is_filtered: bool, token: RequestToken },
    /// 加群请求。`invitor` 为邀请人（Lagrange `request/group` 的 `invitor_id`：
    /// 经邀请链接申请入群时携带邀请人 uin）；无邀请人/其余协议为 None。
    GroupJoin { group: Uin, initiator: Uin, comment: String, invitor: Option<Uin>, is_filtered: bool, token: RequestToken },
    GroupInvitedJoin { group: Uin, initiator: Uin, target: Uin, token: RequestToken },
    GroupInvite { group: Uin, initiator: Uin, comment: String, source_group: Option<Uin>, token: RequestToken },
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Meta {
    /// 协议端（传输层）连上：由**框架的事件源**在底层 socket 连接成功时发出（正向/反向 WS、
    /// Milky WS 等），跨协议口径一致；每次（重）连都发一次。是「协议端连接」的权威信号。
    Connect,
    /// 协议端（传输层）断开：由**框架的事件源**在 socket 掉线、即将重连前发出。`reason`
    /// 为底层错误文案（如有）。正常停机不发。注意：传输断开 ≠ 账号掉线（见 [`Meta::BotOffline`]）。
    Disconnect { reason: Option<String> },
    /// 框架就绪：登录已解析出**可用账号**（`self_id != 0`）、`Bot` 句柄可正常动作。每次
    /// `run_*` 仅发**一次**（重连不重放）。需要「机器人完全就绪且有可用账号」才跑的逻辑
    /// （定时任务 / 加载后初始化）监听此事件即可：无可用账号则它根本不发、相关逻辑自然不跑。
    /// `nickname` 为登录账号的昵称（来自 `get_login_info`；未知时为空串）。
    Ready { self_id: Uin, nickname: String },
    Heartbeat { interval: i64, status: ImplStatus },
    /// 机器人上线（Lagrange `bot_online` / OneBot lifecycle enable）。`reason` 为
    /// Lagrange 上线原因文案（如重连/扫码）；无该信息的来源为 None。
    BotOnline { reason: Option<String> },
    BotOffline,
}

#[derive(Clone, Debug)]
pub struct RawEvent {
    pub protocol: Protocol,
    pub kind: String,
    pub raw: Value,
}

impl RequestToken {
    /// 由 OneBot flag 构造。
    pub fn onebot_flag(flag: impl Into<String>) -> Self {
        RequestToken(RequestTokenInner::OneBotFlag(flag.into()))
    }
}

impl Event {
    /// 本事件所属的可寻址会话（消息取其 peer；通知/请求取群或好友 peer），无则 `None`。
    /// 供开关门控与 waiter 限定作用域使用。
    pub fn peer(&self) -> Option<Peer> {
        use Notice as N;
        use Request as R;
        match self {
            Event::Message(m) => Some(m.peer),
            Event::Notice(n) => match n {
                N::Recall { peer, .. } => Some(*peer),
                N::PeerPinChange { peer, .. } => Some(*peer),
                N::MemberIncrease { group, .. }
                | N::MemberDecrease { group, .. }
                | N::AdminChange { group, .. }
                | N::Mute { group, .. }
                | N::WholeMute { group, .. }
                | N::GroupNameChange { group, .. }
                | N::Honor { group, .. }
                | N::GroupCardChange { group, .. }
                | N::GroupNudge { group, .. }
                | N::Reaction { group, .. }
                | N::EssenceChange { group, .. }
                | N::GroupFileUpload { group, .. }
                | N::GroupDismiss { group, .. }
                | N::GroupTitleChange { group, .. }
                | N::LuckyKing { group, .. } => Some(Peer::group(*group)),
                N::FriendNudge { user, .. }
                | N::FriendFileUpload { user, .. }
                | N::FriendAdd { user } => Some(Peer::friend(*user)),
                // 群/私聊均可的 notice：有 group 即群 peer，否则好友 peer。
                N::InputStatus { user, group, .. }
                | N::PokeRecall { user, group, .. }
                | N::OnlineFile { user, group, .. }
                | N::FlashFile { user, group, .. }
                | N::GrayTip { user: Some(user), group, .. } => Some(match group {
                    Some(g) => Peer::group(*g),
                    None => Peer::friend(*user),
                }),
                // gray_tip 无 user（纯群灰条）时退化为群 peer（若有 group）。
                N::GrayTip { user: None, group: Some(g), .. } => Some(Peer::group(*g)),
                N::GrayTip { user: None, group: None, .. }
                | N::ProfileLike { .. }
                | N::BotOffline { .. }
                | N::Other { .. } => None,
            },
            Event::Request(r) => match r {
                R::Friend { initiator, .. } => Some(Peer::friend(*initiator)),
                R::GroupJoin { group, .. }
                | R::GroupInvitedJoin { group, .. }
                | R::GroupInvite { group, .. } => Some(Peer::group(*group)),
            },
            Event::Meta(_) | Event::Raw(_) => None,
        }
    }

    /// 若本事件发生在群里，返回群号，否则 `None`。
    pub fn group(&self) -> Option<Uin> {
        self.peer().and_then(|p| match p.scene {
            Scene::Group => Some(p.id),
            _ => None,
        })
    }

    /// 行为主体（消息发送者 / 通知操作者 / 请求发起者），无则 `None`。
    pub fn sender(&self) -> Option<Uin> {
        use Notice as N;
        use Request as R;
        match self {
            Event::Message(m) => Some(m.sender),
            Event::Notice(n) => match n {
                N::Recall { sender, .. } => Some(*sender),
                N::MemberIncrease { user, .. }
                | N::MemberDecrease { user, .. }
                | N::AdminChange { user, .. }
                | N::Honor { user, .. }
                | N::GroupCardChange { user, .. }
                | N::Reaction { user, .. }
                | N::GroupFileUpload { user, .. }
                | N::GroupTitleChange { user, .. }
                | N::InputStatus { user, .. }
                | N::PokeRecall { user, .. }
                | N::OnlineFile { user, .. }
                | N::FlashFile { user, .. }
                | N::LuckyKing { user, .. } => Some(*user),
                N::Mute { operator, .. } | N::WholeMute { operator, .. }
                | N::GroupNameChange { operator, .. }
                | N::GroupDismiss { operator, .. }
                | N::ProfileLike { operator, .. } => Some(*operator),
                N::GroupNudge { sender, .. } => Some(*sender),
                N::FriendNudge { user, .. } | N::FriendFileUpload { user, .. }
                | N::FriendAdd { user } => Some(*user),
                N::GrayTip { user, .. } => *user,
                N::EssenceChange { .. } | N::PeerPinChange { .. }
                | N::BotOffline { .. } | N::Other { .. } => None,
            },
            Event::Request(r) => match r {
                R::Friend { initiator, .. } | R::GroupJoin { initiator, .. }
                | R::GroupInvitedJoin { initiator, .. } | R::GroupInvite { initiator, .. } => Some(*initiator),
            },
            Event::Meta(_) | Event::Raw(_) => None,
        }
    }
}
