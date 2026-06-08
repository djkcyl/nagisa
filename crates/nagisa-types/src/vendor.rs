//! OneBot 实现端厂商标识 [`Vendor`]：按 `get_version_info.app_name` 子串判定，供适配器对
//! 同名动作在不同实现端之间做 per-vendor 别名/降级。

/// OneBot 协议端的实现厂商，由 `app_name` 子串判定。
///
/// 只区分 nagisa 显式适配的三家——Lagrange.OneBot / NapCat / LLOneBot；其余实现端
/// （go-cqhttp / Shamrock / 未知 / 所有 Milky 端）一律归 [`Vendor::Other`]。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Vendor {
    LagrangeOneBot,
    NapCat,
    LLOneBot,
    /// 其他 / 未探测到 / 非上述三家(含所有 Milky 端)。
    #[default]
    Other,
}

impl Vendor {
    /// 由 `app_name`（大小写不敏感子串）判定厂商；非三家一律 [`Vendor::Other`]。
    pub fn from_app_name(app_name: &str) -> Self {
        let n = app_name.to_ascii_lowercase();
        if n.contains("lagrange") {
            Vendor::LagrangeOneBot
        } else if n.contains("napcat") {
            Vendor::NapCat
        } else if n.contains("llonebot") {
            Vendor::LLOneBot
        } else {
            Vendor::Other
        }
    }
}
