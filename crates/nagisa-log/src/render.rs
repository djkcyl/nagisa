//! 把统一 [`Event`] 渲染成一行简洁、可读的中文日志。
//!
//! 渲染只读、无 IO:依赖 `nagisa-types` 域模型,可选地查 [`NameStore`] 把群号/QQ 号换成名字。
//! 每个事件变体映射到一行:前缀 `[…]` 标注种类(按种类着色),正文给出当事人/内容。
//!
//! 三件可调,经 [`RenderOpts`]:
//! - **着色**([`Style`]):`Ansi` 给种类前缀 / 发送者 / 各消息段套 ANSI 颜色,`Plain` 纯文本。
//!   叶子级着色(每片独立上色再拼接),不嵌套,故不会出现颜色被内层 reset 截断。
//! - **解析名字**([`RenderOpts::names`]):查得到就把 `123` 显示成 `名字(123)`,否则回落裸号。
//! - **换行不换行**:渲染出的整行里的 `\n`/`\r` 统一替换成可见的 `⏎`,使一条消息恒为一行日志。
//!
//! 一件不可调:名字(群名/昵称)显示超过 [`NAME_MAX_CHARS`] 字一律截断补 `…`——超长网名/群名
//! 不该把整行内容挤出屏幕。只影响日志显示,号码恒完整。
//!
//! 两个入口:[`render`] 是「纯文本、不查名字」的便捷形式(等价默认 [`RenderOpts`]),供日志总线
//! 与无名称缓存的场景直接调用;要着色 / 解析名字则填好 [`RenderOpts`] 走 [`render_line`]。
use crate::names::NameStore;
use nagisa_types::entity::{FriendInfo, MemberInfo};
use nagisa_types::event::{Event, FlashFilePhase, Meta, Notice, OnlineFileDirection, Request, RequestState};
use nagisa_types::id::{MessageId, Peer, Scene, Uin};
use nagisa_types::resource::Media;
use nagisa_types::segment::{ContactKind, Forward, MusicShare, Segment};

// ── ANSI 调色板（语义命名，便于统一调整）──────────────────────────────────────
// 设计口径(与行格式器的 loguru 框架色「时间绿 / 来源青」配套):
// 视觉层级 = 彩色场景标签 → **粗体**人名 → 暗灰号码 → 默认色正文 + 彩色段落小岛。
// 入站场景按色相区分(群蓝/私聊品红),出站 [发送] 粗绿;通知黄、请求粗黄、元事件暗灰。
const C_SEND: &str = "1;32"; // 出站消息标签：粗绿（一眼区分收发）
const C_GROUP: &str = "34"; // 群消息前缀：蓝
const C_PRIV: &str = "35"; // 私聊/临时前缀：品红
const C_NOTICE: &str = "33"; // 通知类标签：黄
const C_REQUEST: &str = "1;33"; // 请求类标签：粗黄（比通知重——通常要人工处置）
const C_META: &str = "90"; // 元事件：灰（暗）
const C_NAME: &str = "1"; // 发送者名：粗体默认色（任何终端配色下都醒目、不添色噪）
const C_ID: &str = "90"; // 各种号：灰（暗）
const C_AT: &str = "36"; // @提及：青
const C_MEDIA: &str = "35"; // 图片/视频/语音/文件：品红
const C_FACE: &str = "33"; // 表情/商城表情：黄
const C_LINK: &str = "34"; // 链接/音乐/位置/分享：蓝
const C_MISC: &str = "90"; // 其余占位段/引用：灰（暗）

/// 名字(群名/昵称)的显示上限(字符数)。超过则截断补 `…`——超长网名/群名每行重复,会把
/// 真正的内容挤出屏幕。**只影响日志显示,不动数据**;号码恒完整保留(名字会重名,号才是锚点)。
const NAME_MAX_CHARS: usize = 12;

/// 把名字截到 [`NAME_MAX_CHARS`] 字,超出补 `…`。按字符计数,不劈 UTF-8;全角字占两个
/// 终端列,12 个 CJK 字约 24 列——「可读的名字 vs 不挤正文」的折中,不追求按列宽精确对齐。
fn clip_name(s: &str) -> String {
    truncate_preview(s, NAME_MAX_CHARS)
}

/// 着色风格:给渲染片段套 ANSI 颜色,或纯文本。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    /// 纯文本,不加任何 ANSI。
    Plain,
    /// 加 ANSI 颜色（终端可读）。
    Ansi,
}

impl Style {
    /// 给 `s` 套上颜色 `code`(如 `"32"`/`"1;36"`);`Plain` 直接原样返回。
    fn paint(self, code: &str, s: &str) -> String {
        match self {
            Style::Plain => s.to_string(),
            Style::Ansi => format!("\x1b[{code}m{s}\x1b[0m"),
        }
    }
}

/// 渲染选项:着色风格 + 可选名称缓存 + 当前群上下文。
#[derive(Clone, Copy)]
pub struct RenderOpts<'a> {
    /// 着色风格。
    pub style: Style,
    /// 名称缓存:查得到则把号显示成 `名字(号)`。`None` 则一律裸号。
    pub names: Option<&'a NameStore>,
    /// 当前事件的群上下文（群作用域事件填群号）。用它解析人名时优先取**该群**的群名片
    /// （[`NameStore::member`]）、回落到全局昵称;`None`（私聊/元事件）则只用全局昵称。
    /// 由 [`render_line`] 自事件的 peer 自动填,调用方一般无需手动设。
    pub group: Option<i64>,
    /// 最近消息缓存:撤回通知据此显示「撤回了什么」。`None` 则只显示「撤回了一条消息」。
    pub messages: Option<&'a crate::messages::MessageStore>,
}

impl RenderOpts<'_> {
    /// 纯文本、不查名字(便捷 [`render`] 用的这套选项)。
    pub const fn plain() -> Self {
        RenderOpts { style: Style::Plain, names: None, group: None, messages: None }
    }
}

/// 把单个事件渲染成一行**纯文本**可读中文(不查名字)。等价 `render_line(ev, &RenderOpts::plain())`。
///
/// 示例:
/// ```text
/// [群 123] 张三(456): 你好 @789 [图片]
/// [撤回] 456 撤回了一条消息
/// ```
pub fn render(event: &Event) -> String {
    render_line(event, &RenderOpts::plain())
}

/// 把单个事件渲染成一行可读中文,按 `opts` 着色 / 解析名字 / 折叠换行。
pub fn render_line(event: &Event, opts: &RenderOpts) -> String {
    // 自事件的 peer 填群上下文:群作用域事件里解析人名优先取该群的群名片(见 RenderOpts::group)。
    let group = event.peer().filter(|p| p.scene == Scene::Group).map(|p| p.id.0);
    let opts = &RenderOpts { group, ..*opts };
    let line = match event {
        Event::Message(m) => render_message(m, opts),
        Event::Notice(n) => render_notice(n, opts),
        Event::Request(r) => render_request(r, opts),
        Event::Meta(meta) => render_meta(meta, opts),
        Event::Raw(raw) => opts.style.paint(C_META, &format!("[原始] {:?} {}", raw.protocol, raw.kind)),
        // `Event` 为 #[non_exhaustive]：未知未来变体降级，绝不 panic。
        _ => opts.style.paint(C_META, "[未知事件]"),
    };
    // 折叠换行:整行里的 \n/\r 换成可见 ⏎,使一条消息恒为一行日志。
    fold_newlines(&line)
}

/// 按**字符**(非字节)截断到 `max_chars`,超出补 `…`。用于名字与引用预览,避免把日志撑长。
///
/// 截口落在 ZWJ 拼接序列中间时回退掉尾部悬空的零宽连接符(U+200D)——它只该夹在两个字形
/// 之间,裸露在结尾会渲染成残缺字形。组合 emoji 仍可能被截成「序列的第一个成员 + `…`」
/// (如全家福截成单人),这是按字符截断的固有折中;为日志显示引入 grapheme 分词依赖不值。
fn truncate_preview(s: &str, max_chars: usize) -> String {
    let mut it = s.chars();
    let mut head: String = it.by_ref().take(max_chars).collect();
    if it.next().is_none() {
        return head;
    }
    while head.ends_with('\u{200D}') {
        head.pop();
    }
    format!("{head}…")
}

/// 剥掉一层外层方括号(ASCII `[]` 或中文 `【】`),用于把 QQ 给媒体塞的占位 summary(`[图片]`)
/// 还原成类型词(`图片`)。非包裹形态原样返回。
fn strip_brackets(s: &str) -> &str {
    let t = s.trim();
    t.strip_prefix('[')
        .and_then(|x| x.strip_suffix(']'))
        .or_else(|| t.strip_prefix('【').and_then(|x| x.strip_suffix('】')))
        .map(str::trim)
        .unwrap_or(t)
}

/// 图片/动画表情的占位标签:`[类型 说明? 文件名?]`。
/// - **类型词**:summary 若是带方括号的占位(`[动画表情]`/`[图片]`)→ 去括号取类型词;否则一律「图片」。
/// - **说明**:summary 若是**不带方括号**的描述(如 QQ 给图片的「喵喵喵」)→ 作说明附上(截断)。
/// - **文件名**:wire 的 `filename`(完整 `{MD5}.ext`,可用于匹配/去重/对照),回落到非 URL 形态的
///   `recv.id` 的文件名部分。`recv.id` 多为带 rkey 的下载 URL(re-send 要用、但不适合展示),
///   `media_file_name` 会把它判成残渣返 `None`,故不会拿 URL 当文件名。
fn image_label(res: &Media) -> String {
    let summary = res.summary.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let (kind, caption) = match summary {
        Some(s) if is_bracketed(s) => {
            let k = strip_brackets(s).trim();
            ((if k.is_empty() { "图片" } else { k }).to_string(), None)
        }
        Some(s) => ("图片".to_string(), Some(truncate_preview(s, 16))),
        None => ("图片".to_string(), None),
    };
    let mut out = format!("[{kind}");
    if let Some(c) = caption {
        out.push(' ');
        out.push_str(&c);
    }
    if let Some(name) = media_file_name(res) {
        out.push(' ');
        out.push_str(&name);
    }
    out.push(']');
    out
}

/// summary 是否是「带方括号的占位」(`[动画表情]` / `【…】`)——是则视作类型词,否则视作描述文本。
fn is_bracketed(s: &str) -> bool {
    let t = s.trim();
    (t.starts_with('[') && t.ends_with(']') && t.chars().count() >= 2)
        || (t.starts_with('【') && t.ends_with('】') && t.chars().count() >= 2)
}

/// 媒体的展示用**完整文件名**:优先 wire 的 `filename`(干净 `{MD5}.ext`),回落到非 URL 形态的
/// `recv.id` 的文件名部分;都不是干净文件名(如带 rkey 的下载 URL)则 `None`。
fn media_file_name(res: &Media) -> Option<String> {
    let recv = res.recv.as_ref()?;
    [recv.raw.get("filename").and_then(|v| v.as_str()), recv.id.as_deref()]
        .into_iter()
        .flatten()
        .find_map(clean_file_name)
}

/// 把候选(文件名 / 本地 path / 下载 URL)清成可展示的完整文件名:取最后一段路径、保留扩展名。
/// URL 查询残渣(含 `?`/`=`/`&` 或就是 `download`)不是干净文件名 → `None`。
fn clean_file_name(raw: &str) -> Option<String> {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw).trim();
    if base.is_empty() || base.contains(['?', '=', '&']) || base.eq_ignore_ascii_case("download") {
        return None;
    }
    Some(base.to_string())
}

/// 把字符串里的换行折成可见标记,使其保持单行。
fn fold_newlines(s: &str) -> String {
    if s.contains('\n') || s.contains('\r') {
        s.replace("\r\n", "\n").replace(['\n', '\r'], " ⏎ ")
    } else {
        s.to_string()
    }
}

/// 消息的简短引用 id:OneBot 的 `message_id`(`onebot_id`,撤回/回复/get_msg 的锚点)优先,
/// 否则 Milky 的 `seq`。日志给每条消息标上它,引用处(回复/撤回)只标这个 id 即可——可在日志里
/// ctrl-F 找到原消息那行,不必重复整段内容。
fn msg_ref(id: &MessageId) -> Option<String> {
    id.onebot_id.map(|v| v.to_string()).or_else(|| (id.seq != 0).then(|| id.seq.to_string()))
}

/// 群号 → `名字(号)` 或裸号(查不到)。名字超长截断(见 [`NAME_MAX_CHARS`])。
fn group_label(gid: i64, opts: &RenderOpts) -> String {
    match opts.names.and_then(|n| n.group(gid)) {
        Some(name) => format!("{}({gid})", clip_name(&name)),
        None => gid.to_string(),
    }
}

/// 在当前群上下文里解析一个人的显示名:有群上下文优先取该群的群名片([`NameStore::member`]),
/// 否则全局昵称([`NameStore::user`])。查不到 `None`。
fn resolve_user(uin: i64, opts: &RenderOpts) -> Option<String> {
    let names = opts.names?;
    match opts.group {
        Some(g) => names.member(g, uin),
        None => names.user(uin),
    }
}

/// QQ 号 → `名字(号)` 或裸号(查不到)。名字按当前群上下文解析(见 [`resolve_user`])、
/// 超长截断(见 [`NAME_MAX_CHARS`])。
fn user_label(uin: i64, opts: &RenderOpts) -> String {
    match resolve_user(uin, opts) {
        Some(name) => format!("{}({uin})", clip_name(&name)),
        None => uin.to_string(),
    }
}

/// 渲染一条消息事件：场景前缀 + 发送者（带昵称） + 内容。
fn render_message(m: &nagisa_types::event::MessageEvent, opts: &RenderOpts) -> String {
    // bot 出站消息(is_self):没有发送者(就是 bot),以 `[发送]` 标出方向,见 render_self_message。
    if m.is_self {
        return render_self_message(m, opts);
    }
    let prefix = scene_prefix(&m.peer, opts);
    let sender = render_sender(m.sender, m.member.as_ref(), m.friend.as_ref(), opts);
    let body = render_segments_styled(&m.content, opts);
    // 给每条消息标上自己的 id(暗色),供回复/撤回等引用处指回——ctrl-F 即定位原消息。
    match msg_ref(&m.id) {
        Some(r) => {
            let tag = opts.style.paint(C_ID, &format!("#{r}"));
            format!("{prefix} {sender} {tag}: {body}")
        }
        None => format!("{prefix} {sender}: {body}"),
    }
}

/// 渲染一条 **bot 出站**消息:`[发送] 群/私聊 名(id) #msgid: 内容`。与入站走同一 `render_message`
/// 入口(经 `is_self` 分流),复用 scene/名字解析/段渲染——只是没有发送者(就是 bot),并以 `[发送]`
/// 粗绿标签标出方向;`#msgid` 供与撤回/回复对照。群上下文已由 `render_line` 自 peer 填好。
fn render_self_message(m: &nagisa_types::event::MessageEvent, opts: &RenderOpts) -> String {
    let tag = opts.style.paint(C_SEND, "[发送]");
    let dest = match m.peer.scene {
        Scene::Group => format!("群 {}", group_label(m.peer.id.0, opts)),
        Scene::Friend => format!("私聊 {}", user_label(m.peer.id.0, opts)),
        Scene::Temp => format!("临时 {}", m.peer.id),
    };
    let body = render_segments_styled(&m.content, opts);
    match msg_ref(&m.id) {
        Some(r) => {
            let id_tag = opts.style.paint(C_ID, &format!("#{r}"));
            format!("{tag} {dest} {id_tag}: {body}")
        }
        None => format!("{tag} {dest}: {body}"),
    }
}

/// 场景前缀(按种类着色)：群给群号(可解析群名)，私聊/临时各自标注。
fn scene_prefix(peer: &Peer, opts: &RenderOpts) -> String {
    match peer.scene {
        Scene::Group => opts.style.paint(C_GROUP, &format!("[群 {}]", group_label(peer.id.0, opts))),
        Scene::Friend => opts.style.paint(C_PRIV, "[私聊]"),
        Scene::Temp => opts.style.paint(C_PRIV, &format!("[临时 {}]", peer.id)),
    }
}

/// 发送者：优先群名片/群昵称、其次好友昵称、再次缓存学到的名;名着色、`(号)` 暗色。
fn render_sender(sender: Uin, member: Option<&MemberInfo>, friend: Option<&FriendInfo>, opts: &RenderOpts) -> String {
    // 优先群名片/群昵称([`MemberInfo::display_name`]),其次好友备注/昵称
    // ([`FriendInfo::display_name`]),再次缓存学到的名;空串视作无名,回落下一级。
    let name = member
        .and_then(|m| non_empty(m.display_name()))
        .or_else(|| friend.and_then(|f| non_empty(f.display_name())))
        .or_else(|| resolve_user(sender.0, opts));
    match name {
        Some(name) => {
            format!("{}{}", opts.style.paint(C_NAME, &clip_name(&name)), opts.style.paint(C_ID, &format!("({sender})")))
        }
        None => opts.style.paint(C_ID, &sender.to_string()),
    }
}

/// 空串视作「无名」→ `None`;非空则拥有化返回。
fn non_empty(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_string())
}

/// 把消息段序列拼成一行**纯文本**(不着色、不查名字)。供只想渲染一段内容(而非整个事件)的
/// 场景直接调用。
pub fn render_segments(segments: &[Segment]) -> String {
    render_segments_styled(segments, &RenderOpts::plain())
}

fn render_segments_styled(segments: &[Segment], opts: &RenderOpts) -> String {
    let mut out = String::new();
    // QQ 给商城表情消息额外塞一段「向后兼容兜底文本」(独立 text 段,内容 = caption),协议端
    // (Lagrange/LLOneBot 的 mface 段把 caption 放在 summary 字段里,但 QQ 那段独立 text 仍被
    // 如实透传)→ 直接渲染就重复,如 `[商城表情 喵喵喵]喵喵喵`。这里记住上一段商城表情的 summary,
    // 跳过**紧随其后、与之完全相同**的那段冗余文本(只去重,不改消息——纯显示层)。
    let mut market_summary: Option<&str> = None;
    for seg in segments {
        if let Segment::Text(t) = seg {
            if market_summary.is_some_and(|s| s.trim() == t.trim() && !s.trim().is_empty()) {
                market_summary = None; // 消费掉,避免误连跳后续文本
                continue;
            }
        }
        out.push_str(&render_segment(seg, opts));
        market_summary = match seg {
            Segment::MarketFace { summary: Some(s), .. } if !s.trim().is_empty() => Some(s),
            _ => None,
        };
    }
    out
}

/// 渲染单个消息段(按段类型着色)。文本原样透出(默认色);其余段用 `[…]` 占位并按类着色。
fn render_segment(seg: &Segment, opts: &RenderOpts) -> String {
    let s = opts.style;
    match seg {
        Segment::Text(t) => t.clone(),
        Segment::Mention { user, name } => {
            // 优先用缓存里的群名片(当前群上下文),其次回落到 wire 自带的 name(解码层已归一化成裸名),
            // 再不行就裸号。补**一个** @——name 在解码层已剥掉前导 @,故这里不会出现「@@」。
            let label = match resolve_user(user.0, opts) {
                Some(cached) => format!("@{}", clip_name(&cached)),
                None => match name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
                    Some(n) => format!("@{}", clip_name(n)),
                    None => format!("@{user}"),
                },
            };
            s.paint(C_AT, &label)
        }
        Segment::MentionAll => s.paint(C_AT, "@全体成员"),
        Segment::Face { id, .. } => {
            let id = id.trim();
            let label = if id.is_empty() { "[表情]".to_string() } else { format!("[表情 {id}]") };
            s.paint(C_FACE, &label)
        }
        Segment::Reply { id, sender, .. } => {
            // 被回复者:优先段自带 sender,Lagrange 的 reply 段无 sender → 按 id 查最近消息缓存补名字。
            // 内容不再整段预览,改标被回复消息的 #id——用户可 ctrl-F 定位原消息那行(免重复整段内容)。
            let target = sender.map(|u| u.0).or_else(|| opts.messages.and_then(|m| m.get(id)).map(|c| c.sender.0));
            let mut inner = String::from("回复");
            if let Some(uin) = target {
                inner.push_str(&format!(
                    " @{}",
                    resolve_user(uin, opts).map(|n| clip_name(&n)).unwrap_or_else(|| uin.to_string())
                ));
            }
            if let Some(r) = msg_ref(id) {
                inner.push_str(&format!(" #{r}"));
            }
            s.paint(C_MISC, &format!("[{inner}]"))
        }
        Segment::Image { res, .. } => s.paint(C_MEDIA, &image_label(res)),
        Segment::Record { .. } => s.paint(C_MEDIA, "[语音]"),
        Segment::Video { .. } => s.paint(C_MEDIA, "[视频]"),
        Segment::File { name, .. } => s.paint(C_MEDIA, &format!("[文件 {name}]")),
        Segment::Forward(f) => match f {
            Forward::Ref { .. } | Forward::Nodes { .. } => s.paint(C_LINK, "[合并转发]"),
        },
        Segment::MarketFace { package_id, emoji_id, summary, .. } => {
            // 优先用从 gtimg 表情包列表反查到的**规范名**(`包名·表情名`,不受发送方改写);否则
            // 回落到 summary(可能被改写的描述);再不行只标占位。
            let name = opts.names.and_then(|n| n.market_face_display(*package_id, emoji_id)).or_else(|| {
                // summary 多是带方括号的占位(`[疑问]`)→ 去括号,免得 `[商城表情 [疑问]]`。
                summary.as_deref().map(strip_brackets).map(str::trim).filter(|x| !x.is_empty()).map(str::to_string)
            });
            let label = match name {
                Some(n) => format!("[商城表情 {n}]"),
                None => "[商城表情]".to_string(),
            };
            s.paint(C_FACE, &label)
        }
        Segment::LightApp { .. } => s.paint(C_MISC, "[小程序]"),
        Segment::Xml { .. } => s.paint(C_MISC, "[XML卡片]"),
        Segment::Poke { .. } => s.paint(C_FACE, "[戳一戳]"),
        Segment::Contact { kind, id } => {
            let label = match kind {
                ContactKind::Friend => format!("[推荐好友 {id}]"),
                ContactKind::Group => format!("[推荐群 {id}]"),
            };
            s.paint(C_LINK, &label)
        }
        Segment::Location { title, .. } => {
            let label = match title {
                Some(t) if !t.trim().is_empty() => format!("[位置 {t}]"),
                _ => "[位置]".to_string(),
            };
            s.paint(C_LINK, &label)
        }
        Segment::Music(m) => {
            let label = match m {
                MusicShare::Platform { ty, .. } => format!("[音乐分享 {ty}]"),
                MusicShare::Custom { title, .. } => format!("[音乐分享 {title}]"),
            };
            s.paint(C_LINK, &label)
        }
        Segment::Share { title, .. } => s.paint(C_LINK, &format!("[链接 {title}]")),
        Segment::Rps { .. } => s.paint(C_FACE, "[猜拳]"),
        Segment::Dice { .. } => s.paint(C_FACE, "[骰子]"),
        Segment::Shake => s.paint(C_FACE, "[窗口抖动]"),
        Segment::Anonymous { .. } => s.paint(C_MISC, "[匿名]"),
        Segment::Keyboard { .. } => s.paint(C_MISC, "[键盘]"),
        Segment::Markdown { .. } => s.paint(C_MISC, "[Markdown]"),
        Segment::LongMsg { .. } => s.paint(C_MISC, "[长消息]"),
        Segment::FlashFile { .. } | Segment::FlashTransfer { .. } => s.paint(C_MEDIA, "[闪传]"),
        Segment::MiniApp { .. } => s.paint(C_MISC, "[小程序]"),
        Segment::OnlineFile { file_name, .. } => s.paint(C_MEDIA, &format!("[在线文件 {file_name}]")),
        Segment::Raw { kind, .. } => s.paint(C_MISC, &format!("[{kind}]")),
        // `Segment` 为 #[non_exhaustive]：未知未来段降级为占位，绝不 panic。
        _ => s.paint(C_MISC, "[未知段]"),
    }
}

/// 通知类:统一标签着黄、群号/QQ 号尽量解析成名字。
fn render_notice(n: &Notice, opts: &RenderOpts) -> String {
    let tag = |t: &str| opts.style.paint(C_NOTICE, t);
    let u = |uin: Uin| user_label(uin.0, opts);
    let g = |gid: Uin| group_label(gid.0, opts);
    match n {
        Notice::Recall { id, sender, operator, .. } => {
            // 撤回标上被撤消息的 #id(指回原消息那行);内容预览保留(防撤回:被撤内容最该看见)。
            let label = match msg_ref(id) {
                Some(r) => format!("[撤回 #{r}]"),
                None => "[撤回]".to_string(),
            };
            let base = if sender == operator {
                format!("{} {} 撤回了一条消息", tag(&label), u(*sender))
            } else {
                format!("{} {} 撤回了 {} 的一条消息", tag(&label), u(*operator), u(*sender))
            };
            // 撤回通知只带被撤消息 id;若最近消息缓存里还留着,附上被撤内容预览。
            match opts.messages.and_then(|store| store.get(id)) {
                Some(msg) => {
                    let preview = render_segments_styled(&msg.content, &RenderOpts { style: Style::Plain, ..*opts });
                    let preview = preview.trim();
                    if preview.is_empty() {
                        base
                    } else {
                        format!("{base}: {}", truncate_preview(preview, 30))
                    }
                }
                None => base,
            }
        }
        Notice::MemberIncrease { group, user, operator, invitor } => {
            let mut s = format!("{} {} 加入群 {}", tag("[进群]"), u(*user), g(*group));
            if let Some(inv) = invitor {
                s.push_str(&format!("（{} 邀请）", u(*inv)));
            } else if let Some(op) = operator {
                s.push_str(&format!("（{} 操作）", u(*op)));
            }
            s
        }
        Notice::MemberDecrease { group, user, operator, reason } => {
            use nagisa_types::event::MemberDecreaseReason as R;
            let how = match reason {
                R::Leave => "退出",
                R::Kick | R::KickMe => "被踢出",
                R::Disband => "因群解散离开",
                R::Unknown => "离开",
            };
            match operator {
                Some(op) if matches!(reason, R::Kick | R::KickMe) => {
                    format!("{} {} 将 {} {how}群 {}", tag("[退群]"), u(*op), u(*user), g(*group))
                }
                _ => format!("{} {} {how}群 {}", tag("[退群]"), u(*user), g(*group)),
            }
        }
        Notice::AdminChange { group, user, is_set, .. } => {
            let verb = if *is_set { "被设为" } else { "被取消" };
            format!("{} {} 在群 {} {verb}管理员", tag("[管理变更]"), u(*user), g(*group))
        }
        Notice::Mute { group, user, operator, duration } => {
            format!("{} {} 禁言 {} {duration}s（群 {}）", tag("[禁言]"), u(*operator), u(*user), g(*group))
        }
        Notice::WholeMute { group, operator, is_mute } => {
            let verb = if *is_mute { "开启" } else { "关闭" };
            format!("{} {} {verb}群 {} 全员禁言", tag("[全员禁言]"), u(*operator), g(*group))
        }
        Notice::GroupNameChange { group, new_name, operator } => {
            format!("{} {} 把群 {} 改名为 {new_name}", tag("[群名变更]"), u(*operator), g(*group))
        }
        Notice::LuckyKing { group, user, target } => {
            format!("{} 群 {} {} 的红包，运气王是 {}", tag("[运气王]"), g(*group), u(*user), u(*target))
        }
        Notice::Honor { group, user, honor } => {
            use nagisa_types::event::HonorKind as H;
            let h = match honor {
                H::Talkative => "龙王",
                H::Performer => "群聊之火",
                H::Emotion => "快乐源泉",
                H::Unknown => "荣誉",
            };
            format!("{} 群 {} {} 获得 {h}", tag("[荣誉]"), g(*group), u(*user))
        }
        Notice::GroupCardChange { group, user, new_card, .. } => {
            format!("{} 群 {} {} 改名片为 {new_card}", tag("[名片变更]"), g(*group), u(*user))
        }
        Notice::FriendNudge { user, display, .. } => {
            format!("{} {} {}你{}", tag("[戳一戳]"), u(*user), display.action, display.suffix)
        }
        Notice::GroupNudge { group, sender, receiver, display } => format!(
            "{} 群 {} {} {}了 {}{}",
            tag("[戳一戳]"),
            g(*group),
            u(*sender),
            display.action,
            u(*receiver),
            display.suffix
        ),
        Notice::Reaction { group, user, face_id, is_add, .. } => {
            let verb = if *is_add { "回应" } else { "取消回应" };
            format!("{} 群 {} {} {verb} {face_id}", tag("[表情回应]"), g(*group), u(*user))
        }
        Notice::EssenceChange { group, sender, is_set, .. } => {
            let verb = if *is_set { "设为" } else { "取消" };
            match sender {
                Some(s) => format!("{} 群 {} {verb}精华（作者 {}）", tag("[精华]"), g(*group), u(*s)),
                None => format!("{} 群 {} {verb}精华", tag("[精华]"), g(*group)),
            }
        }
        Notice::GroupFileUpload { group, user, file } => {
            format!("{} 群 {} {} 上传 {}", tag("[群文件]"), g(*group), u(*user), file.name)
        }
        Notice::FriendFileUpload { user, file, .. } => {
            format!("{} {} 发送 {}", tag("[私聊文件]"), u(*user), file.name)
        }
        Notice::FriendAdd { user } => format!("{} 已添加好友 {}", tag("[好友添加]"), u(*user)),
        Notice::PeerPinChange { peer, is_pinned } => {
            let verb = if *is_pinned { "置顶" } else { "取消置顶" };
            format!("{} {}{verb}", tag("[置顶变更]"), scene_prefix(peer, opts))
        }
        Notice::BotOffline { reason, .. } => format!("{} {reason}", tag("[下线]")),
        Notice::GroupDismiss { group, operator } => {
            format!("{} {} 解散了群 {}", tag("[群解散]"), u(*operator), g(*group))
        }
        Notice::GroupTitleChange { group, user, title } => {
            format!("{} 群 {} {} 头衔变为 {title}", tag("[头衔变更]"), g(*group), u(*user))
        }
        Notice::InputStatus { user, group, status_text, .. } => match group {
            Some(grp) => format!("{} 群 {} {} {status_text}", tag("[输入状态]"), g(*grp), u(*user)),
            None => format!("{} {} {status_text}", tag("[输入状态]"), u(*user)),
        },
        Notice::ProfileLike { operator, operator_nick, times } => {
            let who = if operator_nick.trim().is_empty() {
                u(*operator)
            } else {
                format!("{}({operator})", clip_name(operator_nick.trim()))
            };
            format!("{} {who} 点赞 {times} 次", tag("[资料点赞]"))
        }
        Notice::GrayTip { group, content, .. } => match group {
            Some(grp) => format!("{} 群 {} {content}", tag("[灰字提示]"), g(*grp)),
            None => format!("{} {content}", tag("[灰字提示]")),
        },
        Notice::PokeRecall { group, user } => match group {
            Some(grp) => format!("{} 群 {} {} 撤回了戳一戳", tag("[戳一戳撤回]"), g(*grp), u(*user)),
            None => format!("{} {} 撤回了戳一戳", tag("[戳一戳撤回]"), u(*user)),
        },
        Notice::OnlineFile { direction, user, group } => {
            let dir = match direction {
                OnlineFileDirection::Send => "发送",
                OnlineFileDirection::Receive => "接收",
            };
            match group {
                Some(grp) => {
                    format!("{} 群 {} {} {dir}在线文件", tag("[在线文件]"), g(*grp), u(*user))
                }
                None => format!("{} {} {dir}在线文件", tag("[在线文件]"), u(*user)),
            }
        }
        Notice::FlashFile { phase, user, group } => {
            let p = match phase {
                FlashFilePhase::Downloading => "下载中",
                FlashFilePhase::Downloaded => "下载完成",
                FlashFilePhase::Uploading => "上传中",
                FlashFilePhase::Uploaded => "上传完成",
                FlashFilePhase::Unknown => "进行中",
            };
            match group {
                Some(grp) => format!("{} 群 {} {} 闪传{p}", tag("[闪传]"), g(*grp), u(*user)),
                None => format!("{} {} 闪传{p}", tag("[闪传]"), u(*user)),
            }
        }
        Notice::Other { protocol, kind, .. } => format!("{} {protocol:?} {kind}", tag("[通知]")),
        // `Notice` 为 #[non_exhaustive]：未知未来通知降级，绝不 panic。
        _ => tag("[通知]"),
    }
}

fn render_request(r: &Request, opts: &RenderOpts) -> String {
    let tag = |t: &str| opts.style.paint(C_REQUEST, t);
    let u = |uin: Uin| user_label(uin.0, opts);
    let g = |gid: Uin| group_label(gid.0, opts);
    match r {
        Request::Friend { initiator, comment, state, .. } => {
            let mut s = format!("{} {}: {comment}", tag("[好友请求]"), u(*initiator));
            if let Some(t) = request_state_tag(*state) {
                s.push_str(&format!("（{t}）"));
            }
            s
        }
        Request::GroupJoin { group, initiator, comment, invitor, .. } => {
            let mut s = format!("{} {} → 群 {}: {comment}", tag("[加群请求]"), u(*initiator), g(*group));
            if let Some(inv) = invitor {
                s.push_str(&format!("（邀请人 {}）", u(*inv)));
            }
            s
        }
        Request::GroupInvitedJoin { group, initiator, target, .. } => {
            format!("{} {} 邀请 {} 加入群 {}", tag("[他人入群邀请]"), u(*initiator), u(*target), g(*group))
        }
        Request::GroupInvite { group, initiator, comment, .. } => {
            format!("{} {} 邀请你加入群 {}: {comment}", tag("[入群邀请]"), u(*initiator), g(*group))
        }
        // `Request` 为 #[non_exhaustive]：未知未来请求降级，绝不 panic。
        _ => tag("[请求]"),
    }
}

fn request_state_tag(state: RequestState) -> Option<&'static str> {
    match state {
        RequestState::Pending => None,
        RequestState::Accepted => Some("已同意"),
        RequestState::Rejected => Some("已拒绝"),
        RequestState::Ignored => Some("已忽略"),
        RequestState::Unknown => None,
    }
}

fn render_meta(meta: &Meta, opts: &RenderOpts) -> String {
    let m = |t: &str| opts.style.paint(C_META, t);
    match meta {
        Meta::Connect => m("[元] 协议端连接"),
        Meta::Disconnect { reason } => match reason {
            Some(r) if !r.trim().is_empty() => m(&format!("[元] 协议端断开（{r}）")),
            _ => m("[元] 协议端断开"),
        },
        Meta::Ready { self_id, nickname } => {
            let acct = if nickname.trim().is_empty() {
                self_id.0.to_string()
            } else {
                format!("{}({})", clip_name(nickname.trim()), self_id.0)
            };
            m(&format!("[元] 就绪（账号 {acct}）"))
        }
        Meta::Heartbeat { interval, .. } => m(&format!("[元] 心跳 {interval}ms")),
        Meta::BotOnline { reason } => match reason {
            Some(r) if !r.trim().is_empty() => m(&format!("[元] 账号上线（{r}）")),
            _ => m("[元] 账号上线"),
        },
        Meta::BotOffline => m("[元] 账号下线"),
        // `Meta` 为 #[non_exhaustive]：未知未来元事件降级，绝不 panic。
        _ => m("[元]"),
    }
}
