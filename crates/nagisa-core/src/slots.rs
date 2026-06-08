//! 类型化命名 slot 投影:`Slots<T>` —— `Args<T>` 的字面孪生。
//!
//! 消息有**两个解析面**:HEAD(匹配器看到的命名正则捕获组)与 TAIL(命令头之后的段流)。
//! `Args<T>`(`args.rs`)拥有 TAIL;**`Slots<T>` 拥有 HEAD**——`Matcher::slots` 编出的
//! 命名、可选、类型化捕获组,投影成结构体字段。`Captures(Vec<String>)` 是位置/无类型的,
//! `Slots` 是命名 + `Option<T>` 可选 + 边界处一次性类型化解析。
//!
//! 设计要点:
//! - **`SlotValue`**:单捕获投影。空白桥 `impl<T: FromArg> SlotValue for T` 让 int/String/Uin
//!   **及任意 `#[derive(ArgEnum)]` 枚举**零代码即是 `SlotValue`(union/enum 槽免费)。
//!   **没有** `impl SlotValue for (A,B)`:tuple 是多组关注,由 `#[derive(Slots)]` 在它知道两个
//!   内组的那一层处理。
//! - **`FromSlots`**:由 `#[derive(Slots)]` 生成,产出 `matcher()` + 从 `named_captures` 投影。
//! - **`Slots<T>: FromContext`**:`Args<T>` 的镜像,共享同一 parse-miss 路径(`on_parse_miss`)。
//! - **`Tail<T>`**:贪婪尾槽(`(.*)` → `Option<String>`),或 `Tail<Vec<Segment>>` 收原始多模态尾。
use crate::args::{on_parse_miss, ArgError, FromArg};
use crate::ctx::Ctx;
use crate::extract::{Extracted, FromContext, Reject};
use crate::matcher::{Matcher, ParsedCommand};
use async_trait::async_trait;
use nagisa_types::segment::Segment;
use std::borrow::Cow;
use std::collections::HashMap;

/// 命名正则槽的别名:名 → `Some(text)`(命中) | `None`(缺省的可选槽)。
/// 即 `ParsedCommand.named_captures` 的类型,`FromSlots::from_slots` 据此投影。
pub type NamedCaptures = HashMap<Cow<'static, str>, Option<String>>;

/// 在框架边界把**单个**捕获组文本解析成一个字段值。`FromArg` 的兄弟,复用 `ArgError`。
///
/// `from_capture` 总是看到**恰好一个**捕获组 → 一个 `&str`,且**无从**得知槽的分隔符
/// (那住在 `SlotSpec`/derive 里,从不被穿进来)——故 tuple **不**实现 `SlotValue`,
/// 由 derive 在多组层处理。
pub trait SlotValue: Sized {
    /// 期望类型的人读名(报错用)。
    const TYPE_NAME: &'static str;
    /// 把单个捕获组文本解析成 `Self`。
    fn from_capture(s: &str) -> Result<Self, ArgError>;
}

/// 空白桥:任意 `FromArg` 类型即是 `SlotValue`——复用 String/整数/Uin 实现**及**每个
/// `#[derive(ArgEnum)]` 类型。`FromArg` 对 tuple 无实现(`args.rs`),故与未来手写的
/// 单捕获 `SlotValue` 无 coherence 冲突。
impl<T: FromArg> SlotValue for T {
    const TYPE_NAME: &'static str = <T as FromArg>::TYPE_NAME;
    fn from_capture(s: &str) -> Result<Self, ArgError> {
        T::from_arg(s).ok_or_else(|| ArgError::Parse {
            field: "<slot>",
            value: s.to_string(),
            expected: <T as FromArg>::TYPE_NAME,
        })
    }
}

/// 由 `#[derive(Slots)]` 生成:产出命令头匹配器 + 从命名捕获投影出 `Self`。
pub trait FromSlots: Sized {
    /// 命令头构建器(`Matcher::slots(..)`)。
    fn matcher() -> Matcher;
    /// 从命名捕获投影出 `Self`;失败 ⇒ `ArgError`(经 `on_parse_miss` 处理)。
    fn from_slots(caps: &NamedCaptures) -> Result<Self, ArgError>;
    /// 可选的显式用法串(`#[slots(usage="…")]`);默认 `None`。
    fn usage() -> Option<&'static str> {
        None
    }
}

/// 类型化命名 slot 提取器:`async fn h(m: Slots<ViewBoard>)`。
/// `Args<T>` 的字面孪生——同样仅在命令匹配后(`ParsedCommand` 存在)可用,解析失败经**共享**
/// `on_parse_miss`(prod usage 回贴 / dev WARN / 静默)后 `Skip`。
pub struct Slots<T>(pub T);

#[async_trait]
impl<T: FromSlots + Send> FromContext for Slots<T> {
    async fn from_context(ctx: &Ctx) -> Extracted<Self> {
        let p = ctx.get_ext::<ParsedCommand>().ok_or(Reject::Skip)?; // = Args<T> 的入口。
        match T::from_slots(&p.named_captures) {
            Ok(v) => Ok(Slots(v)),
            Err(e) => {
                on_parse_miss(ctx, &p.command, &e, T::usage()).await;
                Err(Reject::Skip)
            }
        }
    }
}

/// 贪婪尾槽的载荷类型。`#[slot(tail)] q: Option<String>` 直接用 `String`
/// (无需 `Tail` 包装);`Tail<Vec<Segment>>` 把未解析的多模态尾原样交还(此实现下尾是纯文本,
/// 故包成单个 `Segment::Text`——`from_slots` 仅见文本捕获)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tail<T>(pub T);

/// 从尾捕获文本构造一个尾载荷。derive 对 `#[slot(tail)]` 字段调用它。
pub trait FromTailText: Sized {
    fn from_tail_text(s: &str) -> Self;
}

impl FromTailText for String {
    fn from_tail_text(s: &str) -> Self {
        s.to_string()
    }
}

impl<T: FromTailText> FromTailText for Tail<T> {
    fn from_tail_text(s: &str) -> Self {
        Tail(T::from_tail_text(s))
    }
}

impl FromTailText for Vec<Segment> {
    fn from_tail_text(s: &str) -> Self {
        if s.is_empty() {
            Vec::new()
        } else {
            vec![Segment::text(s)]
        }
    }
}
