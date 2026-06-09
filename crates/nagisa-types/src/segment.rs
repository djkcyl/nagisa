//! 统一消息段 [`Segment`] 及其附属类型（图片子类型、合并转发、音乐分享、媒体下载提示等）。
//! 业务收发用同一套 [`Segment`]，wire 级的入/出差异由适配器私有承担；协议私有/未知段一律落到
//! [`Segment::Raw`] 逃生口，绝不丢弃。[`Segment`] 上的关联函数（`text` / `at` / `image_url` …）
//! 是常用段的便捷构造器。
use crate::capability::Protocol;
use crate::id::{MessageId, Uin};
use crate::resource::{Media, ResourceSource};
use serde_json::{Map, Value};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImageSubType {
    Normal,
    /// 大表情/原创表情(OneBot `subType` 非 0)。
    Sticker,
    /// 闪照(阅后即焚):OneBot `data.type = "flash"` / Mirai `FlashImage`。内容过滤、
    /// 反撤回类插件需按此分支。
    Flash,
}

/// `Contact` 段的推荐目标类型。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ContactKind {
    Friend,
    Group,
}

/// 音乐分享:平台预设(qq/163/kugou/migu/kuwo…)或自定义卡片。
#[derive(Clone, Debug)]
pub enum MusicShare {
    /// 平台预设:`ty` 为平台标识(qq/163/kugou/migu/kuwo),`id` 为歌曲 id。
    Platform { ty: String, id: String },
    /// 自定义分享卡片。
    Custom {
        url: String,
        audio: String,
        title: String,
        content: Option<String>,
        image: Option<String>,
    },
}

/// 合并转发：接收为引用 + 预览元信息；发送为内联节点。
#[derive(Clone, Debug)]
pub enum Forward {
    Ref {
        id: String,
        title: Option<String>,
        preview: Vec<String>,
        summary: Option<String>,
    },
    Nodes {
        nodes: Vec<ForwardNode>,
        title: Option<String>,
        summary: Option<String>,
        prompt: Option<String>,
        /// 自定义合并转发卡片的预览行（gocq/NapCat `news`，形如 `[{text}]`）。
        /// 空 → 不写出该字段。
        news: Vec<String>,
        /// 自定义合并转发卡片来源标题（gocq/NapCat `source`）。缺省 → None。
        source: Option<String>,
    },
}

/// 媒体段发送侧下载行为提示（OneBot image/record/video 的 `cache`/`proxy`/`timeout`）。
/// 仅发送侧有意义；接收侧恒为默认。Milky 无对应字段（降级忽略）。
/// OFFICIAL: https://github.com/botuniverse/onebot-11/blob/master/message/segment.md (§图片/§语音/§短视频 cache/proxy/timeout)
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MediaSendHints {
    /// 是否使用已缓存的文件（`false` → 不使用缓存）。
    pub cache: Option<bool>,
    /// 是否通过代理下载远程文件。
    pub proxy: Option<bool>,
    /// 远程文件下载超时秒数。
    pub timeout: Option<i32>,
}

/// 合并转发的单个节点（发送者 + 内容）。
///
/// **跨协议非对称（固有限制）**：[`user`](Self::user) 在 **Milky** 后端解码时恒为
/// `Uin(0)` 哨兵——Milky 的 `IncomingForwardedMessage` 只带 `sender_name`，wire 上
/// 没有发送者 `user_id`，故无法填充真实 uin（见 `nagisa-milky/src/decode.rs`
/// `forward_node_from_value`）。OneBot 的 `node` 段带 `user_id`，能正确填充。下游若在
/// Milky 后端按 `user` 寻址转发节点的发送者，须以 `name`（sender_name）为准——这是
/// Milky IR 的天然缺口，非解码缺陷。
#[derive(Clone, Debug)]
pub struct ForwardNode {
    /// 节点发送者 QQ 号。Milky 后端无此 wire 字段 → 恒为 `Uin(0)`（详见结构体级文档）。
    pub user: Uin,
    pub name: String,
    pub content: Vec<Segment>,
    /// 合并转发节点气泡时间戳（部分实现收发；可缺省）。
    pub time: Option<i64>,
}

/// 统一消息段。`#[non_exhaustive]`：加段不破坏下游。
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Segment {
    Text(String),
    Mention { user: Uin, name: Option<String> },
    MentionAll,
    /// QQ 表情段。`id`/`large` 为 OneBot v11 标准字段。
    /// `result_id`/`chain_count` 为 NapCat「超级表情」(super-face) 扩展（连发动画表情，
    /// 仅 NapCat 收发；其余厂商缺省 → None）。`sub_type` 为 LLOneBot 的 `FaceType`
    /// （表情子类型，如 0=普通 / 1=超级 / 2=原创，仅 LLOneBot 透传 → 其余 None）。
    /// 来源已核对（2026-06-04）：NapCat `OB11MessageFace` data `resultId`(string)/`chainCount`(number)；
    /// LLOneBot `OB11MessageFace` data `sub_type`(number, FaceType)。
    /// ENDPOINT: NapNeko/NapCatQQ packages/napcat-onebot/types/message.ts (OB11MessageFaceSchema)
    ///   + LLOneBot/LLOneBot src/onebot11/types.ts (OB11MessageFace)。
    Face { id: String, large: bool, result_id: Option<String>, chain_count: Option<i32>, sub_type: Option<i32> },
    Reply { id: MessageId, sender: Option<Uin>, time: Option<i64>, quoted: Vec<Segment> },
    Image { res: Media, sub_type: ImageSubType, hints: MediaSendHints },
    Record { res: Media, magic: Option<i32>, hints: MediaSendHints },
    /// 视频段。`thumb`（封面缩略图）为**跨协议非对称（固有限制）**字段：**标准 OneBot v11
    /// 的 video 段没有 thumb wire 字段**，故纯 v11 端 encode 时必然丢弃 `thumb`；nagisa 仍会
    /// 在 encode 时写出 `thumb` 键（LLOneBot/go-cqhttp 扩展接受它，标准端忽略多发键，无害——
    /// 见 `nagisa-onebot/src/encode.rs`），但**标准 v11 解码侧 `thumb` 恒为 `None`**。Milky 有
    /// 对称的 `thumb_uri` wire 字段，能完整收发。即：`thumb` 在标准 OneBot v11 下是只写不读
    /// 的有损字段，这是 v11 wire 的天然缺口，非编解码缺陷。
    Video { res: Media, hints: MediaSendHints, thumb: Option<ResourceSource> },
    File { id: String, name: String, size: u64, hash: Option<String>, url: Option<String> },
    Forward(Forward),
    MarketFace { package_id: i32, emoji_id: String, key: String, summary: Option<String>, url: Option<String> },
    LightApp { app_name: Option<String>, payload: String },
    /// XML 卡片（OneBot `xml` / Milky 入站 `xml`）。与 `LightApp` 对称，避免 json 有
    /// 类型而 xml 退化成 `Raw` 的不对称。
    Xml { service_id: Option<i32>, payload: String },
    /// 戳一戳消息段（OneBot）。注意区别于 `send_nudge` 动作与 nudge 通知。
    Poke { kind: i32, id: i32, strength: Option<i32>, name: Option<String> },
    /// 推荐好友/群名片。
    Contact { kind: ContactKind, id: Uin },
    /// 位置分享。
    Location { lat: f64, lon: f64, title: Option<String>, content: Option<String> },
    /// 音乐分享（发送向；接收通常表现为 `LightApp`）。
    Music(MusicShare),
    /// 链接分享卡片（OneBot `share`）。`url`/`title` 必填，`content`/`image` 可选。
    /// 与已建模的 LightApp/Xml/Music 卡片并列；Milky 发送侧无对应段（降级跳过）。
    Share { url: String, title: String, content: Option<String>, image: Option<String> },
    /// 猜拳魔法表情（OneBot `rps`，收发皆有）。标准 v11 为空 data；NapCat 额外带
    /// `result`（1=布 / 2=剪刀 / 3=石头，随机数已定）→ `result` 保留（缺省 → None）。
    /// ENDPOINT: NapNeko/NapCatQQ packages/napcat-onebot/types/message.ts (OB11MessageRPS data `result`)
    Rps { result: Option<i32> },
    /// 掷骰子魔法表情（OneBot `dice`，收发皆有）。标准 v11 为空 data；NapCat 额外带
    /// `result`（1–6 点数）→ `result` 保留（缺省 → None）。
    /// ENDPOINT: NapNeko/NapCatQQ packages/napcat-onebot/types/message.ts (OB11MessageDice data `result`)
    Dice { result: Option<i32> },
    /// 窗口抖动 / 戳一戳快捷（OneBot `shake`，空 data，仅发送）。
    Shake,
    /// 匿名发送（OneBot `anonymous`，仅发送）。`ignore=Some(true)` 表示无法匿名时
    /// 继续以普通身份发送；`None` = 不带该字段。
    Anonymous { ignore: Option<bool> },
    /// QQ-Bot 内联键盘（Lagrange `keyboard` 段）。`content` 为 `KeyboardData` JSON
    /// 对象（按钮行/列）。Milky 无对应段（出站降级跳过）。
    /// 来源已核对（2026-06-04）：type=`"keyboard"`、data 字段 `content`（KeyboardData JSON）。
    /// ENDPOINT: LagrangeDev/Lagrange.Core Lagrange.OneBot/Message/Entity/KeyboardSegment.cs
    Keyboard { content: Value },
    /// Markdown 卡片（Lagrange `markdown` 段）。`content` 为 Markdown 文本字符串。
    /// Milky 无对应段（出站降级跳过）。
    /// 来源已核对（2026-06-04）：type=`"markdown"`、data 字段 `content`（string）。
    /// ENDPOINT: LagrangeDev/Lagrange.Core Lagrange.OneBot/Message/Entity/MarkdownSegment.cs
    Markdown { content: String },
    /// 长消息引用（Lagrange `longmsg` 段，主要入站）。`id` 为长消息 res_id。
    /// Milky 无对应段（出站降级跳过）。
    /// 来源已核对（2026-06-04）：type=`"longmsg"`、data 字段 `id`（string，**非** `res_id`）。
    /// ENDPOINT: LagrangeDev/Lagrange.Core Lagrange.OneBot/Message/Entity/LongMsgSegment.cs
    LongMsg { id: String },
    /// QQ 闪传卡片（LLOneBot 私有 `flash_file` 段）。入站由 LLOneBot 解析「闪传」
    /// markdown 卡片得到；出站可据此重建段。`title` 在 wire 上偶有缺省（标题属性可能
    /// 取不到），故为 `Option`（缺失 → None，保持 decode 无误）。
    /// 来源已核对（2026-06-04）：type=`"flash_file"`、data 字段
    /// `title`(string,可缺)/`file_set_id`(string)/`scene_type`(number)。
    /// ENDPOINT: LLOneBot/LLOneBot src/onebot11/types.ts (OB11MessageFlashFile)
    ///   + src/onebot11/transform/message/incoming.ts。
    FlashFile { title: Option<String>, file_set_id: String, scene_type: i32 },
    /// 小程序卡片（NapCat 私有 `miniapp` 段）。`payload` 为小程序的 JSON 字符串
    /// （NapCat data 字段 `data`，原样透传，便于手工构建/转发）。与 LightApp/Xml 并列。
    /// 来源已核对（2026-06-04）：type=`"miniapp"`、data 字段 `data`（string，小程序 JSON）。
    /// ENDPOINT: NapNeko/NapCatQQ packages/napcat-onebot/types/message.ts (OB11MessageMiniAppSchema)
    MiniApp { payload: String },
    /// 在线文件/文件夹卡片（NapCat 私有 `onlinefile` 段）。
    /// 来源已核对（2026-06-04）：type=`"onlinefile"`、data 字段
    /// `msgId`(string)/`elementId`(string)/`fileName`(string)/`fileSize`(string)/`isDir`(bool)。
    /// ENDPOINT: NapNeko/NapCatQQ packages/napcat-onebot/types/message.ts (OB11MessageOnlineFileSchema)
    OnlineFile { msg_id: String, element_id: String, file_name: String, file_size: String, is_dir: bool },
    /// QQ 闪传卡片（NapCat 私有 `flashtransfer` 段）。`file_set_id` 为闪传文件集 id。
    /// 与 LLOneBot 的 `FlashFile` 同属闪传但 wire 名/字段不同，故各自建模。
    /// 来源已核对（2026-06-04）：type=`"flashtransfer"`、data 字段 `fileSetId`（string）。
    /// ENDPOINT: NapNeko/NapCatQQ packages/napcat-onebot/types/message.ts (OB11MessageFlashTransferSchema)
    FlashTransfer { file_set_id: String },
    /// 逃生口：协议私有/未知段。adapter 的 decode 必须把未知段塞进来，绝不丢弃。
    Raw { protocol: Protocol, kind: String, data: Map<String, Value> },
}

impl Segment {
    pub fn text(s: impl Into<String>) -> Self {
        Segment::Text(s.into())
    }
    pub fn at(user: impl Into<Uin>) -> Self {
        Segment::Mention { user: user.into(), name: None }
    }
    pub fn at_all() -> Self {
        Segment::MentionAll
    }
    pub fn face(id: impl Into<String>) -> Self {
        Segment::Face { id: id.into(), large: false, result_id: None, chain_count: None, sub_type: None }
    }
    pub fn image_bytes(b: impl Into<bytes::Bytes>) -> Self {
        Segment::Image {
            res: Media::from_source(ResourceSource::bytes(b)),
            sub_type: ImageSubType::Normal,
            hints: MediaSendHints::default(),
        }
    }
    pub fn image_url(u: impl Into<String>) -> Self {
        Segment::Image {
            res: Media::from_source(ResourceSource::url(u)),
            sub_type: ImageSubType::Normal,
            hints: MediaSendHints::default(),
        }
    }
    /// 本地文件图片（[`ResourceSource::Path`]，发送时由 adapter 读取/编码）。
    /// 与 [`image_bytes`](Self::image_bytes)/[`image_url`](Self::image_url) 三态对齐。
    pub fn image_path(p: impl Into<std::path::PathBuf>) -> Self {
        Segment::Image {
            res: Media::from_source(ResourceSource::path(p)),
            sub_type: ImageSubType::Normal,
            hints: MediaSendHints::default(),
        }
    }
    pub fn reply(id: MessageId) -> Self {
        Segment::Reply { id, sender: None, time: None, quoted: Vec::new() }
    }
    pub fn xml(payload: impl Into<String>) -> Self {
        Segment::Xml { service_id: None, payload: payload.into() }
    }
    pub fn poke(kind: i32, id: i32) -> Self {
        Segment::Poke { kind, id, strength: None, name: None }
    }
    pub fn location(lat: f64, lon: f64) -> Self {
        Segment::Location { lat, lon, title: None, content: None }
    }
    pub fn music(platform: impl Into<String>, id: impl Into<String>) -> Self {
        Segment::Music(MusicShare::Platform { ty: platform.into(), id: id.into() })
    }
    pub fn share(url: impl Into<String>, title: impl Into<String>) -> Self {
        Segment::Share { url: url.into(), title: title.into(), content: None, image: None }
    }
    pub fn anonymous(ignore: bool) -> Self {
        Segment::Anonymous { ignore: Some(ignore) }
    }
    /// 带 summary(替代文本)的图片。Milky 出站 image.summary / OneBot image summary。
    pub fn image_url_with_summary(u: impl Into<String>, summary: impl Into<String>) -> Self {
        let mut res = Media::from_source(ResourceSource::url(u));
        res.summary = Some(summary.into());
        Segment::Image { res, sub_type: ImageSubType::Normal, hints: MediaSendHints::default() }
    }
    /// 视频 URL 段（无缩略图）。
    pub fn video_url(u: impl Into<String>) -> Self {
        Segment::Video {
            res: Media::from_source(ResourceSource::url(u)),
            hints: MediaSendHints::default(),
            thumb: None,
        }
    }
    /// 若是文本段返回其内容。
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Segment::Text(t) => Some(t),
            _ => None,
        }
    }
    /// 合并转发段的便捷入口：`Segment::forward(nodes)` ≡
    /// `Segment::Forward(Forward::nodes(nodes))`。需要标题/卡片预览时用
    /// [`Forward::nodes`] + 链式 setter，再自行 `Segment::Forward(..)`。
    pub fn forward(nodes: Vec<ForwardNode>) -> Self {
        Segment::Forward(Forward::nodes(nodes))
    }
}

impl Forward {
    /// 内联合并转发的常用构造：只给节点，4 个少用字段
    /// （`summary`/`prompt`/`news`/`source`）默认空。需要其一时用链式 setter 或直接
    /// 构造 [`Forward::Nodes`] 结构体变体。镜像 `Segment::share`/`music` 的默认化约定。
    pub fn nodes(nodes: Vec<ForwardNode>) -> Self {
        Forward::Nodes {
            nodes,
            title: None,
            summary: None,
            prompt: None,
            news: Vec::new(),
            source: None,
        }
    }
    /// 设卡片标题（仅对 [`Forward::Nodes`] 生效；`Ref` 变体原样返回）。
    pub fn title(mut self, t: impl Into<String>) -> Self {
        if let Forward::Nodes { title, .. } = &mut self {
            *title = Some(t.into());
        }
        self
    }
}

impl ForwardNode {
    /// 一个转发节点：发送者 + 名字 + 任意段内容（`time` 默认 `None`）。
    pub fn new(user: impl Into<Uin>, name: impl Into<String>, content: Vec<Segment>) -> Self {
        ForwardNode { user: user.into(), name: name.into(), content, time: None }
    }
    /// 最常见的「一个发送者说一段纯文本」节点，省去各插件自造的 `node()`/`text_node()`。
    pub fn text(user: impl Into<Uin>, name: impl Into<String>, text: impl Into<String>) -> Self {
        ForwardNode::new(user, name, vec![Segment::text(text)])
    }
    /// 设节点气泡时间戳（链式）。
    pub fn at_time(mut self, t: i64) -> Self {
        self.time = Some(t);
        self
    }

    /// 把一组「逻辑条目」打包成每节点 ≤ `max_chars` 字的若干纯文本转发节点 —— **按条目切，绝不拆条目**。
    ///
    /// 节点内条目以 `sep` 连接；累计字数（含 `sep`）超过上限就另起一节点，但一个条目永远完整落在
    /// 某一个节点里（条目自身就超限时它独占一节点，也不切开）。字数按 Unicode 字符计，`name` 每节点重复。
    /// 适合「N 条漂流瓶 / N 条记录 / N 条命令」这类列表：按条目而非按字断开。条目为空 ⇒ 空 `Vec`。
    pub fn chunk_items<I, S>(
        user: impl Into<Uin>,
        name: impl Into<String>,
        items: I,
        sep: &str,
        max_chars: usize,
    ) -> Vec<ForwardNode>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let user = user.into();
        let name = name.into();
        let sep_len = sep.chars().count();
        let mut nodes = Vec::new();
        let mut buf = String::new();
        let mut len = 0usize;
        for item in items {
            let item: String = item.into();
            let il = item.chars().count();
            // 装不下就先把当前节点收口，再放这个条目（条目本身整块进新节点，不切开）。
            if !buf.is_empty() && len + sep_len + il > max_chars {
                nodes.push(ForwardNode::text(user, name.clone(), std::mem::take(&mut buf)));
                len = 0;
            }
            if !buf.is_empty() {
                buf.push_str(sep);
                len += sep_len;
            }
            buf.push_str(&item);
            len += il;
        }
        if !buf.is_empty() {
            nodes.push(ForwardNode::text(user, name, buf));
        }
        nodes
    }

    /// 把一大段多行文本按**行**切成 ≤ `max_chars` 字的若干文本节点（[`Self::chunk_items`] 的便捷形：
    /// 条目 = 行、分隔 = 换行）。行本身不会被切开；但逻辑条目跨多行时，请直接用 [`Self::chunk_items`]
    /// 按条目切，以免一个条目被拆到两个节点。文本为空 ⇒ 空 `Vec`。
    pub fn chunk_text(
        user: impl Into<Uin>,
        name: impl Into<String>,
        text: impl Into<String>,
        max_chars: usize,
    ) -> Vec<ForwardNode> {
        let text = text.into();
        let lines: Vec<&str> = text.lines().collect();
        Self::chunk_items(user, name, lines, "\n", max_chars)
    }
}
