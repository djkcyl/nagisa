//! `#[derive(Slots)]` 与 `matcher!{}` 的**解析 + 展开层**:命令头 + 命名类型化正则槽。
//!
//! 职责:`expand_slots` 把结构体的 `#[slots(full=/usage=)]` 头与各字段的
//! `#[slot(re=/union=/tail)]` 槽,展开成 `FromSlots` 实现——`matcher()` 拼出一串
//! `SlotSpec`(头字面量 + 各槽正则,槽间补 `\s*` 分隔)产出 `Matcher`,`from_slots()`
//! 从 `NamedCaptures` 逐字段投影(单捕获经 `SlotValue`、tuple 多组各经 `FromArg`、
//! tail 经 `FromTailText`)。`matcher!{}` 是同款 codegen 的内联入口:`expand_matcher_macro`
//! 把 `MatcherMacro` 合成一个等价 `DeriveInput` 喂回 `expand_slots`,再求值为
//! `<合成类型>::matcher()`。`SlotField` / `SlotSrc` / `slot_group_names` 是内部表示与助手。
//!
//! 协作:复用 [`crate::args_derive`] 的 `option_inner`(可选性)与 [`crate::attrs`] 的
//! `expr_to_litstr`(union 数组元素)。生成的代码引用引擎的 `Matcher` / `SlotSpec` /
//! `NamedCaptures` / `SlotValue` / `FromArg` 等,路径根经 [`crate::nagisa_core_root`] 解析。

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

/// 解析后的一个 slot 字段。
struct SlotField {
    ident: Ident,
    /// 去掉 `Option` 后的类型(可选性单独记 `optional`)。
    inner_ty: Type,
    optional: bool,
    src: SlotSrc,
    /// 若 `inner_ty` 是 tuple,各元素类型(供多组 codegen);否则空。
    tuple_elems: Vec<Type>,
}

/// 若 `ty` 是 tuple `(A, B, …)` 返回其元素类型;否则 `None`。
fn tuple_elems(ty: &Type) -> std::option::Option<Vec<Type>> {
    match ty {
        Type::Tuple(t) if !t.elems.is_empty() => Some(t.elems.iter().cloned().collect()),
        _ => None,
    }
}

/// 头与首槽之间补的分隔片段(容忍头后的空白)。
const SLOT_SEP: &str = r"\s*";

pub(crate) fn expand_slots(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let nc = nagisa_core_root();
    let struct_ident = &input.ident;

    // —— 结构体级 `#[slots(full = "…", usage = "…")]`。——
    let mut full_head: std::option::Option<String> = None;
    let mut usage: std::option::Option<String> = None;
    for attr in &input.attrs {
        if !attr.path().is_ident("slots") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("full") {
                let s: LitStr = meta.value()?.parse()?;
                full_head = Some(s.value());
            } else if meta.path.is_ident("usage") {
                let s: LitStr = meta.value()?.parse()?;
                usage = Some(s.value());
            } else {
                return Err(meta.error("unknown #[slots(..)] key (use full=/usage=)"));
            }
            Ok(())
        })?;
    }

    let fields = match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(named) => &named.named,
            _ => {
                return Err(Error::new_spanned(
                    struct_ident,
                    "#[derive(Slots)] requires a struct with named fields",
                ))
            }
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
        for attr in &field.attrs {
            if !attr.path().is_ident("slot") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("re") {
                    let s: LitStr = meta.value()?.parse()?;
                    set_slot_src(&mut src, &meta, SlotSrc::Re(s.value()))
                } else if meta.path.is_ident("union") {
                    // union = ["a","b"] 或单字符串。
                    let arr: ExprArray = meta.value()?.parse()?;
                    let mut alts = Vec::with_capacity(arr.elems.len());
                    for e in &arr.elems {
                        alts.push(expr_to_litstr(e)?.value());
                    }
                    set_slot_src(&mut src, &meta, SlotSrc::Union(alts))
                } else if meta.path.is_ident("tail") {
                    set_slot_src(&mut src, &meta, SlotSrc::Tail)
                } else {
                    Err(meta.error("unknown #[slot(..)] key (use re=/union=/tail)"))
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
        // tail 只能是 String 或 Tail<…>(单值),不能是 tuple。
        if matches!(src, SlotSrc::Tail) && !tuple_elems.is_empty() {
            return Err(Error::new_spanned(&field.ident, "#[slot(tail)] cannot be a tuple"));
        }

        slots.push(SlotField { ident, inner_ty, optional, src, tuple_elems });
    }

    // 重名槽名 = 编译错误。
    let mut seen = std::collections::HashSet::new();
    for s in &slots {
        if !seen.insert(s.ident.to_string()) {
            return Err(Error::new_spanned(&s.ident, "duplicate slot name"));
        }
    }

    // —— 生成 SlotSpec 列表(供 matcher())。——
    let mut spec_exprs: Vec<proc_macro2::TokenStream> = Vec::new();
    if let Some(head) = &full_head {
        let lit = LitStr::new(head, Span::call_site());
        spec_exprs.push(quote! {
            #nc::SlotSpec::literal(#nc::regex_escape(#lit))
        });
    }
    for s in &slots {
        let names = slot_group_names(s);
        let name_lits = names.iter().map(|n| LitStr::new(n, Span::call_site()));
        // 槽前补分隔片段(头/前一槽与本槽之间的空白)。
        let sep = LitStr::new(SLOT_SEP, Span::call_site());
        let body = match &s.src {
            SlotSrc::Re(re) => {
                let re_lit = LitStr::new(re, Span::call_site());
                quote! { ::std::format!("{}{}", #sep, #re_lit) }
            }
            SlotSrc::Union(alts) => {
                // (a|b),各臂转义。
                let escaped: Vec<LitStr> =
                    alts.iter().map(|a| LitStr::new(a, Span::call_site())).collect();
                quote! {
                    ::std::format!("{}({})", #sep,
                        [ #( #nc::regex_escape(#escaped) ),* ].join("|"))
                }
            }
            SlotSrc::Tail => {
                quote! { ::std::format!("{}([\\s\\S]*)", #sep) }
            }
        };
        let optional = s.optional;
        spec_exprs.push(quote! {
            #nc::SlotSpec {
                names: ::std::vec![ #( ::std::borrow::Cow::Borrowed(#name_lits) ),* ],
                src: #body,
                optional: #optional,
                flank: #nc::Flank::Whole,
            }
        });
    }

    // —— 生成 from_slots:逐字段从 named_captures 投影。——
    let mut field_builds: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut field_idents: Vec<Ident> = Vec::new();
    for s in &slots {
        let ident = &s.ident;
        field_idents.push(ident.clone());
        let fs = LitStr::new(&ident.to_string(), Span::call_site());
        let names = slot_group_names(s);
        let name_lits: Vec<LitStr> =
            names.iter().map(|n| LitStr::new(n, Span::call_site())).collect();

        // 取一个命名组的 Option<String>(缺组 ⇒ None)。
        let get = |nm: &LitStr| {
            quote! {
                __caps.get(#nm).and_then(|__o| __o.as_ref())
            }
        };

        let build = if matches!(s.src, SlotSrc::Tail) {
            // 尾:from_tail_text(捕获文本或空串)。
            let nm = &name_lits[0];
            let g = get(nm);
            let inner_ty = &s.inner_ty;
            let raw = quote! {
                <#inner_ty as #nc::FromTailText>::from_tail_text(
                    #g.map(|__s| __s.as_str()).unwrap_or("")
                )
            };
            if s.optional {
                // Option<tail>:缺省(无任何文本) ⇒ None;否则 Some(payload)。
                quote! {
                    let #ident = match #g {
                        ::std::option::Option::Some(__t) if !__t.is_empty() =>
                            ::std::option::Option::Some(
                                <#inner_ty as #nc::FromTailText>::from_tail_text(__t.as_str())
                            ),
                        _ => ::std::option::Option::None,
                    };
                }
            } else {
                quote! { let #ident = #raw; }
            }
        } else if !s.tuple_elems.is_empty() {
            // tuple:N 个内组,各经 FromArg。present ⇒ (a,b);缺省(可选)⇒ None。
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
                // 缺省判定:看第一个内组是否命中(其一缺 ⇒ 整槽缺)。
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
            // 单捕获:经 SlotValue(FromArg 空白桥)。
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
                            ::std::option::Option::None =>
                                return ::std::result::Result::Err(#nc::ArgError::Missing(#fs)),
                        };
                        #parse_some
                    };
                }
            }
        };
        field_builds.push(build);
    }

    let usage_fn = match &usage {
        Some(u) => {
            let lit = LitStr::new(u, Span::call_site());
            quote! {
                fn usage() -> ::std::option::Option<&'static str> {
                    ::std::option::Option::Some(#lit)
                }
            }
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
            #usage_fn
        }
    })
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

/// `matcher! { full = "…", <usage = "…">, <field>: <ty> = re("…")|union("a","b")|tail }`。
pub(crate) struct MatcherMacro {
    full: std::option::Option<String>,
    usage: std::option::Option<String>,
    fields: Vec<MatcherMacroField>,
}

struct MatcherMacroField {
    ident: Ident,
    ty: Type,
    src: SlotSrc,
}

impl Parse for MatcherMacro {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut full = None;
        let mut usage = None;
        let mut fields = Vec::new();
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            if key == "full" || key == "usage" {
                let _: Token![=] = input.parse()?;
                let s: LitStr = input.parse()?;
                if key == "full" {
                    full = Some(s.value());
                } else {
                    usage = Some(s.value());
                }
            } else {
                // <field>: <ty> = <src>
                let _: Token![:] = input.parse()?;
                let ty: Type = input.parse()?;
                let _: Token![=] = input.parse()?;
                let src = parse_matcher_src(input)?;
                fields.push(MatcherMacroField { ident: key, ty, src });
            }
            if input.peek(Token![,]) {
                let _: Token![,] = input.parse()?;
            }
        }
        Ok(MatcherMacro { full, usage, fields })
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
        let s: LitStr = content.parse()?;
        Ok(SlotSrc::Re(s.value()))
    } else if kind == "union" {
        let content;
        syn::parenthesized!(content in input);
        let alts = Punctuated::<LitStr, Token![,]>::parse_terminated(&content)?;
        Ok(SlotSrc::Union(alts.iter().map(LitStr::value).collect()))
    } else {
        Err(Error::new(kind.span(), "expected re(\"..\") / union(\"a\",\"b\") / tail"))
    }
}

/// `matcher!{}` 展开:合成一个内联结构体 + `#[derive(Slots)]` 等价的 `FromSlots`,
/// 并求值为「`<该类型>::matcher()`」。规范路径仍是 `#[derive(Slots)]`。
pub(crate) fn expand_matcher_macro(m: MatcherMacro) -> proc_macro2::TokenStream {
    // 复用 expand_slots：先构造 SlotField(供 field_decls),再合成等价 DeriveInput 喂回 expand_slots 借其 codegen。
    let slots: Vec<SlotField> = m
        .fields
        .into_iter()
        .map(|f| {
            let (inner_ty, optional) = match option_inner(&f.ty) {
                Some(inner) => (inner.clone(), true),
                None => (f.ty.clone(), false),
            };
            let tuple_elems = tuple_elems(&inner_ty).unwrap_or_default();
            SlotField { ident: f.ident, inner_ty, optional, src: f.src, tuple_elems }
        })
        .collect();

    let ty_ident = format_ident!("__NagiMatcher");
    let field_decls: Vec<proc_macro2::TokenStream> = slots
        .iter()
        .map(|s| {
            let id = &s.ident;
            let inner = &s.inner_ty;
            if s.optional {
                quote! { #id: ::std::option::Option<#inner> }
            } else {
                quote! { #id: #inner }
            }
        })
        .collect();

    // 借 expand_slots 的 codegen:把合成结构体喂回去。
    let synth: DeriveInput = {
        let full_attr = match &m.full {
            Some(h) => {
                let lit = LitStr::new(h, Span::call_site());
                quote! { #[slots(full = #lit)] }
            }
            None => quote! {},
        };
        let usage_attr = match &m.usage {
            Some(u) => {
                let lit = LitStr::new(u, Span::call_site());
                quote! { #[slots(usage = #lit)] }
            }
            None => quote! {},
        };
        let slot_attrs: Vec<proc_macro2::TokenStream> = slots
            .iter()
            .map(|s| {
                let id = &s.ident;
                let inner = &s.inner_ty;
                let ty_tok = if s.optional {
                    quote! { ::std::option::Option<#inner> }
                } else {
                    quote! { #inner }
                };
                let attr = match &s.src {
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
                quote! { #attr #id: #ty_tok }
            })
            .collect();
        syn::parse_quote! {
            #full_attr
            #usage_attr
            struct #ty_ident { #( #slot_attrs ),* }
        }
    };

    let impl_ts = match expand_slots(synth) {
        Ok(ts) => ts,
        Err(e) => return e.to_compile_error(),
    };
    let nc = nagisa_core_root();

    // 合成一个内联结构体 + 其 `FromSlots`,求值为 `<Struct>::matcher()`(返回 `Matcher`)。
    // `matcher!{}` 因此是纯头匹配器糖;`Slots<T>` 提取器仍需具名的 `#[derive(Slots)]` 类型。
    quote! {
        {
            #[allow(dead_code)]
            struct #ty_ident { #( #field_decls ),* }
            #impl_ts
            <#ty_ident as #nc::FromSlots>::matcher()
        }
    }
}
