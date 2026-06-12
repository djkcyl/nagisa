//! `FromContext` 提取器（路1 核心）。Handler 的每个参数都按类型从 `Ctx` 注入。
//!
//! `Reject::Skip` = 本 handler 不适用（类型/场景没中），dispatch 继续传播；
//! `Reject::Error` = 提取出错（如缺失依赖），记日志、不触发本 handler。
use crate::bot::Bot;
use crate::ctx::{Ctx, StateMap};
use crate::matcher::ParsedCommand;
use async_trait::async_trait;
use nagisa_types::event::{MessageEvent, Meta, Notice, Request};
use nagisa_types::prelude::*;
use nagisa_types::segment::Segment;
use std::any::{type_name, TypeId};
use std::sync::Arc;

/// 提取失败的两种语义。
#[derive(Debug)]
pub enum Reject {
    /// 本 handler 不适用（匹配器/类型过滤没中）；dispatch 跳过它、继续。
    Skip,
    /// 提取真正出错（如缺失 `State`）；记日志，不触发本 handler。
    Error(Error),
}

/// 提取结果：成功得 `Self`，失败为 `Reject`（注意区别于 `nagisa_types::Result`）。
pub type Extracted<T> = std::result::Result<T, Reject>;

/// 让 `Extracted<()>`（即 `Result<(), Reject>`）能用 `?` 直接落进 handler 体——
/// handler 返回 [`HandlerResult`](crate::handler::HandlerResult) = `Result<(), nagisa_types::Error>`，
/// 故 `?` 需要 `Reject -> Error` 的转换（招牌写法
/// `async fn h(cd: Cd, ..) { cd.gate(format!("bili:{aid}"), D60)?; reduce_gold().await?; .. }`
/// 要求 `cd.gate(..)?` 与 `reduce_gold().await?` 在同一函数体里共用同一错误类型）。
///
/// 映射：
/// - `Reject::Error(e)` → 原样的业务错误 `e`（提取真出错，照常上报）。
/// - `Reject::Skip` → 一个 `BadParams` 分类、哨兵 retcode 的 `Error`：在 handler 体里
///   `?`-早退即“本次不处理”（如冷却中、数据派生键命中冷却）；dispatch 会把它记一条
///   handler WARN 并继续传播。这是 handler **体内**的早退语义，区别于提取阶段
///   `Skip` 的“静默跳过本 handler”——进了体内 handler 已在运行，没有再“跳过”的位置，
///   故落为一次明确的、可见的早退。
///
/// 孤儿规则：源类型 [`Reject`] 是本 crate 本地类型，故可为外部 `Error` 实现 `From`。
impl From<Reject> for Error {
    fn from(r: Reject) -> Self {
        match r {
            Reject::Error(e) => e,
            Reject::Skip => Error::action_kind(
                ActionErrorKind::BadParams,
                "handler skipped (Reject::Skip propagated via `?`, e.g. cooldown hit)",
            ),
        }
    }
}

/// 从每事件上下文按类型提取一个值。`Skip` 即类型层过滤。
#[async_trait]
pub trait FromContext: Sized {
    async fn from_context(ctx: &Ctx) -> Extracted<Self>;
}

#[async_trait]
impl FromContext for Bot {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        Ok(ctx.bot().clone())
    }
}

/// 可选提取：内层 `Skip`（类型/场景没中）→ `None`，于是本 handler 仍会运行；
/// 内层 `Error`（真出错）继续向上传播。把"否则整条 handler 被跳过"变成"该项可选"。
#[async_trait]
impl<T: FromContext + Send> FromContext for Option<T> {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        match T::from_context(ctx).await {
            Ok(v) => Ok(Some(v)),
            Err(Reject::Skip) => Ok(None),
            Err(Reject::Error(e)) => Err(Reject::Error(e)),
        }
    }
}

#[async_trait]
impl FromContext for Arc<Event> {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        Ok(Arc::clone(ctx.event()))
    }
}

#[async_trait]
impl FromContext for MessageEvent {
    /// 非消息事件 → `Skip`（类型层过滤）。命中则克隆内层 `MessageEvent`。
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        ctx.message().cloned().ok_or(Reject::Skip)
    }
}

/// 群消息提取器：仅当事件是 `Scene::Group` 的消息时命中，否则 `Skip`。
pub struct GroupMessage(pub MessageEvent);

#[async_trait]
impl FromContext for GroupMessage {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        match ctx.message() {
            Some(m) if m.peer.scene == Scene::Group => Ok(GroupMessage(m.clone())),
            _ => Err(Reject::Skip),
        }
    }
}

/// 私聊消息提取器：仅当事件是 `Scene::Friend`/`Scene::Temp` 的消息时命中，否则 `Skip`。
pub struct PrivateMessage(pub MessageEvent);

#[async_trait]
impl FromContext for PrivateMessage {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        match ctx.message() {
            Some(m) if matches!(m.peer.scene, Scene::Friend | Scene::Temp) => Ok(PrivateMessage(m.clone())),
            _ => Err(Reject::Skip),
        }
    }
}

/// 发送者 QQ 号（仅消息事件，否则 `Skip`）。
pub struct Sender(pub Uin);

#[async_trait]
impl FromContext for Sender {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        ctx.message().map(|m| Sender(m.sender)).ok_or(Reject::Skip)
    }
}

/// 消息事件的对端寻址（仅消息事件，否则 `Skip`）。
pub struct EventPeer(pub Peer);

#[async_trait]
impl FromContext for EventPeer {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        ctx.message().map(|m| EventPeer(m.peer)).ok_or(Reject::Skip)
    }
}

/// 会话场景提取器：复用既有 [`Scene`](nagisa_types::id::Scene)（`Friend`/`Group`/`Temp`），
/// **不**新增第二个 `Scene` 枚举（那会与 prelude 同名类型冲突）。一个 handler 取它即可在
/// 同一处按对端种类分支、再用 [`Reply`] 回到当下这个对端——于是好友/群两份重复 handler 收敛为一。
///
/// 配合 peer-agnostic 的角色门控（`group_admin() | superuser()` 两个场景都可用），
/// 同一 handler 还能据此派生 waiter 的 `.block(scene)`（按场景条件吞没）。
///
/// 非消息事件 → `Skip`（无对端场景可言）。
#[async_trait]
impl FromContext for nagisa_types::id::Scene {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        // `peer.scene` 已是 `Scene`（id.rs）。
        ctx.message().map(|m| m.peer.scene).ok_or(Reject::Skip)
    }
}

/// app 共享状态注入。从 `Router::data` 注册的状态表里取 `Arc<T>`；缺失 → `Reject::Error`。
///
/// 实现了 `Deref<Target = T>`，故 handler 可直接写 `state.field` 而非 `state.0.field`；
/// 元组字段 `.0`（`Arc<T>`）仍保留，需要克隆 `Arc` 时可用。
pub struct State<T: Send + Sync + 'static>(pub Arc<T>);

impl<T: Send + Sync + 'static> std::ops::Deref for State<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

#[async_trait]
impl<T: Send + Sync + 'static> FromContext for State<T> {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        let state: &StateMap = ctx.state();
        match state.get(&TypeId::of::<T>()) {
            Some(any) => match Arc::clone(any).downcast::<T>() {
                Ok(typed) => Ok(State(typed)),
                // 同一 TypeId 必然同类型，downcast 不会失败；保守兜底。
                Err(_) => Err(Reject::Error(Error::action(format!("state type mismatch for {}", type_name::<T>())))),
            },
            None => Err(Reject::Error(Error::action_kind(
                ActionErrorKind::BadParams,
                format!("missing app state `{}` (register via Router::data)", type_name::<T>()),
            ))),
        }
    }
}

/// Notice 事件提取器：仅当事件是 `Event::Notice` 时命中，否则 `Skip`。
#[async_trait]
impl FromContext for Notice {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        match ctx.event().as_ref() {
            Event::Notice(n) => Ok(n.clone()),
            _ => Err(Reject::Skip),
        }
    }
}

/// Request 事件提取器：仅当事件是 `Event::Request` 时命中，否则 `Skip`。
#[async_trait]
impl FromContext for Request {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        match ctx.event().as_ref() {
            Event::Request(r) => Ok(r.clone()),
            _ => Err(Reject::Skip),
        }
    }
}

/// Meta 事件提取器：仅当事件是 `Event::Meta` 时命中，否则 `Skip`。
#[async_trait]
impl FromContext for Meta {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        match ctx.event().as_ref() {
            Event::Meta(m) => Ok(m.clone()),
            _ => Err(Reject::Skip),
        }
    }
}

/// 便捷回复句柄：携带消息事件的 `Peer` + 触发消息的 `MessageId` + `Bot`，向同一会话回话。
/// 仅消息事件可提取（否则 `Skip`）。
pub struct Reply {
    peer: Peer,
    /// 触发本次回复的消息 id（来自 `MessageEvent::id`）。
    /// 供 [`quote`](Self::quote)/[`reply`](Self::reply) 引用原消息。
    trigger: MessageId,
    bot: Bot,
}

impl Reply {
    /// 向来源会话发送一段纯文本。
    pub async fn text(&self, s: impl Into<String>) -> Result<MessageId> {
        self.bot.send(&self.peer, &[Segment::text(s)]).await
    }

    /// 向来源会话发送任意消息段。
    pub async fn send(&self, segs: &[Segment]) -> Result<MessageId> {
        self.bot.send(&self.peer, segs).await
    }

    /// 引用触发消息回复任意消息段（在段前插入一个 `Reply` 段）。
    pub async fn quote(&self, segs: &[Segment]) -> Result<MessageId> {
        let mut out = Vec::with_capacity(segs.len() + 1);
        out.push(Segment::reply(self.trigger.clone()));
        out.extend_from_slice(segs);
        self.bot.send(&self.peer, &out).await
    }

    /// 引用触发消息回复一段纯文本。
    pub async fn reply(&self, s: impl Into<String>) -> Result<MessageId> {
        self.quote(&[Segment::text(s)]).await
    }

    /// 向来源会话发送一张图片（URL）。
    pub async fn image(&self, url: impl Into<String>) -> Result<MessageId> {
        self.bot.send(&self.peer, &[Segment::image_url(url)]).await
    }

    /// 向来源会话发送一个 @ 提及。
    pub async fn at(&self, user: impl Into<Uin>) -> Result<MessageId> {
        self.bot.send(&self.peer, &[Segment::at(user)]).await
    }

    /// 向来源会话发送一个 QQ 表情。
    pub async fn face(&self, id: impl Into<String>) -> Result<MessageId> {
        self.bot.send(&self.peer, &[Segment::face(id)]).await
    }

    /// 链式构造一条回复，镜像 `Msg` 构建体：
    /// `reply.msg().at(user).text("..").send().await?`。
    /// 比手搓 `Vec<Segment>` 再 `reply.send(&segs)` 更顺手。
    pub fn msg(&self) -> ReplyMsg<'_> {
        ReplyMsg { reply: self, msg: Msg::new() }
    }

    /// 回复目标会话寻址。
    pub fn peer(&self) -> &Peer {
        &self.peer
    }

    /// 触发本次回复的消息 id。
    pub fn trigger(&self) -> &MessageId {
        &self.trigger
    }
}

/// [`Reply::msg`] 返回的链式构建体：累积 [`Msg`] 段，终结于 [`send`](Self::send)
/// （向来源会话发送）或 [`quote`](Self::quote)（引用触发消息再发送）。
/// 字段方法照搬 `Msg`，故 `reply.msg().at(u).text("hi")` 与 `Msg` 同手感。
pub struct ReplyMsg<'a> {
    reply: &'a Reply,
    msg: Msg,
}

impl ReplyMsg<'_> {
    pub fn text(mut self, s: impl Into<String>) -> Self {
        self.msg = self.msg.text(s);
        self
    }
    pub fn at(mut self, user: impl Into<Uin>) -> Self {
        self.msg = self.msg.at(user);
        self
    }
    pub fn at_all(mut self) -> Self {
        self.msg = self.msg.at_all();
        self
    }
    pub fn face(mut self, id: impl Into<String>) -> Self {
        self.msg = self.msg.face(id);
        self
    }
    pub fn image_url(mut self, url: impl Into<String>) -> Self {
        self.msg = self.msg.image_url(url);
        self
    }
    /// 追加一张内存图片（PNG/GIF/… 字节）。镜像 [`Msg::image_bytes`]，让现渲染现发的
    /// 插件能走 `reply.msg().image_bytes(png).text("..").send()` 而非手搓 `Segment` 数组。
    pub fn image_bytes(mut self, bytes: impl Into<bytes::Bytes>) -> Self {
        self.msg = self.msg.image_bytes(bytes);
        self
    }
    /// 追加一张本地文件图片（镜像 [`Msg::image_path`]，三态与 `image_url`/`image_bytes` 对齐）。
    pub fn image_path(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.msg = self.msg.image_path(path);
        self
    }
    /// 在已累积段前引用触发消息（等价于 [`Reply::quote`] 的链式入口）。
    pub fn reply_to_trigger(mut self) -> Self {
        self.msg = self.msg.reply(self.reply.trigger.clone());
        self
    }
    /// 追加任意段。
    pub fn push(mut self, seg: Segment) -> Self {
        self.msg = self.msg.push(seg);
        self
    }
    /// 发送累积的消息到来源会话。
    pub async fn send(self) -> Result<MessageId> {
        self.reply.send(&self.msg.build()).await
    }
    /// 引用触发消息后发送累积的消息。
    pub async fn quote(self) -> Result<MessageId> {
        self.reply.quote(&self.msg.build()).await
    }
}

#[async_trait]
impl FromContext for Reply {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        match ctx.message() {
            Some(m) => Ok(Reply { peer: m.peer, trigger: m.id.clone(), bot: ctx.bot().clone() }),
            None => Err(Reject::Skip),
        }
    }
}

// —— 命令提取器：读取匹配器在 dispatch 时存入 `ctx` 扩展的 `ParsedCommand`。——
// 匹配器命中后 `ctx.insert_ext(parsed)`；故仅在「命令型 handler」内可提取，
// 普通 handler（无匹配器）下这些提取器 `Skip`。

/// 命令之后剩余的消息段（保留非文本段，如图片）。无 `ParsedCommand` 时 `Skip`。
pub struct CommandArg(pub Vec<Segment>);

#[async_trait]
impl FromContext for CommandArg {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        ctx.get_ext::<ParsedCommand>().map(|p| CommandArg(p.args)).ok_or(Reject::Skip)
    }
}

/// 命令剩余段的纯文本。无 `ParsedCommand` 时 `Skip`。
pub struct ArgText(pub String);

#[async_trait]
impl FromContext for ArgText {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        ctx.get_ext::<ParsedCommand>().map(|p| ArgText(p.args_text)).ok_or(Reject::Skip)
    }
}

/// 命中的命令字面量（或正则整段匹配）。无 `ParsedCommand` 时 `Skip`。
pub struct Command(pub String);

#[async_trait]
impl FromContext for Command {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        ctx.get_ext::<ParsedCommand>().map(|p| Command(p.command)).ok_or(Reject::Skip)
    }
}

/// 正则捕获组（非正则匹配器时为空）。无 `ParsedCommand` 时 `Skip`。
pub struct Captures(pub Vec<String>);

#[async_trait]
impl FromContext for Captures {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        ctx.get_ext::<ParsedCommand>().map(|p| Captures(p.captures)).ok_or(Reject::Skip)
    }
}

// —— 元素提取器：从「匹配后的消息链」抽取首个 @ / 图片，与 waiter 统一。——
//
// 招牌赌注：**读取内联元素的类型，正是 `recv::<Image>` 返回的类型** —— 于是
// 「内联 OR 追问」是同一个类型：
//   let img = match img_opt { Some(i) => i, None => {            // 内联 OR 追问，一个类型
//       reply.text("发一张图").await?;
//       waiter.recv::<Image>(D30).await.ok_or(Timeout)?         // recv::<T: FromContext> 已存在
//   }};
//
// 统一靠同一份「匹配链」来源：命令型 handler 里读 `ParsedCommand.args`（命令头之后的
// 剩余段）；waiter 投递的新消息没有 `ParsedCommand`，回退读整条消息 `content`。两路都走
// 同一个 `seg_as_at`/`seg_as_image` 投影，故 `recv::<Image>`（追问得来的内联图）与内联
// `Image` 槽是**同一个类型**。
//
// 「匹配链」上跑投影 `f`：有 `ParsedCommand` 跑其 `args`（命令头之后的剩余段），否则跑
// 整条消息 `content`（waiter 投递的新消息没有 `ParsedCommand`）。两路同一份投影，于是
// 内联槽与 `recv::<T>` 是同一个类型。非消息且无 `ParsedCommand` → `None`（提取器据此 `Skip`）。
fn first_in_chain<R>(ctx: &Ctx, f: impl Fn(&Segment) -> Option<R>) -> Option<R> {
    if let Some(p) = ctx.get_ext::<ParsedCommand>() {
        return p.args.iter().find_map(&f);
    }
    ctx.message()?.content.iter().find_map(&f)
}

/// 匹配链中首个 @ 提及，投影为被 @ 的 `Uin`。链中无 mention（或非消息事件）→ `Skip`。
///
/// 用于「内联 @ 某人 OR 追问」：`async fn h(At(target): At, ..)` 或 `Option<At>` 可选。
pub struct At(pub Uin);

#[async_trait]
impl FromContext for At {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        first_in_chain(ctx, crate::args::seg_as_at).map(At).ok_or(Reject::Skip)
    }
}

/// 匹配链中首个图片段，投影为 [`Media`]（含 `recv.url`）。链中无图片（或非消息事件）→ `Skip`。
///
/// 容忍前导文本/换行：在段流里 `find_map` 找首个图片，前面的文本段自然被跳过。
///
/// 这就是 `recv::<Image>` 返回的类型——内联图与追问得来的图统一为一个类型。
pub struct Image(pub Media);

#[async_trait]
impl FromContext for Image {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        first_in_chain(ctx, crate::args::seg_as_image).map(Image).ok_or(Reject::Skip)
    }
}

/// 消息是否「to me」：私聊/Temp 场景，或群聊里首段是 `@self` mention。
///
/// 此提取器独立于 [`Matcher`]：即便没有命令匹配器，只要是消息事件就可提取。
/// 非消息事件返回 `Skip`。
///
/// [`Matcher`]: crate::matcher::Matcher
pub struct ToMe(pub bool);

#[async_trait]
impl FromContext for ToMe {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        let msg = ctx.message().ok_or(Reject::Skip)?;
        // 私聊 / Temp 天然 to_me。
        if matches!(msg.peer.scene, Scene::Friend | Scene::Temp) {
            return Ok(ToMe(true));
        }
        // 群聊：首段为 @self 则 to_me。
        let self_id = ctx.bot().self_id();
        let to_me =
            msg.content.first().is_some_and(|seg| matches!(seg, Segment::Mention { user, .. } if *user == self_id));
        Ok(ToMe(to_me))
    }
}
