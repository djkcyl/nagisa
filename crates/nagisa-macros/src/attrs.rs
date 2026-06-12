//! `#[command]` / `#[event]` 属性参数的**解析层**。
//!
//! 职责:把宏入口拿到的属性 token 流解析成结构化参数——`CommandArgs`(匹配器
//! `MatcherKind` + 行为旗标 + `MetaArgs`)、`EventArgs`(`EventKind` 变体 + `MetaArgs`)、
//! 以及二者共用的 `MetaArgs`(`id`/`name`/`description`/`can_disable`/`default_enable`/
//! `hidden`/`gate`/`cooldown`/`usage`)。每个 meta 项先解析成内部 `Meta` 枚举(裸旗标 /
//! `key = 字面量` / `key = <expr>` / `key = <Type>`),`MetaValue` 再做字符串/整数/布尔
//! 的取值与类型校验。
//!
//! 协作:`apply_common_meta` 是两个属性宏共享的公共键消费器,二者的 `Parse` 各自只
//! 处理独有键(command 的匹配器 / `mention_me` / `usage`;event 的 `EventKind` 位置参)。
//! 解析结果交给 [`crate::trigger`] 展开成代码。`expr_to_litstr` 也供 [`crate::slots`]
//! 复用(解析 `union` 数组元素)。

use proc_macro2::Span;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Error, Expr, ExprLit, ExprUnary, Ident, Lit, LitBool, LitInt, LitStr, Token, Type, UnOp};

/// 匹配器选择：互斥的三选一（一种触发器,三种写法）。
/// **前导位置参数** = 字面量命令词(`#[command("签到", "sign")]`,编译成正则);`regex` = 原始正则;
/// `slots = <Type>` = 一个 `#[derive(Slots)]` 类型,头匹配器取 `<Type as FromSlots>::matcher()`。
pub(crate) enum MatcherKind {
    Union(Vec<String>),
    Regex(String),
    Slots(Type),
}

/// 解析后的 `#[command(...)]` 参数：匹配器 + 行为 + 可选插件元数据。
pub(crate) struct CommandArgs {
    pub(crate) kind: MatcherKind,
    pub(crate) mention_me: bool,
    /// 严格模式（`#[command(.., exact)]`）：整条消息只能是命令词本身，连 回复 / @bot
    /// 都不算；只对无参命令有意义（与 `args: Args<T>` 形参同用是编译错）。
    pub(crate) exact: bool,
    pub(crate) priority: i32,
    /// 一级 top 观察者（`#[command(top)]`）；永不被 waiter 拦截。
    pub(crate) top: bool,
    /// 触发器元数据（→ `TriggerMeta`）。`None` 表示未显式给出，用缺省。
    pub(crate) meta: MetaArgs,
}

/// `#[command]`/`#[event]` 的元数据参数（对应 `TriggerMeta`）。`name` 为 `None` 时 expand
/// 期回填函数名；bool 默认为可禁用 + 默认启用。
///
/// 只保留实际会被 `expand()` 接到 `TriggerMeta` 的键。曾经解析但被丢弃的
/// `group`/`version`/`aliases` 已删除（`aliases` 由匹配器的多个命令词覆盖）。
pub(crate) struct MetaArgs {
    /// 触发器 id（→ `TriggerMeta.id`）。`None` ⇒ expand 期回填函数名。
    pub(crate) id: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) description: Option<String>,
    /// 可禁用开关(键 `can_disable`)。
    pub(crate) can_disable: Option<bool>,
    pub(crate) default_enable: Option<bool>,
    pub(crate) hidden: Option<bool>,
    /// `gate = <Rule expr>`：任意 `Rule` 表达式，**原样**拼进门控槽。
    /// 宏对权限/场景词汇零知识——只搬运表达式。`None` ⇒ 无声明式门控。
    pub(crate) gate: Option<Expr>,
    /// `cooldown = 30` 或 `cooldown = Cooldown::new(..).max_exec(..)`：
    /// 经 `Cooldown::from(<expr>).into_rule(TriggerId::of(plugin,id))` 物化，`&` 进门控链**最右**。
    pub(crate) cooldown: Option<Expr>,
    /// `usage = "<str>"`：parse-miss 时经共享 `on_parse_miss` 回贴的用法串。
    pub(crate) usage: Option<String>,
    /// `order = <int>`：help 里同插件命令的展示次序（小在前），缺省 0、并列保持注册序。
    pub(crate) order: Option<i32>,
}

impl MetaArgs {
    /// 全空的初值（`CommandArgs`/`EventArgs` 解析前用）。
    fn empty() -> Self {
        MetaArgs {
            id: None,
            name: None,
            description: None,
            can_disable: None,
            default_enable: None,
            hidden: None,
            gate: None,
            cooldown: None,
            usage: None,
            order: None,
        }
    }
}

/// `#[command]`/`#[event]` 共用的 Meta 键消费：把对二者语义一致的公共键（`gate`/`cooldown`
/// 及 `id`/`name`/`description`/`can_disable`/`default_enable`/
/// `hidden`）落进 `meta`。返回 `Ok(true)` 表示本项已被消费，`Ok(false)` 表示是各宏独有键
/// （matcher 类/`priority`/`usage`/`slots` 等），交回调用方自行处理（独有报错分支不在此）。
///
/// `usage` 在两个宏里语义分叉（command 接受、event 报错），故**不**在此消费——留给各 parse。
fn apply_common_meta(meta: &mut MetaArgs, m: &Meta) -> syn::Result<bool> {
    match m {
        // `gate`/`cooldown`：原样搬运的 `Rule`/cooldown 表达式。
        Meta::Expr(id, expr) => match id.to_string().as_str() {
            "gate" => {
                meta.gate = Some(expr.clone());
                Ok(true)
            }
            "cooldown" => {
                meta.cooldown = Some(expr.clone());
                Ok(true)
            }
            _ => Ok(false),
        },
        Meta::KeyValue(id, value) => match id.to_string().as_str() {
            "id" => {
                meta.id = Some(value.as_single_string(id)?);
                Ok(true)
            }
            "name" => {
                meta.name = Some(value.as_single_string(id)?);
                Ok(true)
            }
            "description" => {
                meta.description = Some(value.as_single_string(id)?);
                Ok(true)
            }
            "can_disable" => {
                meta.can_disable = Some(value.as_bool(id)?);
                Ok(true)
            }
            "default_enable" => {
                meta.default_enable = Some(value.as_bool(id)?);
                Ok(true)
            }
            "hidden" => {
                meta.hidden = Some(value.as_bool(id)?);
                Ok(true)
            }
            "order" => {
                meta.order = Some(value.as_i32(id)?);
                Ok(true)
            }
            _ => Ok(false),
        },
        // 旗标 / 类型项不属公共键集合（`mention_me`/`top`/`slots` 各宏独有）。
        Meta::Flag(_) | Meta::Ty(_, _) => Ok(false),
    }
}

impl Parse for CommandArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut kind: Option<MatcherKind> = None;
        let mut mention_me = false;
        let mut exact = false;
        let mut top = false;
        let mut priority: i32 = 0;
        let mut meta = MetaArgs::empty();

        // 前导位置参数 = 字面命令词：`#[command("签到", "sign", ..)]`。命令词写在最前，
        // 后面才是其它键（regex/slots/gate/usage/…）。
        let mut words: Vec<String> = Vec::new();
        while input.peek(LitStr) {
            let s: LitStr = input.parse()?;
            words.push(s.value());
            if input.peek(Token![,]) {
                let _: Token![,] = input.parse()?;
            } else {
                break;
            }
        }
        if !words.is_empty() {
            kind = Some(MatcherKind::Union(words));
        }

        let metas = Punctuated::<Meta, Token![,]>::parse_terminated(input)?;
        for m in metas {
            // 公共键（gate/cooldown/id/name/description/can_disable/default_enable/hidden）
            // 由共享 helper 消费；返回 false 表示是 `#[command]` 独有键，落到下方分支。
            if apply_common_meta(&mut meta, &m)? {
                continue;
            }
            match m {
                Meta::Flag(id) if id == "mention_me" => mention_me = true,
                Meta::Flag(id) if id == "exact" => exact = true,
                Meta::Flag(id) if id == "top" => top = true,
                Meta::Flag(id) => {
                    return Err(Error::new(id.span(), format!("unknown flag `{id}`")));
                }
                // 公共 `gate`/`cooldown` 已在 helper 消费；到此的 Expr 必是未知键。
                Meta::Expr(id, _) => {
                    return Err(Error::new(id.span(), format!("unknown argument `{id}`")));
                }
                // `slots = <Type>`：`#[derive(Slots)]` 类型,头匹配器取 `<Type>::matcher()`。
                Meta::Ty(id, ty) => {
                    if id == "slots" {
                        set_kind(&mut kind, &id, MatcherKind::Slots(ty))?;
                    } else {
                        return Err(Error::new(id.span(), format!("unknown argument `{id}`")));
                    }
                }
                Meta::KeyValue(id, value) => {
                    let key = id.to_string();
                    match key.as_str() {
                        "regex" => {
                            set_kind(&mut kind, &id, MatcherKind::Regex(value.as_single_string(&id)?))?;
                        }
                        "priority" => priority = value.as_i32(&id)?,
                        // `usage = "<str>"`：parse-miss 回贴用法串。command 独有
                        // （event 无 parser 可 miss，在其 parse 里报错）。
                        "usage" => meta.usage = Some(value.as_single_string(&id)?),
                        other => {
                            return Err(Error::new(id.span(), format!("unknown argument `{other}`")));
                        }
                    }
                }
            }
        }

        let kind = kind.ok_or_else(|| {
            Error::new(
                Span::call_site(),
                "#[command] 需要一个匹配器：前导位置命令词 `\"..\"`、`regex=\"..\"` 或 `slots=Type`",
            )
        })?;

        Ok(CommandArgs { kind, mention_me, exact, priority, top, meta })
    }
}

fn set_kind(slot: &mut Option<MatcherKind>, id: &Ident, kind: MatcherKind) -> syn::Result<()> {
    if slot.is_some() {
        return Err(Error::new(id.span(), "只能给一种匹配器：前导位置命令词、`regex=\"..\"` 或 `slots=Type`，三选一"));
    }
    *slot = Some(kind);
    Ok(())
}

/// 一个 meta 项：裸标志（`mention_me`）、字面量 `key = value`、表达式 `key = <expr>`
/// （`gate`/`cooldown` 收任意 `Rule`/cooldown 表达式，原样搬运，宏对其内容零知识），
/// 或类型 `slots = <Type>`（`#[derive(Slots)]` 类型，取其 `::matcher()`）。
enum Meta {
    Flag(Ident),
    KeyValue(Ident, MetaValue),
    Expr(Ident, Expr),
    Ty(Ident, Type),
}

/// `key = value` 里的 value：字符串、整数或布尔（命令词改成位置参数后，已无键收数组）。
enum MetaValue {
    Str(LitStr),
    Int(LitInt),
    NegInt(LitInt), // 负整数:-N
    Bool(LitBool),
}

impl Parse for Meta {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let id: Ident = input.parse()?;
        if input.peek(Token![=]) {
            let _: Token![=] = input.parse()?;
            // `gate`/`cooldown` 收任意表达式（`Rule` 代数 / `Cooldown::new(..)…`）——
            // 整段原样吃进一个 `Expr`，交由 expand 期拼接，宏不解读其内容。
            if id == "gate" || id == "cooldown" {
                let expr: Expr = input.parse()?;
                Ok(Meta::Expr(id, expr))
            } else if id == "slots" {
                // `slots = <Type>`：一个 `#[derive(Slots)]` 类型路径,取其 `::matcher()`。
                let ty: Type = input.parse()?;
                Ok(Meta::Ty(id, ty))
            } else {
                let value = MetaValue::parse(input)?;
                Ok(Meta::KeyValue(id, value))
            }
        } else {
            Ok(Meta::Flag(id))
        }
    }
}

impl Parse for MetaValue {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let expr: Expr = input.parse()?;
        match &expr {
            Expr::Lit(ExprLit { lit: Lit::Str(s), .. }) => Ok(MetaValue::Str(s.clone())),
            Expr::Lit(ExprLit { lit: Lit::Int(i), .. }) => Ok(MetaValue::Int(i.clone())),
            Expr::Lit(ExprLit { lit: Lit::Bool(b), .. }) => Ok(MetaValue::Bool(b.clone())),
            // 负整数:-N
            Expr::Unary(ExprUnary { op: UnOp::Neg(_), expr: inner, .. }) => {
                if let Expr::Lit(ExprLit { lit: Lit::Int(i), .. }) = inner.as_ref() {
                    Ok(MetaValue::NegInt(i.clone()))
                } else {
                    Err(Error::new_spanned(expr, "expected a string, integer, or bool literal"))
                }
            }
            _ => Err(Error::new_spanned(expr, "expected a string, integer, or bool literal")),
        }
    }
}

pub(crate) fn expr_to_litstr(expr: &Expr) -> syn::Result<LitStr> {
    match expr {
        Expr::Lit(ExprLit { lit: Lit::Str(s), .. }) => Ok(s.clone()),
        _ => Err(Error::new_spanned(expr, "expected a string literal")),
    }
}

impl MetaValue {
    /// 接受单字符串（供 `regex`/`id`/`name`/`description`/`usage` 等键值参数）。
    fn as_single_string(&self, id: &Ident) -> syn::Result<String> {
        match self {
            MetaValue::Str(s) => Ok(s.value()),
            _ => Err(Error::new(id.span(), format!("`{id}` expects a single string literal"))),
        }
    }

    /// 接受整数（供 `priority`），支持负值。
    fn as_i32(&self, id: &Ident) -> syn::Result<i32> {
        match self {
            MetaValue::Int(i) => i.base10_parse::<i32>(),
            MetaValue::NegInt(i) => {
                let n = i.base10_parse::<i32>()?;
                n.checked_neg().ok_or_else(|| Error::new(id.span(), "priority value out of range for i32"))
            }
            _ => Err(Error::new(id.span(), format!("`{id}` expects an integer literal"))),
        }
    }

    /// 接受布尔字面量（供 `can_disable`/`default_enable`/`hidden`）。
    fn as_bool(&self, id: &Ident) -> syn::Result<bool> {
        match self {
            MetaValue::Bool(b) => Ok(b.value()),
            _ => Err(Error::new(id.span(), format!("`{id}` expects a bool literal (true/false)"))),
        }
    }
}

// ───────────────────────── #[event(Kind, ..)] ─────────────────────────

/// 解析后的 `#[event(..)]` 参数：`EventKind` 变体 + 优先级 + 可选插件元数据。
/// 不含匹配器（事件触发器无 `command`/`regex`）。
pub(crate) struct EventArgs {
    /// 第一个位置参数：`EventKind` 变体的裸标识符（如 `MemberJoin`）。
    pub(crate) kind: Ident,
    pub(crate) priority: i32,
    /// 一级 top 观察者（`#[event(Kind, top)]`）；永不被 waiter 拦截。
    pub(crate) top: bool,
    /// 插件元数据（→ `TriggerMeta`）。`None` 表示用缺省。
    pub(crate) meta: MetaArgs,
}

impl Parse for EventArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        // 第一个 token 必须是 EventKind 变体的裸标识符。
        let kind: Ident = input.parse().map_err(|_| {
            Error::new(
                Span::call_site(),
                "#[event] requires an EventKind variant as its first argument, e.g. #[event(MemberJoin)]",
            )
        })?;
        // 其余键值参数前的逗号（可选——只给 Kind 时无逗号）。
        if input.peek(Token![,]) {
            let _: Token![,] = input.parse()?;
        }

        let mut priority: i32 = 0;
        let mut top = false;
        let mut meta = MetaArgs::empty();

        let metas = Punctuated::<Meta, Token![,]>::parse_terminated(input)?;
        for m in metas {
            // 公共键（gate/cooldown/id/name/description/can_disable/default_enable/hidden）
            // 由共享 helper 消费；返回 false 表示是 `#[event]` 独有键，落到下方分支。
            if apply_common_meta(&mut meta, &m)? {
                continue;
            }
            match m {
                Meta::Flag(id) if id == "top" => top = true,
                Meta::Flag(id) => {
                    return Err(Error::new(id.span(), format!("unknown flag `{id}`")));
                }
                // 公共 `gate`/`cooldown` 已在 helper 消费；到此的 Expr 必是未知键。
                Meta::Expr(id, _) => {
                    return Err(Error::new(id.span(), format!("unknown argument `{id}`")));
                }
                // `slots=` 是命令头匹配器（事件触发器无匹配器）⇒ 在此非法。
                Meta::Ty(id, _) => {
                    return Err(Error::new(id.span(), format!("`{id}` is not valid on #[event]")));
                }
                Meta::KeyValue(id, value) => {
                    let key = id.to_string();
                    match key.as_str() {
                        "priority" => priority = value.as_i32(&id)?,
                        // `usage=` 是 `#[command]` 的 parse-miss 回贴串;事件触发器没有
                        // 「用户敲错参数」一说、无任何消费者 ⇒ 明确报错,而非接受后悄悄
                        // 丢弃(accept-and-ignore 是坑:作者会以为它生效了)。
                        "usage" => {
                            return Err(Error::new(
                                id.span(),
                                "`usage=` is only meaningful on #[command] (parse-miss reply); \
                                 #[event] has no parser to miss — remove it",
                            ));
                        }
                        other => {
                            return Err(Error::new(id.span(), format!("unknown argument `{other}`")));
                        }
                    }
                }
            }
        }

        Ok(EventArgs { kind, priority, top, meta })
    }
}
