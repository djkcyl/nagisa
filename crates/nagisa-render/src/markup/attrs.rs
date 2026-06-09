//! `{key=val, flag, .class}` 属性串解析。块尾属性(`# 标题 {align=center}`)与行内属性
//! span(`[文字]{color=#e00,bold}`)共用。按 `,` 与空白切分;值里不含空格。

/// 一个属性项。
pub(crate) enum Attr {
    /// `key=value`。
    Kv(String, String),
    /// 裸标志(如 `bold`、`center`)。
    Flag(String),
}

/// 解析属性串。
pub(crate) fn parse_attrs(s: &str) -> Vec<Attr> {
    s.split(|c: char| c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| {
            if let Some(eq) = t.find('=') {
                Attr::Kv(t[..eq].trim().to_string(), t[eq + 1..].trim().to_string())
            } else {
                Attr::Flag(t.to_string())
            }
        })
        .collect()
}
