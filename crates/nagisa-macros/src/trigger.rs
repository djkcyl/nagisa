//! `#[command]` / `#[event]` 的**代码展开层**。
//!
//! 职责:把 [`crate::attrs`] 解析出的参数展开成最终代码。两个属性宏共享同一骨架
//! `emit_trigger`——发出「原函数 + `<FN>_KEY` 开关键常量 + `<FN>__nagisa_register` 注册
//! 函数 + `inventory::submit!(TriggerSpec)`」;触发器特有的三处差异(register 体前导、
//! 挂载调用、`TriggerKind`)由 `TriggerVariant` 携带。`validate_handler_fn` 做共用的
//! handler 前置校验(非泛型、无 `self`、`async fn`),`lower_gate` 把 `gate=`/`cooldown=`
//! 降级成传给 router 的 `Option<Rule>`(cooldown AND 进门控链最右)。
//!
//! 协作:`expand`(command)与 `expand_event`(event)由 crate 根的宏入口调用;
//! command 的 `slots = T` 匹配器取 `<T as FromSlots>::matcher()`,衔接 [`crate::slots`]
//! 生成的 impl。所有引擎路径经 [`crate::nagisa_core_root`] 解析。

use proc_macro2::Span;
use quote::{format_ident, quote};
use syn::{Error, Expr, FnArg, GenericArgument, ItemFn, LitStr, PathArguments, Type};

use crate::attrs::{CommandArgs, EventArgs, MatcherKind, MetaArgs};
use crate::nagisa_core_root;

/// 从 handler 形参里找 `args: Args<T>`（末段标识符为 `Args` 的单泛型路径）并取出内层 `T`。
/// 命令用它把 `<T as ArgsMeta>::SPECS` 写进 `TriggerMeta.args`，供 help 自动生成用法；找不到为 `None`。
fn find_args_inner(func: &ItemFn) -> Option<&Type> {
    for input in &func.sig.inputs {
        let FnArg::Typed(pt) = input else { continue };
        let Type::Path(tp) = &*pt.ty else { continue };
        let Some(seg) = tp.path.segments.last() else { continue };
        if seg.ident != "Args" {
            continue;
        }
        let PathArguments::AngleBracketed(ab) = &seg.arguments else { continue };
        if let Some(GenericArgument::Type(t)) = ab.args.first() {
            return Some(t);
        }
    }
    None
}

/// 把 `gate=`/`cooldown=` 降级成传给 `trigger_command`/`event_named` 的 `gate` 参数那个
/// `Option<Rule>` 表达式。两者都缺 ⇒ 字面 `None` token（未门控命令/事件零改动）；否则
/// `Some(..)`。
///
/// 组合方式：`gate` 表达式**原样**拼入；cooldown 规则 AND 在**最右**，使权限/开关先求值、
/// cooldown 只在左侧门全过后才盖戳——`gate & Cooldown::from(<cd>).into_rule(..)`。cooldown
/// 的 `TriggerId` 引用 register 函数运行期的 `plugin_key`/`key` 绑定（拼接点处在作用域内），
/// 故各触发器的作用域键正确。
fn lower_gate(nc: &proc_macro2::TokenStream, gate: &Option<Expr>, cooldown: &Option<Expr>) -> proc_macro2::TokenStream {
    let cd_rule = cooldown.as_ref().map(|cd| {
        quote! {
            #nc::Cooldown::from(#cd).into_rule(#nc::TriggerId::of(plugin_key, key))
        }
    });
    match (gate, cd_rule) {
        (None, None) => quote! { ::core::option::Option::None },
        (Some(g), None) => quote! { ::core::option::Option::Some(#g) },
        (None, Some(cd)) => quote! { ::core::option::Option::Some(#cd) },
        // cooldown AND 在最右 ⇒ 左到右短路下最后才求值(权限过后才盖戳)。
        (Some(g), Some(cd)) => quote! { ::core::option::Option::Some((#g) & (#cd)) },
    }
}

/// `#[command]`/`#[event]` 共用的 handler 函数前置校验：必须是非泛型、无 `self` 接收者的
/// `async fn`。`macro_name`（`"#[command]"`/`"#[event]"`）只用于报错文案。
fn validate_handler_fn(func: &ItemFn, macro_name: &str) -> syn::Result<()> {
    if func.sig.asyncness.is_none() {
        return Err(Error::new_spanned(func.sig.fn_token, format!("`{macro_name}` requires an `async fn`")));
    }
    if !func.sig.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &func.sig.generics,
            format!("`{macro_name}` does not support generic functions"),
        ));
    }
    if let Some(first) = func.sig.inputs.first() {
        if matches!(first, FnArg::Receiver(_)) {
            return Err(Error::new_spanned(first, format!("`{macro_name}` must be a free `async fn`, not a method")));
        }
    }
    Ok(())
}

/// 触发器特有部分（`#[command]` vs `#[event]`），交给共享骨架 [`emit_trigger`] 拼装。
struct TriggerVariant {
    /// register fn 体里、解析插件之前要执行的语句（command：构建 `let m = …;`+mention+usage；
    /// event：空）。
    register_prelude: proc_macro2::TokenStream,
    /// register fn 末尾的挂载调用（`r.trigger_command(..)` / `r.event_named(..)`）。
    register_call: proc_macro2::TokenStream,
    /// `TriggerSpec.meta.kind` 的值（`TriggerKind::Command` / `TriggerKind::Event(<path>)`）。
    trigger_kind: proc_macro2::TokenStream,
    /// `TriggerMeta.words` 的值（命令的字面调用词数组 `&[..]`；事件/正则/槽位为 `&[]`）。
    meta_words: proc_macro2::TokenStream,
    /// `TriggerMeta.args` 的值（`<T as ArgsMeta>::SPECS`，命令的 `args: Args<T>` 形参带入；
    /// 无该形参的命令、事件触发器为 `&[]`）。
    meta_args: proc_macro2::TokenStream,
}

/// 两个属性宏（`#[command]`/`#[event]`）展开的共享骨架：前置校验已过、元数据缺省已算好后，
/// 发出 `原函数 + <FN>_KEY 常量 + <FN>__nagisa_register 注册函数 + inventory 提交`。触发器特有
/// 的三处（register 体前导、挂载调用、`TriggerKind`）由 `variant` 携带，其余完全一致。
#[allow(clippy::too_many_arguments)]
fn emit_trigger(
    nc: &proc_macro2::TokenStream,
    func: &ItemFn,
    meta: &MetaArgs,
    variant: TriggerVariant,
) -> proc_macro2::TokenStream {
    let fn_name = &func.sig.ident;
    let vis = &func.vis;
    let register_name = format_ident!("{}__nagisa_register", fn_name);
    // 强类型分层开关键句柄：`<FN>_KEY` 在使用处解析成 `"<plugin_key>.<id>"`,
    // 故 `EnabledSet::set(echo_a_KEY, ..)` 不会因手敲字符串拼错而静默失效。
    let key_const_name = format_ident!("{}_KEY", fn_name);

    // 从被标注函数上收集 cfg/cfg_attr 属性,镜像到 register 函数上。
    let cfg_attrs: Vec<_> =
        func.attrs.iter().filter(|a| a.path().is_ident("cfg") || a.path().is_ident("cfg_attr")).collect();

    // —— 插件/触发器元数据（→ `TriggerMeta`）。缺省：id=name=fn 名、
    //    can_disable/default_enable=true、hidden=false。——
    let fn_name_str = fn_name.to_string();
    let id_lit = LitStr::new(meta.id.as_deref().unwrap_or(&fn_name_str), Span::call_site());
    let name_lit = LitStr::new(meta.name.as_deref().unwrap_or(&fn_name_str), Span::call_site());
    let description_lit = LitStr::new(meta.description.as_deref().unwrap_or(""), Span::call_site());
    // 详细用法：与挂到 matcher 的 parse-miss hint 同源，但这里另存进 TriggerMeta 供 help 展示。
    let usage_lit = LitStr::new(meta.usage.as_deref().unwrap_or(""), Span::call_site());
    let can_disable = meta.can_disable.unwrap_or(true);
    let default_enable = meta.default_enable.unwrap_or(true);
    let hidden = meta.hidden.unwrap_or(false);
    let order = meta.order.unwrap_or(0);

    let TriggerVariant { register_prelude, register_call, trigger_kind, meta_words, meta_args } = variant;

    quote! {
        #func

        #(#cfg_attrs)*
        #[allow(non_upper_case_globals)]
        #vis const #key_const_name: #nc::SwitchKey =
            #nc::SwitchKey::trigger(::core::module_path!(), #id_lit);

        #(#cfg_attrs)*
        #[doc(hidden)]
        #[allow(non_snake_case)]
        #vis fn #register_name(r: #nc::Router) -> #nc::Router {
            #register_prelude
            // 用本触发器所在模块路径解析其归属插件（最长前缀匹配；无匹配则合成隐式插件），
            // 拿到插件层的 default_enable/can_disable,与触发器层一起交给分层 EnabledSet 门控。
            let mp = ::core::module_path!();
            let (plugin_key, plugin_default, plugin_can_disable) =
                #nc::plugin::resolve_plugin_for(mp);
            let id = #id_lit;
            let key = #nc::plugin::trigger_key(plugin_key, id);
            #register_call
        }

        // 把本触发器登记进 inventory 注册表（`collect_into`/`registered_triggers` 据此工作）。
        // `key`/`plugin_key` 留空，由链接期（`registered_triggers_resolved`）按 `module_path`
        // 回填。携带与 register fn 相同的 cfg 门控，避免被 cfg-out 时残留悬挂引用。
        #(#cfg_attrs)*
        #nc::inventory::submit! {
            #nc::registry::TriggerSpec {
                meta: #nc::plugin::TriggerMeta {
                    id: #id_lit,
                    key: "",
                    plugin_key: "",
                    name: #name_lit,
                    description: #description_lit,
                    usage: #usage_lit,
                    words: #meta_words,
                    args: #meta_args,
                    order: #order,
                    can_disable: #can_disable,
                    default_enable: #default_enable,
                    hidden: #hidden,
                    kind: #trigger_kind,
                    module_path: ::core::module_path!(),
                },
                register: #register_name,
            }
        }
    }
}

pub(crate) fn expand(args: CommandArgs, func: ItemFn) -> syn::Result<proc_macro2::TokenStream> {
    validate_handler_fn(&func, "#[command]")?;

    let nc = nagisa_core_root();
    let fn_name = &func.sig.ident;

    // 构造匹配器表达式（一种触发器:command 字面量 / 原始 regex / slots 类型）。
    let matcher_build = match &args.kind {
        MatcherKind::Union(alts) => {
            let lits = alts.iter().map(|s| LitStr::new(s, Span::call_site()));
            quote! { #nc::Matcher::command([ #( #lits ),* ]) }
        }
        MatcherKind::Regex(s) => {
            let lit = LitStr::new(s, Span::call_site());
            // regex 编译失败时 panic（编译期写死的正则，启动时即暴露），错误信息里带上 pattern。
            quote! { #nc::Matcher::regex(#lit).expect(concat!("invalid #[command] regex: ", #lit)) }
        }
        // `slots = ViewBoard`：头匹配器取 `<ViewBoard as FromSlots>::matcher()`。
        MatcherKind::Slots(ty) => {
            quote! { <#ty as #nc::FromSlots>::matcher() }
        }
    };

    // 剩余内容要求。无参命令(无 `args: Args<T>` 形参,或 T 的参数规格为空)**默认**
    // `no_args`:除呼叫姿势(回复/@bot/空白)外命令词后有任何内容就不算命中——「我的 xxx」
    // 是日常说话,不该触发「我的」。`exact` 旗标显式升档:整条消息只能是命令词本身,
    // 连呼叫姿势都不算;它只对无参命令有意义,与 `args: Args<T>` 形参同用是编译错。
    // 正则 / 槽位匹配器形状由作者的模式自己定,不自动收紧(`exact` 同样可用)。
    let args_inner = find_args_inner(&func);
    if args.exact && args_inner.is_some() {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "`exact` 是无参命令的严格模式(整条消息只能是命令词本身),不能与 `args: Args<T>` 形参同用",
        ));
    }
    let strict_apply = if args.exact {
        quote! { let m = m.exact(); }
    } else {
        match (&args.kind, &args_inner) {
            (MatcherKind::Union(_), None) => quote! { let m = m.no_args(); },
            (MatcherKind::Union(_), Some(t)) => quote! {
                let m = if <#t as #nc::ArgsMeta>::SPECS.is_empty() { m.no_args() } else { m };
            },
            _ => quote! {},
        }
    };

    let mention = if args.mention_me {
        quote! { let m = m.mention_me(); }
    } else {
        quote! {}
    };

    let priority = args.priority;
    let top = args.top;
    let meta = &args.meta;

    // 字面命令词（首个为主词、其余别名）写进 TriggerMeta.words 供 help 展示。
    // 正则/槽位匹配器无字面词 ⇒ 空切片。
    let meta_words = match &args.kind {
        MatcherKind::Union(alts) => {
            let lits = alts.iter().map(|s| LitStr::new(s, Span::call_site()));
            quote! { &[ #( #lits ),* ] }
        }
        _ => quote! { &[] },
    };

    // `usage="…"` → 给 matcher 挂上显式用法串：命中后随事件携带,
    // parse-miss 时优先于 dev 自动 hint 回贴。无 usage 则不动 matcher。
    let usage_apply = match &meta.usage {
        Some(u) => {
            let lit = LitStr::new(u, Span::call_site());
            quote! { let m = m.with_usage(#lit); }
        }
        None => quote! {},
    };

    let default_enable = meta.default_enable.unwrap_or(true);
    let can_disable = meta.can_disable.unwrap_or(true);

    // `gate=`/`cooldown=` → `Option<Rule>`（cooldown AND-ed 在门控链最右）。无声明则为字面
    // `None`,与未门控命令零差异;`Some` 时 matcher 命中后求值,不过则跳过。
    let gate_tokens = lower_gate(&nc, &meta.gate, &meta.cooldown);

    let variant = TriggerVariant {
        register_prelude: quote! {
            let m = #matcher_build;
            #strict_apply
            #mention
            #usage_apply
        },
        register_call: quote! {
            r.trigger_command(plugin_key, key, plugin_default, plugin_can_disable,
                              #default_enable, #can_disable, #top, #priority, m, #gate_tokens, #fn_name)
        },
        trigger_kind: quote! { #nc::plugin::TriggerKind::Command },
        meta_words,
        // 有 `args: Args<T>` 形参就取 `<T as ArgsMeta>::SPECS`,供 help 自动生成用法;否则空。
        meta_args: match find_args_inner(&func) {
            Some(t) => quote! { <#t as #nc::ArgsMeta>::SPECS },
            None => quote! { &[] },
        },
    };
    Ok(emit_trigger(&nc, &func, meta, variant))
}

// ───────────────────────── #[event(Kind, ..)] ─────────────────────────

pub(crate) fn expand_event(args: EventArgs, func: ItemFn) -> syn::Result<proc_macro2::TokenStream> {
    validate_handler_fn(&func, "#[event]")?;

    let nc = nagisa_core_root();
    let fn_name = &func.sig.ident;

    let priority = args.priority;
    let top = args.top;
    let kind_ident = &args.kind;
    let kind_path = quote! { #nc::EventKind::#kind_ident };

    let meta = &args.meta;
    let default_enable = meta.default_enable.unwrap_or(true);
    let can_disable = meta.can_disable.unwrap_or(true);

    // `gate=`/`cooldown=` → `Option<Rule>`(cooldown 合成在门控链最右)。无声明则为字面
    // `None`,与未门控事件 handler 零差异;`Some` 时 EventKind 命中后求值,不过则跳过。
    // (`usage=` 对事件无消费者,解析层已直接报错拒绝——见 `EventArgs::parse`。)
    let gate_tokens = lower_gate(&nc, &meta.gate, &meta.cooldown);

    let variant = TriggerVariant {
        // 事件触发器无匹配器/mention/usage：register 体无前导语句。
        register_prelude: quote! {},
        register_call: quote! {
            r.event_named(plugin_key, key, plugin_default, plugin_can_disable,
                          #default_enable, #can_disable, #top, #priority, #kind_path, #gate_tokens, #fn_name)
        },
        trigger_kind: quote! { #nc::plugin::TriggerKind::Event(#kind_path) },
        // 事件触发器无字面命令词、无命令参数。
        meta_words: quote! { &[] },
        meta_args: quote! { &[] },
    };
    Ok(emit_trigger(&nc, &func, meta, variant))
}
