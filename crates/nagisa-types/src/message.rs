//! 统一消息体 [`Message`]（即 `Vec<Segment>`）、挂在切片上的查询/变换扩展 [`MessageExt`]、
//! 以及流式构造器 [`Msg`]。消息段类型见 [`segment`](crate::segment)。
use crate::id::{MessageId, Uin};
use crate::resource::Media;
use crate::segment::Segment;

/// 业务统一消息体：一串 [`Segment`]。不内嵌 CQ 字符串（CQ 编解码只在 OneBot 适配器内部）。
pub type Message = Vec<Segment>;

/// [`Message`] 的便捷扩展，实现在 `[Segment]` 上（查询 + 变换）。
pub trait MessageExt {
    /// 拼接所有文本段为纯文本（忽略非文本段）。
    fn extract_text(&self) -> String;
    /// 所有图片段的资源引用。
    fn images(&self) -> Vec<&Media>;
    /// 第一张图片。
    fn first_image(&self) -> Option<&Media>;
    /// 是否含图片。
    fn has_image(&self) -> bool;
    /// 所有表情段的 id。
    fn faces(&self) -> Vec<&str>;
    /// 是否含表情段（用于「无表情才触发」之类负向门控）。
    fn has_face(&self) -> bool;
    /// 若有回复段，返回被回复消息的内容段（供解析被引用消息的文本，
    /// 取决于 adapter 是否填充 `quoted`）。
    fn reply_quoted(&self) -> Option<&[Segment]>;
    /// 所有被 @ 的用户（不含 @全体）。
    fn mentions(&self) -> Vec<Uin>;
    /// 是否 @ 了全体成员。
    fn mentions_all(&self) -> bool;
    /// 是否 @ 了指定用户。
    fn mentions_user(&self, user: Uin) -> bool;
    /// 若首段是回复，返回被回复的消息 id。
    fn reply_to(&self) -> Option<&MessageId>;
    /// 是否纯文本（只含文本段，且非空）。
    fn is_text_only(&self) -> bool;
    /// 合并相邻文本段（其它段原样保留）。
    fn merge_text(&self) -> Message;
    /// 按分隔符切分提取出的纯文本（空段过滤）。
    fn split_text(&self, sep: char) -> Vec<String>;
    /// 若首个文本段以 `prefix` 开头，剥离之并返回新消息；否则 `None`。
    /// 用于在 matcher 之外做轻量前缀处理（保留非文本段，如图片/回复）。
    fn strip_text_prefix(&self, prefix: &str) -> Option<Message>;
}

impl MessageExt for [Segment] {
    fn extract_text(&self) -> String {
        self.iter().filter_map(Segment::as_text).collect()
    }

    fn faces(&self) -> Vec<&str> {
        self.iter()
            .filter_map(|s| match s {
                Segment::Face { id, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect()
    }

    fn has_face(&self) -> bool {
        self.iter().any(|s| matches!(s, Segment::Face { .. }))
    }

    fn reply_quoted(&self) -> Option<&[Segment]> {
        self.iter().find_map(|s| match s {
            Segment::Reply { quoted, .. } => Some(quoted.as_slice()),
            _ => None,
        })
    }

    fn images(&self) -> Vec<&Media> {
        self.iter()
            .filter_map(|s| match s {
                Segment::Image { res, .. } => Some(res),
                _ => None,
            })
            .collect()
    }

    fn first_image(&self) -> Option<&Media> {
        self.iter().find_map(|s| match s {
            Segment::Image { res, .. } => Some(res),
            _ => None,
        })
    }

    fn has_image(&self) -> bool {
        self.iter().any(|s| matches!(s, Segment::Image { .. }))
    }

    fn mentions(&self) -> Vec<Uin> {
        self.iter()
            .filter_map(|s| match s {
                Segment::Mention { user, .. } => Some(*user),
                _ => None,
            })
            .collect()
    }

    fn mentions_all(&self) -> bool {
        self.iter().any(|s| matches!(s, Segment::MentionAll))
    }

    fn mentions_user(&self, user: Uin) -> bool {
        self.iter()
            .any(|s| matches!(s, Segment::Mention { user: u, .. } if *u == user))
    }

    fn reply_to(&self) -> Option<&MessageId> {
        self.iter().find_map(|s| match s {
            Segment::Reply { id, .. } => Some(id),
            _ => None,
        })
    }

    fn is_text_only(&self) -> bool {
        !self.is_empty() && self.iter().all(|s| matches!(s, Segment::Text(_)))
    }

    fn merge_text(&self) -> Message {
        let mut out: Message = Vec::with_capacity(self.len());
        for seg in self {
            match (seg, out.last_mut()) {
                (Segment::Text(t), Some(Segment::Text(prev))) => prev.push_str(t),
                _ => out.push(seg.clone()),
            }
        }
        out
    }

    fn split_text(&self, sep: char) -> Vec<String> {
        self.extract_text()
            .split(sep)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    }

    fn strip_text_prefix(&self, prefix: &str) -> Option<Message> {
        let idx = self.iter().position(|s| matches!(s, Segment::Text(_)))?;
        let Segment::Text(t) = &self[idx] else { return None };
        let rest = t.strip_prefix(prefix)?;
        let mut out: Message = self.to_vec();
        if rest.is_empty() {
            out.remove(idx);
        } else {
            out[idx] = Segment::Text(rest.to_string());
        }
        Some(out)
    }
}

/// 流式构建一条消息：`Msg::new().reply(id).at(u).text(" hi").build()`。链式 setter 追加段，
/// `build()`（或 `Into<Message>`）收尾成 [`Message`]。
#[derive(Clone, Debug, Default)]
pub struct Msg(Message);

impl Msg {
    pub fn new() -> Self {
        Msg(Vec::new())
    }
    pub fn text(mut self, s: impl Into<String>) -> Self {
        self.0.push(Segment::Text(s.into()));
        self
    }
    pub fn at(mut self, user: impl Into<Uin>) -> Self {
        self.0.push(Segment::at(user));
        self
    }
    pub fn at_all(mut self) -> Self {
        self.0.push(Segment::MentionAll);
        self
    }
    pub fn face(mut self, id: impl Into<String>) -> Self {
        self.0.push(Segment::face(id));
        self
    }
    pub fn image_url(mut self, url: impl Into<String>) -> Self {
        self.0.push(Segment::image_url(url));
        self
    }
    /// 追加一张内存图片（PNG/GIF/… 字节）。与 [`image_url`](Self::image_url) 对称。
    pub fn image_bytes(mut self, bytes: impl Into<bytes::Bytes>) -> Self {
        self.0.push(Segment::image_bytes(bytes));
        self
    }
    /// 追加一张本地文件图片（与 `image_url`/`image_bytes` 三态对齐）。
    pub fn image_path(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.0.push(Segment::image_path(path));
        self
    }
    pub fn reply(mut self, id: MessageId) -> Self {
        self.0.push(Segment::reply(id));
        self
    }
    /// 追加任意段。
    pub fn push(mut self, seg: Segment) -> Self {
        self.0.push(seg);
        self
    }
    pub fn build(self) -> Message {
        self.0
    }
}

impl From<Msg> for Message {
    fn from(m: Msg) -> Message {
        m.0
    }
}
