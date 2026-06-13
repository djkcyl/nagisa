//! `#[derive(Slots)]` 与 `matcher!{}` 的**解析 + 展开层**:命令头 = **有序区块序列**。
//!
//! 命令头是一串自由拼接的区块——**字面块**(固定字符串)与**捕获块**(命名、类型化正则槽),
//! 顺序与块间空白可配。结构体级 `#[slots(...)]` 给出序列:
//!
//! ```ignore
//! #[derive(Slots)]
//! #[slots(lit("查看"), board, lit("榜"), scope, sep = "\\s*", usage = "…")]
//! struct ViewRank {
//!     #[slot(union = ["排行", "金币", "等级", "发言", "签到"])] board: String,
//!     #[slot(union = ["全局", "全站"])] scope: Option<String>,
//! }
//! ```
//!
//! - `lit("查看")` = 固定块;裸标识符 `board` = 引用同名字段(其 `#[slot(union=/re=/tail)]` 定来源/
//!   类型;`Option<T>` ⇒ 可选块);`sep = "…"` = 块间分隔正则(默认 `\s*`,容忍可选空白);
//!   `usage = "…"` = 解析失败回贴串。不写任何固定块/字段引用时,序列退化为「字段声明顺序」。
//!
//! `expand_slots` 把序列编成一串 `SlotSpec`(字面块 `SlotSpec::literal`、捕获块命名槽,块间补
//! `sep`)产出 `Matcher`;`from_slots()` 从 `NamedCaptures` 逐字段投影;`command_words()` 把
//! 「字面 × 各必填 `union` 块」的笛卡尔积算成具体命令词(查看排行榜 / 查看金币榜 …),供 help 像
//! 字面命令一样枚举。`matcher!{}` 是同款 codegen 的内联入口。

use proc_macro2::Span;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Data, DeriveInput, Error, ExprArray, Fields, Ident, LitStr, Token, Type};

use crate::args_derive::option_inner;
use crate::attrs::expr_to_litstr;
use crate::nagisa_core_root;

/// 一个 slot 字段的正则来源。
enum SlotSrc {
    /// `#[slot(re = "…")]` 原始正则片段。
    Re(String),
    /// `#[slot(union = ["a","b"])]` 字面量交替(转义后拼 `a|b`)。
    Union(Vec<String>),
    /// `#[slot(tail)]` 贪婪尾(`[\s\S]*`)。
    Tail,
}

/// 解析后的一个 slot 字段(捕获块的来源/类型)。
struct SlotField {
    ident: Ident,
    /// 去掉 `Option` 后的类型(可选性单独记 `optional`)。
    inner_ty: Type,
    optional: bool,
    src: SlotSrc,
    /// 展示名(`#[slot(name="…")]`,缺则字段标识符):用法模板的占位符 + 参数区的参数名。
    name: Option<String>,
    /// 参数说明(`#[slot(desc="…")]`,缺则空):进参数区那一行的说明(选项 / 可选标记自动前缀)。
    desc: Option<String>,
    /// 若 `inner_ty` 是 tuple,各元素类型(供多组 codegen);否则空。
    tuple_elems: Vec<Type>,
}

impl SlotField {
    /// 展示名:`#[slot(name=…)]` 或字段标识符。
    fn label(&self) -> String {
        self.name.clone().unwrap_or_else(|| self.ident.to_string())
    }
}

/// 序列里的一个区块:字面块 or 引用某字段的捕获块(`usize` 是 `fields` 下标)。
enum Block {
    Lit(String),
    Field(usize),
}

/// 块间分隔正则的默认值(容忍可选空白)。
const DEFAULT_SEP: &str = r"\s*";

/// 若 `ty` 是 tuple `(A, B, …)` 返回其元素类型;否则 `None`。
fn tuple_elems(ty: &Type) -> std::option::Option<Vec<Type>> {
    match ty {
        Type::Tuple(t) if !t.elems.is_empty() => Some(t.elems.iter().cloned().collect()),
        _ => None,
    }
}

// ───────────────────────── 结构体级序列 `#[slots(...)]` 解析 ─────────────────────────

/// `#[slots(...)]` 里的一项:固定块字面、字段引用、或 `sep=`/`usage=` 键值。
enum SeqItem {
    Lit(String),
    Field(Ident),
    Sep(String),
    Usage(String),
}

impl Parse for SeqItem {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let id: Ident = input.parse()?;
        // 固定块写成 `lit("文字")`(合法 MetaList:rust-analyzer 不会把裸字面当意外 token 报红)。
        if id == "lit" {
            let content;
            syn::parenthesized!(content in input);
            return Ok(SeqItem::Lit(content.parse::<LitStr>()?.value()));
        }
        if input.peek(Token![=]) {
            let _: Token![=] = input.parse()?;
            let s: LitStr = input.parse()?;
            return match id.to_string().as_str() {
                "sep" => Ok(SeqItem::Sep(s.value())),
                "usage" => Ok(SeqItem::Usage(s.value())),
                other => Err(Error::new(
                    id.span(),
                    format!("unknown #[slots(..)] key `{other}` (use sep=/usage=, lit(\"..\") block, or a field name)"),
                )),
            };
        }
        Ok(SeqItem::Field(id))
    }
}

/// 收齐 `#[slots(...)]`:有序条目(字面 / 字段引用)+ `sep` + `usage`。
struct SlotsAttr {
    items: Vec<SeqItem>,
    sep: Option<String>,
    usage: Option<String>,
}

fn parse_slots_attr(input: &DeriveInput) -> syn::Result<SlotsAttr> {
    let mut items = Vec::new();
    let mut sep = None;
    let mut usage = None;
    for attr in &input.attrs {
        if !attr.path().is_ident("slots") {
            continue;
        }
        let parsed = attr.parse_args_with(Punctuated::<SeqItem, Token![,]>::parse_terminated)?;
        for it in parsed {
            match it {
                SeqItem::Sep(s) => sep = Some(s),
                SeqItem::Usage(u) => usage = Some(u),
                other => items.push(other),
            }
        }
    }
    Ok(SlotsAttr { items, sep, usage })
}

// ───────────────────────── 字段 `#[slot(...)]` 解析 ─────────────────────────

fn parse_fields(input: &DeriveInput) -> syn::Result<Vec<SlotField>> {
    let struct_ident = &input.ident;
    let fields = match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(named) => &named.named,
            _ => return Err(Error::new_spanned(struct_ident, "#[derive(Slots)] requires a struct with named fields")),
        },
        _ => return Err(Error::new_spanned(struct_ident, "#[derive(Slots)] only supports structs")),
    };

    let mut slots: Vec<SlotField> = Vec::new();
    for field in fields {
        let ident = field.ident.clone().expect("named field");
        let ty = field.ty.clone();
        let (inner_ty, optional) = match option_inner(&ty) {
            Some(inner) => (inner.clone(), true),
            None => (ty.clone(), false),
        };

        let mut src: std::option::Option<SlotSrc> = None;
        let mut name: std::option::Option<String> = None;
        let mut desc: std::option::Option<String> = None;
        for attr in &field.attrs {
            if !attr.path().is_ident("slot") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("re") {
                    set_slot_src(&mut src, &meta, SlotSrc::Re(meta.value()?.parse::<LitStr>()?.value()))
                } else if meta.path.is_ident("union") {
                    let arr: ExprArray = meta.value()?.parse()?;
                    let mut alts = Vec::with_capacity(arr.elems.len());
                    for e in &arr.elems {
                        alts.push(expr_to_litstr(e)?.value());
                    }
                    set_slot_src(&mut src, &meta, SlotSrc::Union(alts))
                } else if meta.path.is_ident("tail") {
                    set_slot_src(&mut src, &meta, SlotSrc::Tail)
                } else if meta.path.is_ident("name") {
                    name = Some(meta.value()?.parse::<LitStr>()?.value());
                    Ok(())
                } else if meta.path.is_ident("desc") {
                    desc = Some(meta.value()?.parse::<LitStr>()?.value());
                    Ok(())
                } else {
                    Err(meta.error("unknown #[slot(..)] key (use re=/union=/tail/name=/desc=)"))
                }
            })?;
        }

        let src = src.ok_or_else(|| {
            Error::new_spanned(
                &field.ident,
                "#[derive(Slots)] field requires #[slot(re=\"..\")] / #[slot(union=[..])] / #[slot(tail)]",
            )
        })?;

        let tuple_elems = tuple_elems(&inner_ty).unwrap_or_default();
        if matches!(src, SlotSrc::Tail) && !tuple_elems.is_empty() {
            return Err(Error::new_spanned(&field.ident, "#[slot(tail)] cannot be a tuple"));
        }

        slots.push(SlotField { ident, inner_ty, optional, src, name, desc, tuple_elems });
    }

    // 重名槽名 = 编译错误。
    let mut seen = std::collections::HashSet::new();
    for s in &slots {
        if !seen.insert(s.ident.to_string()) {
            return Err(Error::new_spanned(&s.ident, "duplicate slot name"));
        }
    }
    Ok(slots)
}

/// 把 `#[slots(...)]` 序列解析成有序 [`Block`]:字面块直存,字段引用查到 `fields` 下标;不写序列时
/// 退化为「字段声明顺序」。校验:每个字段恰被引用一次,引用的标识符必须是已声明字段。
fn resolve_blocks(items: &[SeqItem], fields: &[SlotField]) -> syn::Result<Vec<Block>> {
    if items.is_empty() {
        return Ok((0..fields.len()).map(Block::Field).collect());
    }
    let mut blocks = Vec::new();
    let mut used = vec![false; fields.len()];
    for it in items {
        match it {
            SeqItem::Lit(s) => blocks.push(Block::Lit(s.clone())),
            SeqItem::Field(id) => {
                let idx = fields
                    .iter()
                    .position(|f| &f.ident == id)
                    .ok_or_else(|| Error::new(id.span(), format!("#[slots(..)] references unknown field `{id}`")))?;
                if used[idx] {
                    return Err(Error::new(id.span(), format!("#[slots(..)] references field `{id}` more than once")));
                }
                used[idx] = true;
                blocks.push(Block::Field(idx));
            }
            _ => {}
        }
    }
    if let Some(i) = used.iter().position(|u| !u) {
        return Err(Error::new_spanned(
            &fields[i].ident,
            format!("field `{}` is not placed in #[slots(..)] sequence", fields[i].ident),
        ));
    }
    Ok(blocks)
}

// ───────────────────────── codegen ─────────────────────────

pub(crate) fn expand_slots(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let nc = nagisa_core_root();
    let struct_ident = &input.ident;

    let attr = parse_slots_attr(&input)?;
    let fields = parse_fields(&input)?;
    let blocks = resolve_blocks(&attr.items, &fields)?;
    let sep = attr.sep.as_deref().unwrap_or(DEFAULT_SEP);

    // —— SlotSpec 列表(块序;块间补 sep,首块不补)。——
    let spec_exprs: Vec<proc_macro2::TokenStream> =
        blocks.iter().enumerate().map(|(i, b)| block_spec(&nc, b, &fields, if i == 0 { "" } else { sep })).collect();

    // —— from_slots:逐字段从 named_captures 投影(与块序无关,按名取)。——
    let field_builds: Vec<proc_macro2::TokenStream> = fields.iter().map(|s| field_build(&nc, s)).collect();
    let field_idents: Vec<&Ident> = fields.iter().map(|s| &s.ident).collect();

    // —— command_words:字面 × 各必填 union 块的笛卡尔积(编译期算)。——
    let words = command_words(&blocks, &fields);
    let word_lits = words.iter().map(|w| LitStr::new(w, Span::call_site()));

    // —— synopsis:用法模板(固定块原样、必填捕获块 `<名>`、可选捕获块 `[名]`)。——
    let synopsis_lit = LitStr::new(&slots_synopsis(&blocks, &fields), Span::call_site());

    // —— SLOTS:每个命名槽一条 ArgSpec(供 help 在「参数」区列出)。——
    let slot_specs: Vec<proc_macro2::TokenStream> = fields.iter().map(|s| slot_argspec(&nc, s)).collect();

    let usage_fn = match &attr.usage {
        Some(u) => {
            let lit = LitStr::new(u, Span::call_site());
            quote! { fn usage() -> ::std::option::Option<&'static str> { ::std::option::Option::Some(#lit) } }
        }
        None => quote! {},
    };

    Ok(quote! {
        impl #nc::FromSlots for #struct_ident {
            fn matcher() -> #nc::Matcher {
                #nc::Matcher::slots(::std::vec![ #( #spec_exprs ),* ])
                    .expect("#[derive(Slots)] produced an invalid slot program")
            }
            fn from_slots(__caps: &#nc::NamedCaptures)
                -> ::std::result::Result<Self, #nc::ArgError>
            {
                #( #field_builds )*
                ::std::result::Result::Ok(#struct_ident { #( #field_idents ),* })
            }
            const COMMAND_WORDS: &'static [&'static str] = &[ #( #word_lits ),* ];
            const SLOTS_SYNOPSIS: &'static str = #synopsis_lit;
            const SLOTS: &'static [#nc::ArgSpec] = &[ #( #slot_specs ),* ];
            #usage_fn
        }
    })
}

/// 一个区块的 `SlotSpec` 表达式。`lead` 是块前要补的分隔正则(首块为 `""`)。
fn block_spec(
    nc: &proc_macro2::TokenStream,
    block: &Block,
    fields: &[SlotField],
    lead: &str,
) -> proc_macro2::TokenStream {
    let lead_lit = LitStr::new(lead, Span::call_site());
    match block {
        Block::Lit(text) => {
            let lit = LitStr::new(text, Span::call_site());
            quote! {
                #nc::SlotSpec {
                    names: ::std::vec![],
                    src: ::std::format!("{}{}", #lead_lit, #nc::regex_escape(#lit)),
                    optional: false,
                    flank: #nc::Flank::Whole,
                }
            }
        }
        Block::Field(idx) => {
            let s = &fields[*idx];
            let names = slot_group_names(s);
            let name_lits = names.iter().map(|n| LitStr::new(n, Span::call_site()));
            let body = match &s.src {
                SlotSrc::Re(re) => {
                    let re_lit = LitStr::new(re, Span::call_site());
                    quote! { ::std::string::String::from(#re_lit) }
                }
                SlotSrc::Union(alts) => {
                    let escaped: Vec<LitStr> = alts.iter().map(|a| LitStr::new(a, Span::call_site())).collect();
                    quote! { ::std::format!("({})", [ #( #nc::regex_escape(#escaped) ),* ].join("|")) }
                }
                SlotSrc::Tail => quote! { ::std::string::String::from(r"([\s\S]*)") },
            };
            let optional = s.optional;
            quote! {
                #nc::SlotSpec {
                    names: ::std::vec![ #( ::std::borrow::Cow::Borrowed(#name_lits) ),* ],
                    src: ::std::format!("{}{}", #lead_lit, #body),
                    optional: #optional,
                    flank: #nc::Flank::Whole,
                }
            }
        }
    }
}

/// 编译期算「字面 × 各必填 union 块」的笛卡尔积成完整命令词。可选块不展开;遇必填非 union 块
/// (`re`/`tail`,不可枚举)⇒ 返回空(help 退回按命令名显示)。无任何 union 块亦返回空。
fn command_words(blocks: &[Block], fields: &[SlotField]) -> Vec<String> {
    let mut words = vec![String::new()];
    let mut saw_union = false;
    for b in blocks {
        match b {
            Block::Lit(text) => {
                for w in &mut words {
                    w.push_str(text);
                }
            }
            Block::Field(idx) => {
                let s = &fields[*idx];
                if s.optional {
                    continue; // 可选块不进枚举
                }
                match &s.src {
                    SlotSrc::Union(alts) => {
                        saw_union = true;
                        words = words.iter().flat_map(|w| alts.iter().map(move |a| format!("{w}{a}"))).collect();
                    }
                    _ => return Vec::new(), // 必填 re/tail ⇒ 不可枚举
                }
            }
        }
    }
    if saw_union {
        words
    } else {
        Vec::new()
    }
}

/// 编译期渲用法模板:固定块原样、必填捕获块 `<选项|…>`(union 列选项,re/tail 用字段名占位)、可选
/// 捕获块 `[选项|…]`。如 `查看<排行|金币|等级|发言|签到>榜[全局|全站]`。供 help 当用法行直接显示。
fn slots_synopsis(blocks: &[Block], fields: &[SlotField]) -> String {
    let mut out = String::new();
    for b in blocks {
        match b {
            Block::Lit(text) => out.push_str(text),
            Block::Field(idx) => {
                // 用占位符(展示名),不内联选项——选项进「参数」区列出。必填 `<名>`、可选 `[名]`。
                let s = &fields[*idx];
                let (l, r) = if s.optional { ('[', ']') } else { ('<', '>') };
                out.push(l);
                out.push_str(&s.label());
                out.push(r);
            }
        }
    }
    out
}

/// 一个命名槽的 [`ArgSpec`](供 help 在「参数」区列出)。`kind` 一律 `Positional`(槽即位置取值);
/// `desc` 自动前缀「可选」与 `union` 可选值,再接作者 `#[slot(desc=…)]`,用 ` · ` 连。
fn slot_argspec(nc: &proc_macro2::TokenStream, s: &SlotField) -> proc_macro2::TokenStream {
    let name = LitStr::new(&s.label(), Span::call_site());
    let required = !s.optional;
    let mut parts: Vec<String> = Vec::new();
    if s.optional {
        parts.push("可选".to_string());
    }
    if let SlotSrc::Union(alts) = &s.src {
        parts.push(alts.join("/"));
    }
    if let Some(d) = &s.desc {
        if !d.is_empty() {
            parts.push(d.clone());
        }
    }
    let desc = LitStr::new(&parts.join(" · "), Span::call_site());
    quote! {
        #nc::ArgSpec {
            name: #name,
            kind: #nc::ArgKind::Positional,
            short: "",
            long: "",
            required: #required,
            default: "",
            desc: #desc,
        }
    }
}

/// 单字段的 `from_slots` 投影代码(单捕获经 `SlotValue`、tuple 多组各经 `FromArg`、tail 经 `FromTailText`)。
fn field_build(nc: &proc_macro2::TokenStream, s: &SlotField) -> proc_macro2::TokenStream {
    let ident = &s.ident;
    let fs = LitStr::new(&ident.to_string(), Span::call_site());
    let names = slot_group_names(s);
    let name_lits: Vec<LitStr> = names.iter().map(|n| LitStr::new(n, Span::call_site())).collect();

    let get = |nm: &LitStr| quote! { __caps.get(#nm).and_then(|__o| __o.as_ref()) };

    if matches!(s.src, SlotSrc::Tail) {
        let nm = &name_lits[0];
        let g = get(nm);
        let inner_ty = &s.inner_ty;
        if s.optional {
            quote! {
                let #ident = match #g {
                    ::std::option::Option::Some(__t) if !__t.is_empty() =>
                        ::std::option::Option::Some(<#inner_ty as #nc::FromTailText>::from_tail_text(__t.as_str())),
                    _ => ::std::option::Option::None,
                };
            }
        } else {
            quote! {
                let #ident = <#inner_ty as #nc::FromTailText>::from_tail_text(
                    #g.map(|__s| __s.as_str()).unwrap_or("")
                );
            }
        }
    } else if !s.tuple_elems.is_empty() {
        let elems = &s.tuple_elems;
        let parses: Vec<proc_macro2::TokenStream> = elems
            .iter()
            .zip(&name_lits)
            .map(|(ety, nm)| {
                let g = get(nm);
                quote! {
                    match #g {
                        ::std::option::Option::Some(__s) =>
                            match <#ety as #nc::FromArg>::from_arg(__s.as_str()) {
                                ::std::option::Option::Some(__v) => __v,
                                ::std::option::Option::None =>
                                    return ::std::result::Result::Err(#nc::ArgError::Parse {
                                        field: #fs,
                                        value: ::std::string::String::from(__s.as_str()),
                                        expected: <#ety as #nc::FromArg>::TYPE_NAME,
                                    }),
                            },
                        ::std::option::Option::None =>
                            return ::std::result::Result::Err(#nc::ArgError::Missing(#fs)),
                    }
                }
            })
            .collect();
        let tuple_val = quote! { ( #( #parses ),* ) };
        if s.optional {
            let first = &name_lits[0];
            let probe = get(first);
            quote! {
                let #ident = match #probe {
                    ::std::option::Option::Some(_) => ::std::option::Option::Some(#tuple_val),
                    ::std::option::Option::None => ::std::option::Option::None,
                };
            }
        } else {
            quote! { let #ident = #tuple_val; }
        }
    } else {
        let nm = &name_lits[0];
        let g = get(nm);
        let inner_ty = &s.inner_ty;
        let parse_some = quote! {
            match <#inner_ty as #nc::SlotValue>::from_capture(__s.as_str()) {
                ::std::result::Result::Ok(__v) => __v,
                ::std::result::Result::Err(__e) => return ::std::result::Result::Err(__e),
            }
        };
        if s.optional {
            quote! {
                let #ident = match #g {
                    ::std::option::Option::Some(__s) => ::std::option::Option::Some(#parse_some),
                    ::std::option::Option::None => ::std::option::Option::None,
                };
            }
        } else {
            quote! {
                let #ident = {
                    let __s = match #g {
                        ::std::option::Option::Some(__s) => __s,
                        ::std::option::Option::None => return ::std::result::Result::Err(#nc::ArgError::Missing(#fs)),
                    };
                    #parse_some
                };
            }
        }
    }
}

/// 防止一个字段同时给多个 `#[slot(..)]` 来源。
fn set_slot_src(
    slot: &mut std::option::Option<SlotSrc>,
    meta: &syn::meta::ParseNestedMeta,
    src: SlotSrc,
) -> syn::Result<()> {
    if slot.is_some() {
        return Err(meta.error("only one of re=/union=/tail may be given per slot"));
    }
    *slot = Some(src);
    Ok(())
}

/// 一个 slot 字段的命名组键:单捕获/尾 ⇒ `["field"]`;tuple ⇒ `["field#0","field#1",…]`。
fn slot_group_names(s: &SlotField) -> Vec<String> {
    let base = s.ident.to_string();
    if s.tuple_elems.is_empty() {
        vec![base]
    } else {
        (0..s.tuple_elems.len()).map(|i| format!("{base}#{i}")).collect()
    }
}

// ───────────────────────── matcher!{} 函数式糖 ─────────────────────────

/// `matcher! { "查看", <field>: <ty> = union("a","b"), "榜", sep = "…", usage = "…" }`：
/// 与 `#[derive(Slots)]` 同款序列(字面块 + 捕获块),求值为一个 `Matcher`。
pub(crate) struct MatcherMacro {
    items: Vec<MatcherItem>,
    sep: std::option::Option<String>,
    usage: std::option::Option<String>,
}

enum MatcherItem {
    Lit(String),
    /// 装箱:`Field` 比 `Lit` 大得多(含 `Type`),箱起来避免 `clippy::large_enum_variant`。
    Field(Box<MatcherField>),
}

/// `matcher!{}` 里一个捕获块字段(就地声明类型与来源)。
struct MatcherField {
    ident: Ident,
    ty: Type,
    src: SlotSrc,
}

impl Parse for MatcherMacro {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut items = Vec::new();
        let mut sep = None;
        let mut usage = None;
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            if key == "lit" {
                // 固定块 lit("…")。
                let content;
                syn::parenthesized!(content in input);
                items.push(MatcherItem::Lit(content.parse::<LitStr>()?.value()));
            } else if input.peek(Token![=]) {
                // sep = "…" / usage = "…"
                let _: Token![=] = input.parse()?;
                let s: LitStr = input.parse()?;
                match key.to_string().as_str() {
                    "sep" => sep = Some(s.value()),
                    "usage" => usage = Some(s.value()),
                    other => return Err(Error::new(key.span(), format!("unknown matcher! key `{other}`"))),
                }
            } else {
                // <field>: <ty> = <src>
                let _: Token![:] = input.parse()?;
                let ty: Type = input.parse()?;
                let _: Token![=] = input.parse()?;
                let src = parse_matcher_src(input)?;
                items.push(MatcherItem::Field(Box::new(MatcherField { ident: key, ty, src })));
            }
            if input.peek(Token![,]) {
                let _: Token![,] = input.parse()?;
            }
        }
        Ok(MatcherMacro { items, sep, usage })
    }
}

/// 解析 `re("…")` / `union("a","b")` / `tail` 三种 src 写法。
fn parse_matcher_src(input: ParseStream) -> syn::Result<SlotSrc> {
    let kind: Ident = input.parse()?;
    if kind == "tail" {
        Ok(SlotSrc::Tail)
    } else if kind == "re" {
        let content;
        syn::parenthesized!(content in input);
        Ok(SlotSrc::Re(content.parse::<LitStr>()?.value()))
    } else if kind == "union" {
        let content;
        syn::parenthesized!(content in input);
        let alts = Punctuated::<LitStr, Token![,]>::parse_terminated(&content)?;
        Ok(SlotSrc::Union(alts.iter().map(LitStr::value).collect()))
    } else {
        Err(Error::new(kind.span(), "expected re(\"..\") / union(\"a\",\"b\") / tail"))
    }
}

/// `matcher!{}` 展开:把序列合成一个内联 `#[derive(Slots)]` 等价结构体(字段 = 捕获块,
/// `#[slots(...)]` = 区块序列),借 `expand_slots` 的 codegen,求值为 `<Struct>::matcher()`。
pub(crate) fn expand_matcher_macro(m: MatcherMacro) -> proc_macro2::TokenStream {
    let nc = nagisa_core_root();
    let ty_ident = format_ident!("__NagiMatcher");

    // 字段声明(仅捕获块) + #[slot(..)] 属性 + #[slots(序列)]。
    let mut field_decls: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut field_attrs: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut seq_items: Vec<proc_macro2::TokenStream> = Vec::new();
    for it in &m.items {
        match it {
            MatcherItem::Lit(text) => {
                let lit = LitStr::new(text, Span::call_site());
                seq_items.push(quote! { lit(#lit) });
            }
            MatcherItem::Field(f) => {
                let MatcherField { ident, ty, src } = f.as_ref();
                let (inner_ty, optional) = match option_inner(ty) {
                    Some(inner) => (inner.clone(), true),
                    None => (ty.clone(), false),
                };
                let ty_tok = if optional {
                    quote! { ::std::option::Option<#inner_ty> }
                } else {
                    quote! { #inner_ty }
                };
                let slot_attr = match src {
                    SlotSrc::Re(re) => {
                        let lit = LitStr::new(re, Span::call_site());
                        quote! { #[slot(re = #lit)] }
                    }
                    SlotSrc::Union(alts) => {
                        let lits = alts.iter().map(|a| LitStr::new(a, Span::call_site()));
                        quote! { #[slot(union = [ #( #lits ),* ])] }
                    }
                    SlotSrc::Tail => quote! { #[slot(tail)] },
                };
                field_decls.push(quote! { #ident: #ty_tok });
                field_attrs.push(quote! { #slot_attr #ident: #ty_tok });
                seq_items.push(quote! { #ident });
            }
        }
    }
    if let Some(s) = &m.sep {
        let lit = LitStr::new(s, Span::call_site());
        seq_items.push(quote! { sep = #lit });
    }
    if let Some(u) = &m.usage {
        let lit = LitStr::new(u, Span::call_site());
        seq_items.push(quote! { usage = #lit });
    }

    let synth: DeriveInput = syn::parse_quote! {
        #[slots( #( #seq_items ),* )]
        struct #ty_ident { #( #field_attrs ),* }
    };
    let impl_ts = match expand_slots(synth) {
        Ok(ts) => ts,
        Err(e) => return e.to_compile_error(),
    };

    quote! {
        {
            #[allow(dead_code)]
            struct #ty_ident { #( #field_decls ),* }
            #impl_ts
            <#ty_ident as #nc::FromSlots>::matcher()
        }
    }
}
