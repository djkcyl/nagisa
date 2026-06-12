//! `#[derive(Args)]` / `#[derive(ArgEnum)]` 的**解析 + 展开层**。
//!
//! 职责:`expand_args` 把带具名字段的结构体连同其 `#[arg(..)]` 字段属性,展开成
//! `ParseArgs::parse_args` 的实现——一个在消息**段流**(`ArgToken`)上扫描旗标/选项、
//! 再按声明顺序构建各字段(文本位置/rest/选项/旗标、元素位置/rest、`at_or_id`)的解析器。
//! `expand_arg_enum` 把无字段枚举展开成按变体名小写匹配的 `FromArg`。`ArgRole` /
//! `ArgField` / `ElemKind` 记录每字段角色,`text_builder` / `or_pattern` 是 codegen 助手。
//!
//! 协作:`option_inner`(判断 `Option<T>` 并取内层)是本模块的公共助手,也供
//! [`crate::slots`] 复用。生成的代码引用引擎的 `FromArg` / `ArgToken` / `ArgError` /
//! `seg_as_*` 段助手,路径根经 [`crate::nagisa_core_root`] 解析。

use proc_macro2::Span;
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Error, Fields, GenericArgument, Ident, LitChar, LitStr, PathArguments, Token, Type};

use crate::nagisa_core_root;

/// 元素槽类型。
#[derive(Clone, Copy)]
enum ElemKind {
    Image,
    Record,
    Video,
    At,
    Reply,
    Face,
}

impl ElemKind {
    fn from_ident(s: &str) -> std::option::Option<ElemKind> {
        std::option::Option::Some(match s {
            "image" => ElemKind::Image,
            "record" => ElemKind::Record,
            "video" => ElemKind::Video,
            "at" => ElemKind::At,
            "reply" => ElemKind::Reply,
            "face" => ElemKind::Face,
            _ => return std::option::Option::None,
        })
    }
    fn helper(self) -> Ident {
        let n = match self {
            ElemKind::Image => "seg_as_image",
            ElemKind::Record => "seg_as_record",
            ElemKind::Video => "seg_as_video",
            ElemKind::At => "seg_as_at",
            ElemKind::Reply => "seg_as_reply",
            ElemKind::Face => "seg_as_face",
        };
        format_ident!("{}", n)
    }
}

enum ArgRole {
    TextPositional,
    TextRest,
    TextOption,
    Flag,
    ElementPositional(ElemKind),
    ElementRest(ElemKind),
    /// `#[arg(at_or_id)]`:一个 @ 提及元素,缺则取下一个文本词 → `Uin`(@ 或裸号二选一)。
    AtOrId,
}

struct ArgField {
    ident: Ident,
    ty: Type,
    role: ArgRole,
    match_lits: Vec<LitStr>,
    default: std::option::Option<String>,
    /// `#[arg(rest, raw)]`:文本 rest 取原文(保真空白/换行),而非按词重拼。
    raw: bool,
    /// `#[arg(name="…")]`:help 里的显示名(缺则字段标识符)。
    name: std::option::Option<String>,
    /// `#[arg(desc="…")]`:help 里的一句话说明。
    desc: std::option::Option<String>,
    /// 短旗标字符（仅旗标/选项有），供 `ArgSpec`。
    short: std::option::Option<char>,
    /// 长旗标名（仅旗标/选项有，显式 `long="…"` 或字段名），供 `ArgSpec`。
    long_name: std::option::Option<String>,
}

pub(crate) fn expand_args(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let nc = nagisa_core_root();
    let struct_ident = &input.ident;
    let fields = match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(named) => &named.named,
            _ => return Err(Error::new_spanned(struct_ident, "#[derive(Args)] requires a struct with named fields")),
        },
        _ => return Err(Error::new_spanned(struct_ident, "#[derive(Args)] only supports structs")),
    };

    let mut parsed: Vec<ArgField> = Vec::new();
    for field in fields {
        let ident = field.ident.clone().expect("named field");
        let ty = field.ty.clone();
        let mut is_flag = false;
        let mut is_rest = false;
        let mut is_raw = false;
        let mut long: std::option::Option<std::option::Option<String>> = None;
        let mut short: std::option::Option<char> = None;
        let mut default: std::option::Option<String> = None;
        let mut elem: std::option::Option<ElemKind> = None;
        let mut is_at_or_id = false;
        let mut name: std::option::Option<String> = None;
        let mut desc: std::option::Option<String> = None;

        for attr in &field.attrs {
            if !attr.path().is_ident("arg") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                let p = &meta.path;
                if p.is_ident("flag") {
                    is_flag = true;
                } else if p.is_ident("rest") {
                    is_rest = true;
                } else if p.is_ident("raw") {
                    is_raw = true;
                } else if p.is_ident("positional") {
                    // 默认行为
                } else if p.is_ident("long") {
                    if meta.input.peek(Token![=]) {
                        let s: LitStr = meta.value()?.parse()?;
                        long = Some(Some(s.value()));
                    } else {
                        long = Some(None);
                    }
                } else if p.is_ident("short") {
                    let c: LitChar = meta.value()?.parse()?;
                    short = Some(c.value());
                } else if p.is_ident("default") {
                    let s: LitStr = meta.value()?.parse()?;
                    default = Some(s.value());
                } else if p.is_ident("name") {
                    // help 显示名(中文友好),不影响解析。
                    let s: LitStr = meta.value()?.parse()?;
                    name = Some(s.value());
                } else if p.is_ident("desc") {
                    // help 一句话说明,不影响解析。
                    let s: LitStr = meta.value()?.parse()?;
                    desc = Some(s.value());
                } else if p.is_ident("at_or_id") {
                    // `Uin` 字段:取一个 @ 提及元素,没有则取下一个文本词当号(兼容 "@123"/"123")。
                    is_at_or_id = true;
                } else if let Some(k) = p.get_ident().and_then(|i| ElemKind::from_ident(&i.to_string())) {
                    elem = Some(k);
                } else {
                    return Err(meta.error("unknown #[arg(..)] key"));
                }
                Ok(())
            })?;
        }

        let mut match_lits: Vec<LitStr> = Vec::new();
        // 旗标/选项的长名:显式 `long="…"` 或退回字段名;非旗标/选项为 None(供 ArgSpec)。
        let long_name = if is_flag || long.is_some() {
            std::option::Option::Some(match &long {
                Some(Some(n)) => n.clone(),
                _ => ident.to_string(),
            })
        } else {
            std::option::Option::None
        };
        if let Some(n) = &long_name {
            match_lits.push(LitStr::new(&format!("--{n}"), Span::call_site()));
            if let Some(c) = short {
                match_lits.push(LitStr::new(&format!("-{c}"), Span::call_site()));
            }
        }

        let role = if is_at_or_id {
            ArgRole::AtOrId
        } else if let Some(k) = elem {
            if is_rest {
                ArgRole::ElementRest(k)
            } else {
                ArgRole::ElementPositional(k)
            }
        } else if is_flag {
            ArgRole::Flag
        } else if long.is_some() {
            ArgRole::TextOption
        } else if is_rest {
            ArgRole::TextRest
        } else {
            ArgRole::TextPositional
        };

        parsed.push(ArgField { ident, ty, role, match_lits, default, raw: is_raw, name, desc, short, long_name });
    }

    // —— 扫描器:变量声明 + Word 匹配臂(选项/旗标)。——
    let mut decls = Vec::new();
    let mut scan_arms = Vec::new();
    for f in &parsed {
        match f.role {
            ArgRole::Flag => {
                let var = format_ident!("__flag_{}", f.ident);
                decls.push(quote! { let mut #var = false; });
                let pat = or_pattern(&f.match_lits);
                // 旗标词被消费 → 计入 __flag_words(供 `raw` rest 跳过前导旗标)。
                scan_arms.push(quote! { #pat => { #var = true; __flag_words += 1; } });
            }
            ArgRole::TextOption => {
                let var = format_ident!("__opt_{}", f.ident);
                decls.push(quote! {
                    let mut #var: ::std::option::Option<::std::string::String> = ::std::option::Option::None;
                });
                let pat = or_pattern(&f.match_lits);
                scan_arms.push(quote! {
                    #pat => {
                        __flag_words += 1; // 选项名
                        // 取下一个 Word 作为值,但拒绝 `--` 开头的(那是另一个选项;
                        // 单 `-` 开头允许,以兼容负数值如 `--remaining -1`)。
                        if let ::std::option::Option::Some(#nc::ArgToken::Word(__v)) =
                            __tokens.get(__i + 1).copied()
                        {
                            if !__v.starts_with("--") {
                                #var = ::std::option::Option::Some(::std::string::String::from(__v));
                                __consumed[__i + 1] = true;
                                __flag_words += 1; // 选项值
                            }
                        }
                    }
                });
            }
            _ => {}
        }
    }

    // 有 `#[arg(rest, raw)]` 时:旗标只认前导(遇到第一个正文词就停止),让正文里的
    // `-x` 当字面文本(旗标只在前导处解析,首个正文词之后的 `-x` 一律视为字面文本)。
    let has_raw_rest = parsed.iter().any(|f| matches!(f.role, ArgRole::TextRest) && f.raw);
    let lead_only = has_raw_rest && !scan_arms.is_empty();
    let flags_done_decl = if lead_only {
        quote! { let mut __flags_done = false; }
    } else {
        quote! {}
    };
    let word_body = if scan_arms.is_empty() {
        quote! { __pos_words.push(__w); }
    } else if lead_only {
        quote! {
            if __flags_done {
                __pos_words.push(__w);
            } else {
                match __w {
                    #(#scan_arms)*
                    _ => { __pos_words.push(__w); __flags_done = true; }
                }
            }
        }
    } else {
        quote! {
            match __w {
                #(#scan_arms)*
                _ => __pos_words.push(__w),
            }
        }
    };

    // —— 字段构建(按声明顺序)。——
    let mut builders = Vec::new();
    let mut field_idents = Vec::new();
    for f in &parsed {
        let ident = &f.ident;
        field_idents.push(ident.clone());
        let fs = LitStr::new(&ident.to_string(), Span::call_site());
        let ty = &f.ty;
        match f.role {
            ArgRole::Flag => {
                let var = format_ident!("__flag_{}", ident);
                builders.push(quote! { let #ident = #var; });
            }
            ArgRole::TextRest if f.raw => {
                // 正文保真:从原始 args 文本里跳过「前导旗标词 + 此前已消费的位置词」,
                // 余下原文逐字保留(内部空白/换行不丢)。须放在所有文本位置参之后。
                builders.push(quote! {
                    let #ident: ::std::string::String = #nc::args::skip_words(
                        __raw_text,
                        __flag_words + (__pos_total - __words.len()),
                    );
                });
            }
            ArgRole::TextRest => {
                builders.push(quote! {
                    let #ident: ::std::string::String =
                        __words.by_ref().collect::<::std::vec::Vec<&str>>().join(" ");
                });
            }
            ArgRole::TextPositional => {
                builders.push(text_builder(&nc, ident, &fs, ty, &f.default, quote! { __words.next() }, false));
            }
            ArgRole::TextOption => {
                let var = format_ident!("__opt_{}", ident);
                builders.push(text_builder(&nc, ident, &fs, ty, &f.default, quote! { #var }, true));
            }
            ArgRole::ElementPositional(k) => {
                let helper = k.helper();
                let optional = option_inner(ty).is_some();
                let result = if optional {
                    quote! { __found }
                } else {
                    quote! { match __found {
                        ::std::option::Option::Some(__v) => __v,
                        ::std::option::Option::None =>
                            return ::std::result::Result::Err(#nc::ArgError::Missing(#fs)),
                    } }
                };
                builders.push(quote! {
                    let #ident = {
                        let mut __found = ::std::option::Option::None;
                        let mut __k = 0usize;
                        while __k < __tokens.len() {
                            if !__consumed[__k] {
                                if let #nc::ArgToken::Element(__seg) = __tokens[__k] {
                                    if let ::std::option::Option::Some(__v) =
                                        #nc::args::#helper(__seg)
                                    {
                                        __consumed[__k] = true;
                                        __found = ::std::option::Option::Some(__v);
                                        break;
                                    }
                                }
                            }
                            __k += 1;
                        }
                        #result
                    };
                });
            }
            ArgRole::ElementRest(k) => {
                let helper = k.helper();
                builders.push(quote! {
                    let #ident = {
                        let mut __v = ::std::vec::Vec::new();
                        let mut __k = 0usize;
                        while __k < __tokens.len() {
                            if !__consumed[__k] {
                                if let #nc::ArgToken::Element(__seg) = __tokens[__k] {
                                    if let ::std::option::Option::Some(__x) =
                                        #nc::args::#helper(__seg)
                                    {
                                        __consumed[__k] = true;
                                        __v.push(__x);
                                    }
                                }
                            }
                            __k += 1;
                        }
                        __v
                    };
                });
            }
            ArgRole::AtOrId => {
                // 字段须是 `Uin` / `Option<Uin>`:内层类型经 from_arg 解析裸号,与 seg_as_at 同型。
                let inner = option_inner(ty).unwrap_or(ty);
                let result = if option_inner(ty).is_some() {
                    quote! { __val }
                } else {
                    quote! { match __val {
                        ::std::option::Option::Some(__v) => __v,
                        ::std::option::Option::None =>
                            return ::std::result::Result::Err(#nc::ArgError::Missing(#fs)),
                    } }
                };
                builders.push(quote! {
                    let #ident = {
                        // 先找一个未消费的 @ 元素;找到即用,且**不占**文本词(让后续位置参对齐)。
                        let mut __at = ::std::option::Option::None;
                        let mut __k = 0usize;
                        while __k < __tokens.len() {
                            if !__consumed[__k] {
                                if let #nc::ArgToken::Element(__seg) = __tokens[__k] {
                                    if let ::std::option::Option::Some(__v) =
                                        #nc::args::seg_as_at(__seg)
                                    {
                                        __consumed[__k] = true;
                                        __at = ::std::option::Option::Some(__v);
                                        break;
                                    }
                                }
                            }
                            __k += 1;
                        }
                        // 没 @ 就取下一个文本词解析成号(兼容 "@123"/"123")。
                        let __val = match __at {
                            ::std::option::Option::Some(__u) => ::std::option::Option::Some(__u),
                            ::std::option::Option::None => __words
                                .next()
                                .and_then(|__w| <#inner as #nc::FromArg>::from_arg(__w)),
                        };
                        #result
                    };
                });
            }
        }
    }

    // —— `ArgsMeta::SPECS`:每字段一条 `ArgSpec`,供 help 自动生成用法说明。——
    let specs: Vec<proc_macro2::TokenStream> = parsed
        .iter()
        .map(|f| {
            let disp = f.name.clone().unwrap_or_else(|| f.ident.to_string());
            let name_lit = LitStr::new(&disp, Span::call_site());
            let kind = match &f.role {
                ArgRole::Flag => quote! { #nc::ArgKind::Flag },
                ArgRole::TextOption => quote! { #nc::ArgKind::Opt },
                ArgRole::TextPositional => quote! { #nc::ArgKind::Positional },
                ArgRole::TextRest => quote! { #nc::ArgKind::Rest },
                ArgRole::AtOrId => quote! { #nc::ArgKind::AtOrId },
                ArgRole::ElementPositional(_) | ArgRole::ElementRest(_) => {
                    quote! { #nc::ArgKind::Element }
                }
            };
            let short_lit = LitStr::new(&f.short.map(|c| c.to_string()).unwrap_or_default(), Span::call_site());
            let long_lit = LitStr::new(f.long_name.as_deref().unwrap_or(""), Span::call_site());
            // 必填 = 非 `Option` 且无 `default` 的位置 / 元素位置 / at_or_id;旗标 / 选项 / rest / 元素rest 恒非必填。
            let required = match &f.role {
                ArgRole::Flag | ArgRole::TextOption | ArgRole::TextRest | ArgRole::ElementRest(_) => false,
                _ => option_inner(&f.ty).is_none() && f.default.is_none(),
            };
            let default_lit = LitStr::new(f.default.as_deref().unwrap_or(""), Span::call_site());
            let desc_lit = LitStr::new(f.desc.as_deref().unwrap_or(""), Span::call_site());
            quote! {
                #nc::ArgSpec {
                    name: #name_lit,
                    kind: #kind,
                    short: #short_lit,
                    long: #long_lit,
                    required: #required,
                    default: #default_lit,
                    desc: #desc_lit,
                }
            }
        })
        .collect();

    Ok(quote! {
        impl #nc::ParseArgs for #struct_ident {
            fn parse_args(__tokens: &[#nc::ArgToken<'_>], __raw_text: &str)
                -> ::std::result::Result<Self, #nc::ArgError>
            {
                let _ = __raw_text; // 仅 `#[arg(rest, raw)]` 用到
                let mut __consumed = ::std::vec![false; __tokens.len()];
                let mut __pos_words: ::std::vec::Vec<&str> = ::std::vec::Vec::new();
                #[allow(unused)]
                let mut __flag_words = 0usize;
                #flags_done_decl
                #(#decls)*
                let mut __i = 0usize;
                while __i < __tokens.len() {
                    if !__consumed[__i] {
                        if let #nc::ArgToken::Word(__w) = __tokens[__i] {
                            #word_body
                        }
                    }
                    __i += 1;
                }
                #[allow(unused)]
                let __pos_total = __pos_words.len();
                let mut __words = __pos_words.into_iter();
                #(#builders)*
                ::std::result::Result::Ok(#struct_ident { #(#field_idents),* })
            }
        }

        impl #nc::ArgsMeta for #struct_ident {
            const SPECS: &'static [#nc::ArgSpec] = &[ #(#specs),* ];
        }
    })
}

/// 文本字段构建:source 是 `__words.next()`(位置)或 `#opt_var`(选项)——都产出 `Option<&str>`/`Option<String>`。
/// `is_opt_source=true` 表示 source 已是 `Option<String>`(选项),需 `&__raw`;否则是 `Option<&str>`(位置)。
fn text_builder(
    nc: &proc_macro2::TokenStream,
    ident: &Ident,
    fs: &LitStr,
    ty: &Type,
    default: &std::option::Option<String>,
    source: proc_macro2::TokenStream,
    is_opt_source: bool,
) -> proc_macro2::TokenStream {
    // 把原始字符串引用统一成 `&str`。
    let as_ref = if is_opt_source {
        quote! { __raw.as_str() }
    } else {
        quote! { __raw }
    };
    let parse_some = |inner: &Type| {
        quote! {
            match <#inner as #nc::FromArg>::from_arg(#as_ref) {
                ::std::option::Option::Some(__v) => __v,
                ::std::option::Option::None => return ::std::result::Result::Err(
                    #nc::ArgError::Parse {
                        field: #fs,
                        value: ::std::string::String::from(#as_ref),
                        expected: <#inner as #nc::FromArg>::TYPE_NAME,
                    }),
            }
        }
    };

    if let Some(inner) = option_inner(ty) {
        let parse = parse_some(inner);
        quote! {
            let #ident = match #source {
                ::std::option::Option::Some(__raw) => ::std::option::Option::Some(#parse),
                ::std::option::Option::None => ::std::option::Option::None,
            };
        }
    } else if let Some(def) = default {
        let def_lit = LitStr::new(def, Span::call_site());
        let parse = parse_some(ty);
        // 缺省分支的 __raw 类型须与 `as_ref` 一致:选项源(String)用 String,位置源(&str)用 &str。
        let default_bind = if is_opt_source {
            quote! { let __raw = ::std::string::String::from(#def_lit); }
        } else {
            quote! { let __raw: &str = #def_lit; }
        };
        quote! {
            let #ident = match #source {
                ::std::option::Option::Some(__raw) => #parse,
                ::std::option::Option::None => {
                    #default_bind
                    #parse
                }
            };
        }
    } else {
        let parse = parse_some(ty);
        quote! {
            let #ident = {
                let __raw = match #source {
                    ::std::option::Option::Some(__r) => __r,
                    ::std::option::Option::None =>
                        return ::std::result::Result::Err(#nc::ArgError::Missing(#fs)),
                };
                #parse
            };
        }
    }
}

/// `"--a" | "-b"` 模式。
fn or_pattern(lits: &[LitStr]) -> proc_macro2::TokenStream {
    quote! { #(#lits)|* }
}

/// 若 `ty` 是 `Option<Inner>` 返回 `Inner`。
pub(crate) fn option_inner(ty: &Type) -> std::option::Option<&Type> {
    let Type::Path(p) = ty else { return None };
    let seg = p.path.segments.last()?;
    if seg.ident != "Option" {
        return None;
    }
    let PathArguments::AngleBracketed(ab) = &seg.arguments else { return None };
    match ab.args.first()? {
        GenericArgument::Type(t) => Some(t),
        _ => None,
    }
}

// ───────────────────────── #[derive(ArgEnum)] ─────────────────────────

pub(crate) fn expand_arg_enum(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let nc = nagisa_core_root();
    let enum_ident = &input.ident;
    let data = match &input.data {
        Data::Enum(e) => e,
        _ => return Err(Error::new_spanned(enum_ident, "#[derive(ArgEnum)] only supports enums")),
    };

    let mut arms = Vec::new();
    let mut accepted: Vec<String> = Vec::new();
    for v in &data.variants {
        if !matches!(v.fields, Fields::Unit) {
            return Err(Error::new_spanned(v, "#[derive(ArgEnum)] requires unit (fieldless) variants"));
        }
        let vid = &v.ident;
        let mut names: Vec<String> = Vec::new();
        let mut renamed = false;
        for attr in &v.attrs {
            if !attr.path().is_ident("arg") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename") {
                    let s: LitStr = meta.value()?.parse()?;
                    names.insert(0, s.value());
                    renamed = true;
                } else if meta.path.is_ident("alias") {
                    let s: LitStr = meta.value()?.parse()?;
                    names.push(s.value());
                } else {
                    return Err(meta.error("unknown #[arg(..)] key on enum variant (use rename/alias)"));
                }
                Ok(())
            })?;
        }
        if !renamed {
            names.insert(0, vid.to_string().to_lowercase());
        }
        for n in &names {
            // 大小写不敏感:臂用小写,from_arg 也对输入小写后匹配(CJK 小写为恒等)。
            let lit = LitStr::new(&n.to_lowercase(), Span::call_site());
            arms.push(quote! { #lit => ::std::option::Option::Some(#enum_ident::#vid), });
            accepted.push(n.clone());
        }
    }

    let type_name = LitStr::new(&accepted.join("|"), Span::call_site());

    Ok(quote! {
        impl #nc::FromArg for #enum_ident {
            const TYPE_NAME: &'static str = #type_name;
            fn from_arg(__s: &str) -> ::std::option::Option<Self> {
                match __s.to_lowercase().as_str() {
                    #(#arms)*
                    _ => ::std::option::Option::None,
                }
            }
        }
    })
}
