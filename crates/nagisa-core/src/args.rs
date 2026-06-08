//! 声明式命令参数解析:`#[derive(Args)]` + `Args<T>` 提取器,**在段流上有序解析**。
//!
//! 命令头(由匹配器消费)之后的剩余消息段被切成一个 **token 流**——
//! 文本段切成 `Word` token,非文本段(图片/@/回复/表情…)保留成 `Element` token,**顺序不变**。
//! 于是位置参数既能是文本(`from: String`)也能是元素(`#[arg(image)] pic: Media`),
//! 还支持 `--opt v` / `-x` 旗标。元素**必填字段缺失 → `Skip`(命令不触发)**;
//! `Option<T>` = 可选;`#[arg(rest)]` 收尾。
use crate::ctx::Ctx;
use crate::extract::{Extracted, FromContext, Reject};
use crate::matcher::ParsedCommand;
use async_trait::async_trait;
use nagisa_types::id::{MessageId, Uin};
use nagisa_types::resource::Media;
use nagisa_types::segment::Segment;

/// 参数解析错误(由生成代码产出,提取器据此 `Skip`)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgError {
    /// 缺少必填参数 `field`(文本或元素)。
    Missing(&'static str),
    /// `field` 的值 `value` 无法解析为 `expected` 类型。
    Parse { field: &'static str, value: String, expected: &'static str },
}

impl std::fmt::Display for ArgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArgError::Missing(field) => write!(f, "missing required argument `{field}`"),
            ArgError::Parse { field, value, expected } => {
                write!(f, "argument `{field}`: cannot parse {value:?} as {expected}")
            }
        }
    }
}
impl std::error::Error for ArgError {}

/// 把单个文本 token 解析成一个字段值。`#[derive(Args)]` 的文本字段类型需实现它。
pub trait FromArg: Sized {
    /// 期望类型的人读名(用于报错)。
    const TYPE_NAME: &'static str;
    fn from_arg(s: &str) -> Option<Self>;
}

macro_rules! from_arg_via_fromstr {
    ($($t:ty => $name:literal),* $(,)?) => {$(
        impl FromArg for $t {
            const TYPE_NAME: &'static str = $name;
            fn from_arg(s: &str) -> Option<Self> { s.parse().ok() }
        }
    )*};
}
from_arg_via_fromstr! {
    String => "string", i64 => "int", i32 => "int", i16 => "int", i8 => "int",
    u64 => "uint", u32 => "uint", u16 => "uint", u8 => "uint",
    f64 => "number", bool => "bool",
}

impl FromArg for Uin {
    const TYPE_NAME: &'static str = "uin";
    fn from_arg(s: &str) -> Option<Self> {
        // 兼容 "@123" / "123"。
        s.trim_start_matches('@').parse::<i64>().ok().map(Uin)
    }
}

/// 参数 token:文本词 或 一个非文本消息段。
#[derive(Clone, Copy, Debug)]
pub enum ArgToken<'a> {
    Word(&'a str),
    Element(&'a Segment),
}

/// 把剩余消息段切成 token 流:文本段按空白切词,非文本段各成一个 `Element` token,顺序保留。
pub fn tokenize_segments(args: &[Segment]) -> Vec<ArgToken<'_>> {
    let mut out = Vec::new();
    for seg in args {
        match seg {
            Segment::Text(t) => out.extend(t.split_whitespace().map(ArgToken::Word)),
            other => out.push(ArgToken::Element(other)),
        }
    }
    out
}

// —— 元素 → 字段值 抽取(供 `#[arg(image/at/reply/face/record/video)]` 生成代码调用)。——
pub fn seg_as_image(seg: &Segment) -> Option<Media> {
    match seg {
        Segment::Image { res, .. } => Some(res.clone()),
        _ => None,
    }
}
pub fn seg_as_record(seg: &Segment) -> Option<Media> {
    match seg {
        Segment::Record { res, .. } => Some(res.clone()),
        _ => None,
    }
}
pub fn seg_as_video(seg: &Segment) -> Option<Media> {
    match seg {
        Segment::Video { res, .. } => Some(res.clone()),
        _ => None,
    }
}
pub fn seg_as_at(seg: &Segment) -> Option<Uin> {
    match seg {
        Segment::Mention { user, .. } => Some(*user),
        _ => None,
    }
}
pub fn seg_as_reply(seg: &Segment) -> Option<MessageId> {
    match seg {
        Segment::Reply { id, .. } => Some(id.clone()),
        _ => None,
    }
}
pub fn seg_as_face(seg: &Segment) -> Option<String> {
    match seg {
        Segment::Face { id, .. } => Some(id.clone()),
        _ => None,
    }
}

/// 跳过 `text` 前 `k` 个空白分隔词,返回其后的**原文**(保留内部空白/换行;
/// 仅去掉到正文首字符前的分隔空白)。用于 `#[arg(rest, raw)]` 的正文保真。
pub fn skip_words(text: &str, k: usize) -> String {
    let mut rest = text.trim_start();
    for _ in 0..k {
        match rest.find(char::is_whitespace) {
            Some(pos) => rest = rest[pos..].trim_start(),
            None => {
                rest = "";
                break;
            }
        }
    }
    rest.to_string()
}

/// 由 `#[derive(Args)]` 生成。把 token 流(+ 原始文本,供 `#[arg(rest, raw)]` 保真)解析为 `Self`。
pub trait ParseArgs: Sized {
    fn parse_args(tokens: &[ArgToken<'_>], raw_text: &str)
        -> std::result::Result<Self, ArgError>;
}

/// 类型化参数提取器:`async fn h(args: Args<MyArgs>)`。
/// 仅在命令匹配后(`ParsedCommand` 存在)可用;解析失败 → `Skip`(=本 handler 不触发)。
pub struct Args<T>(pub T);

#[async_trait]
impl<T: ParseArgs + Send> FromContext for Args<T> {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        let parsed = ctx.get_ext::<ParsedCommand>().ok_or(Reject::Skip)?;
        let tokens = tokenize_segments(&parsed.args);
        // args_text 是各文本段原文拼接(空白保真),供 `#[arg(rest, raw)]` 取正文。
        match T::parse_args(&tokens, &parsed.args_text) {
            Ok(v) => Ok(Args(v)),
            Err(e) => {
                // 显式 `#[command(usage="…")]` 串优先于 dev 自动 hint;
                // 共享同一 parse-miss 策略(Args / Slots / usage= 三处同源)。
                let usage =
                    ctx.get_ext::<crate::matcher::CommandUsage>().map(|crate::matcher::CommandUsage(u)| u);
                on_parse_miss(ctx, &parsed.command, &e, usage.as_deref()).await;
                Err(Reject::Skip)
            }
        }
    }
}

/// 决定一次 head/tail 解析失败该做什么。**Args 与 Slots 共用一份策略**:
///   - prod + `usage` 存在 ⇒ 回贴该 usage 串,然后 Skip。
///   - dev(`App::debug()`)⇒ WARN + 回贴自动 usage_hint(既有 Args 行为),然后 Skip。
///   - prod + 无 usage ⇒ `debug!` 日志、静默 Skip(既有 Args 行为)。
///
/// 优先级:显式 `usage=` **总是**胜过 dev 自动 hint。把既有 `Args` 的内联 dev-WARN
/// 分支原样搬到这里——`Args` 行为字节级保持不变,只是策略现在被 `Args`/`Slots` 共用。
pub(crate) async fn on_parse_miss(ctx: &Ctx, command: &str, err: &ArgError, usage: Option<&str>) {
    // 显式 usage:无论 dev 与否都回贴该串后返回(优先于自动 hint)。
    if let Some(usage) = usage {
        tracing::debug!(command = %command, error = %err, "parse failed; replying explicit usage");
        if let Some(m) = ctx.message() {
            let _ = ctx.bot().send(&m.peer, &[Segment::text(usage)]).await;
        }
        return;
    }
    // 否则:prod 走 `debug!`(参数没中是预期分支,不刷屏);dev 升级 WARN + 回贴自动 hint。
    if ctx.is_dev() {
        let hint = usage_hint(command, err);
        tracing::warn!(
            command = %command,
            error = %err,
            "[dev] parse failed; skipping handler — {hint}"
        );
        if let Some(m) = ctx.message() {
            let _ = ctx.bot().send(&m.peer, &[Segment::text(hint)]).await;
        }
    } else {
        tracing::debug!(error = %err, "parse failed; skipping handler");
    }
}

/// 为一次失败的 `Args<T>` 解析构造一条人读用法提示，如
/// `用法错误: 命令 `transfer` — argument `amount`: cannot parse "x" as int`。
/// 由 dev 模式的 WARN + 可选的用法自动回贴使用。
fn usage_hint(command: &str, err: &ArgError) -> String {
    format!("用法错误: 命令 `{command}` — {err}")
}
