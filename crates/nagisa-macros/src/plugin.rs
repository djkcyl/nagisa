//! `plugin!{}` 的**解析 + 展开层**:声明「当前模块即一个插件」。
//!
//! 职责:解析 `plugin! { key = .., name = .., category = Fun, .. }` 的字段
//! (`PluginArgs` / `PluginField` / `PluginValue`,字符串/布尔/裸标识符三种取值),
//! 再展开成一条 `inventory::submit!(PluginSpec { .. })`。省略的字段逐一回退到
//! `PluginMeta::DEFAULT` 的对应表达式;`category` 的裸标识符展开成全限定枚举变体。
//!
//! 协作:展开点的 `module_path!()` 写进 `PluginSpec.meta.module_path`,供链接期把
//! 同模块下经 [`crate::trigger`] 登记的触发器(最长前缀匹配)归属本插件。引擎路径根
//! 经 [`crate::nagisa_core_root`] 解析。

use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Error, Ident, LitBool, LitStr, Token};

use crate::nagisa_core_root;

/// 解析后的 `plugin!{}` 字段（缺省由 `PluginMeta::DEFAULT` 填充）。
pub(crate) struct PluginArgs {
    key: Option<LitStr>,
    name: Option<LitStr>,
    category: Option<Ident>,
    version: Option<LitStr>,
    description: Option<LitStr>,
    can_disable: Option<LitBool>,
    default_enable: Option<LitBool>,
    hidden: Option<LitBool>,
    maintain: Option<LitBool>,
}

impl Parse for PluginArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut out = PluginArgs {
            key: None,
            name: None,
            category: None,
            version: None,
            description: None,
            can_disable: None,
            default_enable: None,
            hidden: None,
            maintain: None,
        };
        let entries = Punctuated::<PluginField, Token![,]>::parse_terminated(input)?;
        for entry in entries {
            let key = entry.key.to_string();
            match key.as_str() {
                "key" => out.key = Some(entry.value.expect_str(&entry.key)?),
                "name" => out.name = Some(entry.value.expect_str(&entry.key)?),
                "category" => out.category = Some(entry.value.expect_ident(&entry.key)?),
                "version" => out.version = Some(entry.value.expect_str(&entry.key)?),
                "description" => out.description = Some(entry.value.expect_str(&entry.key)?),
                "can_disable" => out.can_disable = Some(entry.value.expect_bool(&entry.key)?),
                "default_enable" => out.default_enable = Some(entry.value.expect_bool(&entry.key)?),
                "hidden" => out.hidden = Some(entry.value.expect_bool(&entry.key)?),
                "maintain" => out.maintain = Some(entry.value.expect_bool(&entry.key)?),
                other => return Err(Error::new(entry.key.span(), format!("unknown plugin field `{other}`"))),
            }
        }
        Ok(out)
    }
}

/// `ident = value` 单项。`category` 取裸标识符（`Fun`），其余取字面量。
struct PluginField {
    key: Ident,
    value: PluginValue,
}

enum PluginValue {
    Str(LitStr),
    Bool(LitBool),
    Ident(Ident),
}

impl Parse for PluginField {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let key: Ident = input.parse()?;
        let _: Token![=] = input.parse()?;
        let value = if input.peek(LitStr) {
            PluginValue::Str(input.parse()?)
        } else if input.peek(LitBool) {
            PluginValue::Bool(input.parse()?)
        } else if input.peek(Ident) {
            PluginValue::Ident(input.parse()?)
        } else {
            return Err(input.error("expected a string, bool, or bare identifier"));
        };
        Ok(PluginField { key, value })
    }
}

impl PluginValue {
    fn expect_str(self, id: &Ident) -> syn::Result<LitStr> {
        match self {
            PluginValue::Str(s) => Ok(s),
            _ => Err(Error::new(id.span(), format!("`{id}` expects a string literal"))),
        }
    }
    fn expect_bool(self, id: &Ident) -> syn::Result<LitBool> {
        match self {
            PluginValue::Bool(b) => Ok(b),
            _ => Err(Error::new(id.span(), format!("`{id}` expects a bool literal (true/false)"))),
        }
    }
    fn expect_ident(self, id: &Ident) -> syn::Result<Ident> {
        match self {
            PluginValue::Ident(i) => Ok(i),
            _ => Err(Error::new(id.span(), format!("`{id}` expects a bare category identifier (e.g. `Fun`)"))),
        }
    }
}

pub(crate) fn expand_plugin(args: PluginArgs) -> proc_macro2::TokenStream {
    let nc = nagisa_core_root();
    // 每个字段:有值则覆盖,无值则继承 PluginMeta::DEFAULT（用 `..` 不可，需逐字段；
    // 故 None 时回退到 DEFAULT 的对应表达式）。category 的裸标识符展开为全限定枚举变体。
    let key = match args.key {
        Some(s) => quote! { #s },
        None => quote! { #nc::plugin::PluginMeta::DEFAULT.key },
    };
    let name = match args.name {
        Some(s) => quote! { #s },
        None => quote! { #nc::plugin::PluginMeta::DEFAULT.name },
    };
    let category = match args.category {
        Some(c) => quote! { #nc::plugin::Category::#c },
        None => quote! { #nc::plugin::PluginMeta::DEFAULT.category },
    };
    let version = match args.version {
        Some(s) => quote! { #s },
        None => quote! { #nc::plugin::PluginMeta::DEFAULT.version },
    };
    let description = match args.description {
        Some(s) => quote! { #s },
        None => quote! { #nc::plugin::PluginMeta::DEFAULT.description },
    };
    let can_disable = match args.can_disable {
        Some(b) => quote! { #b },
        None => quote! { #nc::plugin::PluginMeta::DEFAULT.can_disable },
    };
    let default_enable = match args.default_enable {
        Some(b) => quote! { #b },
        None => quote! { #nc::plugin::PluginMeta::DEFAULT.default_enable },
    };
    let hidden = match args.hidden {
        Some(b) => quote! { #b },
        None => quote! { #nc::plugin::PluginMeta::DEFAULT.hidden },
    };
    let maintain = match args.maintain {
        Some(b) => quote! { #b },
        None => quote! { #nc::plugin::PluginMeta::DEFAULT.maintain },
    };

    quote! {
        #nc::inventory::submit! {
            #nc::plugin::PluginSpec {
                meta: #nc::plugin::PluginMeta {
                    key: #key,
                    name: #name,
                    category: #category,
                    version: #version,
                    description: #description,
                    can_disable: #can_disable,
                    default_enable: #default_enable,
                    hidden: #hidden,
                    maintain: #maintain,
                    module_path: ::core::module_path!(),
                }
            }
        }
    }
}
