//! 行内解析:`**粗**`/`__粗__` `*斜*`/`_斜_` `***粗斜***`/`___粗斜___` `~~删~~` `` `码` ``
//! `==高亮==`、链接 `[文字](URL)`(取文字按链接色渲染,URL 不展示)、属性 span
//! `[文字]{color=#e00,bold,light,weight=500,size=1.2,font=serif,bg=#ff0}`、反斜杠转义 `\X`、硬换行(`\n`,由块级
//! 在行尾 `\` 处插入)。记号可嵌套——进入时在 `base` 样式上叠加,递归解析内层。
//! `_` 族贴着 ASCII 词字符(字母 / 数字 / `_`)不触发,`user_id` 这类标识符不会被吞;
//! CJK 不算词字符,`中_文_` 照常强调。

use super::{parse_attrs, Attr};
use crate::model::{Color, FontRole, Highlight, Inline, TextStyle};

/// 把一段文字解析成行内序列。
pub(crate) fn parse_inlines(s: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    parse_into(s, TextStyle::default(), &mut out);
    out
}

/// 在给定基样式下解析 `s`,结果追加进 `out`。
fn parse_into(s: &str, base: TextStyle, out: &mut Vec<Inline>) {
    let mut buf = String::new();
    let mut i = 0;
    // 上一个已消费的字符(`_` 族词内判定用);span 消费后取其末字符(总是标点)。
    let mut prev: Option<char> = None;
    while i < s.len() {
        let rest = &s[i..];

        // 硬换行(块级在行尾 `\` 处插入的 `\n`)
        if rest.starts_with('\n') {
            flush(&mut buf, &base, out);
            out.push(Inline::LineBreak);
            i += 1;
            prev = Some('\n');
            continue;
        }
        // 反斜杠转义:`\X`(X 为 ASCII 标点)→ X 字面,吞掉其记号含义;末尾或后随非标点则反斜杠按字面。
        if let Some(after) = rest.strip_prefix('\\') {
            match after.chars().next() {
                Some(ch) if ch.is_ascii_punctuation() => {
                    buf.push(ch);
                    i += 1 + ch.len_utf8();
                    prev = Some(ch);
                }
                _ => {
                    buf.push('\\');
                    i += 1;
                    prev = Some('\\');
                }
            }
            continue;
        }
        // 行内图 ![alt](src):不渲染图(行内无版面语义),取 alt 文字当占位。
        if rest.starts_with("![") {
            if let Some((alt, n)) = link_span(&rest[1..]) {
                flush(&mut buf, &base, out);
                parse_into(alt, base.clone(), out);
                i += 1 + n;
                prev = Some(')');
                continue;
            }
        }
        if rest.starts_with('[') {
            // 属性 span [文字]{attrs}
            if let Some((inner, attr_s, n)) = attr_span(rest) {
                flush(&mut buf, &base, out);
                parse_into(inner, apply_attrs(base.clone(), attr_s), out);
                i += n;
                prev = Some('}');
                continue;
            }
            // 链接 [文字](URL):图片点不了,只取文字按链接色渲染。
            if let Some((inner, n)) = link_span(rest) {
                flush(&mut buf, &base, out);
                let mut st = base.clone();
                st.link = true;
                parse_into(inner, st, out);
                i += n;
                prev = Some(')');
                continue;
            }
        }
        // 强调 / 行内码 / 高亮
        if let Some(n) = emphasis(rest, prev, &base, &mut buf, out) {
            prev = s[..i + n].chars().last();
            i += n;
            continue;
        }
        // 普通字符
        let ch = rest.chars().next().unwrap();
        buf.push(ch);
        i += ch.len_utf8();
        prev = Some(ch);
    }
    flush(&mut buf, &base, out);
}

/// 把累积的普通文字按当前样式落成一个 `Text`,并清空缓冲。
fn flush(buf: &mut String, style: &TextStyle, out: &mut Vec<Inline>) {
    if !buf.is_empty() {
        out.push(Inline::Text { text: std::mem::take(buf), style: style.clone() });
    }
}

/// 配对定界记号。命中则处理并返回消耗的字节数。`` ` `` 内是字面量(不再解析);其余在 `base`
/// 上叠样式后递归。定界符按长到短试:`***`/`___`=粗斜,`**`/`__`=粗,`*`/`_`=斜,`~~`=删,`==`=高亮。
/// `_` 族两端贴 ASCII 词字符时不触发(见 [`is_word`])。
fn emphasis(
    rest: &str,
    prev: Option<char>,
    base: &TextStyle,
    buf: &mut String,
    out: &mut Vec<Inline>,
) -> Option<usize> {
    const DELIMS: &[&str] = &["***", "___", "**", "__", "~~", "==", "`", "*", "_"];
    for &d in DELIMS {
        let Some(after) = rest.strip_prefix(d) else { continue };
        let underscore = d.starts_with('_');
        if underscore && prev.is_some_and(is_word) {
            continue; // `_` 开记号贴在词字符后:按字面(snake_case 保护)
        }
        let Some(close) = find_close(after, d, underscore) else { continue };
        let inner = &after[..close];
        if inner.is_empty() {
            continue;
        }
        flush(buf, base, out);
        let consumed = d.len() * 2 + close;
        if d == "`" {
            out.push(Inline::Code(inner.to_string()));
        } else {
            let mut st = base.clone();
            match d {
                "***" | "___" => {
                    st.weight = Some(700);
                    st.italic = true;
                }
                "**" | "__" => st.weight = Some(700),
                "*" | "_" => st.italic = true,
                "~~" => st.strike = true,
                "==" => st.highlight = Some(Highlight::Theme),
                _ => {}
            }
            parse_into(inner, st, out);
        }
        return Some(consumed);
    }
    None
}

/// 在 `after` 里找 `d` 的闭合位置;`_` 族要求闭合后不紧跟词字符(否则继续往后找)。
fn find_close(after: &str, d: &str, underscore: bool) -> Option<usize> {
    let mut from = 0;
    loop {
        let pos = after[from..].find(d)? + from;
        if underscore && after[pos + d.len()..].chars().next().is_some_and(is_word) {
            from = pos + d.len();
            continue;
        }
        return Some(pos);
    }
}

/// ASCII 词字符(字母 / 数字 / `_`)。`_` 族的词内判定只看 ASCII:CJK 邻接不算词内,
/// 所以 `中_文_` 仍可强调,而 `user_id` 不会被吞。
fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `[文字]{attrs}` → `(文字, attrs, 消耗字节数)`。不支持嵌套 `[]`(取第一个 `]`)。
fn attr_span(rest: &str) -> Option<(&str, &str, usize)> {
    let close_br = rest.find(']')?;
    let after = &rest[close_br + 1..];
    if !after.starts_with('{') {
        return None;
    }
    let close_brace = after.find('}')?;
    let inner = &rest[1..close_br];
    let attrs = &after[1..close_brace];
    Some((inner, attrs, close_br + 1 + close_brace + 1))
}

/// `[文字](目标)` → `(文字, 消耗字节数)`。不支持嵌套 `[]`(取第一个 `]`);目标里的圆括号
/// 按配对吞(维基类 URL 常含括号),不跨行。
fn link_span(rest: &str) -> Option<(&str, usize)> {
    let close_br = rest.find(']')?;
    let after = &rest[close_br + 1..];
    if !after.starts_with('(') {
        return None;
    }
    let mut depth = 0usize;
    for (k, ch) in after.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some((&rest[1..close_br], close_br + 1 + k + 1));
                }
            }
            '\n' => return None,
            _ => {}
        }
    }
    None
}

/// 把属性串叠加到基样式上。
fn apply_attrs(mut st: TextStyle, attrs: &str) -> TextStyle {
    for a in parse_attrs(attrs) {
        match a {
            Attr::Kv(k, v) => match k.as_str() {
                "color" => {
                    if let Some(c) = Color::hex(&v) {
                        st.color = Some(c);
                    }
                }
                "bg" => {
                    if let Some(c) = Color::hex(&v) {
                        st.highlight = Some(Highlight::Custom(c));
                    }
                }
                "size" => {
                    if let Ok(m) = v.parse::<f32>() {
                        if m > 0.0 {
                            st.size = m;
                        }
                    }
                }
                "font" => {
                    st.font = match v.as_str() {
                        "sans" => FontRole::Sans,
                        "serif" => FontRole::Serif,
                        "mono" => FontRole::Mono,
                        "kai" => FontRole::Kai,
                        _ => FontRole::Named(v),
                    }
                }
                "weight" => {
                    if let Ok(w) = v.parse::<u16>() {
                        if (1..=1000).contains(&w) {
                            st.weight = Some(w);
                        }
                    }
                }
                _ => {}
            },
            Attr::Flag(f) => match f.as_str() {
                "bold" => st.weight = Some(700),
                "light" => st.weight = Some(300),
                "italic" => st.italic = true,
                "underline" => st.underline = true,
                "strike" => st.strike = true,
                _ => {}
            },
        }
    }
    st
}
