//! 代码块语法上色 —— 零依赖的轻量词法扫描,按语言标签分发。
//!
//! 口径:只分四类词(关键字 / 字面量 / 字符串 / 注释),拿不准就不着色——着色是
//! 锦上添花,错色比无色糟。返回的字节区间升序、互不重叠、落在 char 边界;未覆盖
//! 区间按主题默认代码色渲。认不出的语言返回空(整块默认色),与解析「宽容」同款。
//!
//! 全语言高质量上色(syntect 路线)刻意不做:正则引擎 + 语法资产几百 KB,对
//! 「聊天里发张代码图」不值;真有需求时 tokenize 接口原地可换后端。
//!
//! 安全前提(所有扫描器共守):token 一律从 ASCII 字节起步,UTF-8 多字节字符的
//! 后续字节(≥ 0x80)撞不上任何 ASCII 分支,逐字节推进不会切坏 char 边界;
//! 转义吞字节可能跳进多字节中段,但后续字节不等于 ASCII 引号,误闭不了串。

use core::ops::Range;

/// 代码词类(着色用)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TokenKind {
    /// 关键字(含 C 系预处理行)。
    Keyword,
    /// 字面量:数字与 true/false/null/None 这类常量。
    Literal,
    /// 字符串(各种引号形态,含未闭合到文尾)。
    StringLit,
    /// 注释(行注释与块注释,含未闭合到文尾)。
    Comment,
}

/// 按语言标签切词。标签大小写不敏感,常用别名(rs / py / js / ts / sh…)已映射。
pub(crate) fn tokenize(lang: &str, src: &str) -> Vec<(Range<usize>, TokenKind)> {
    match lang.trim().to_ascii_lowercase().as_str() {
        "rust" | "rs" => tokenize_rust(src),
        "json" | "jsonc" | "json5" => tokenize_json(src),
        "toml" => tokenize_toml(src),
        "python" | "py" => tokenize_python(src),
        "javascript" | "js" | "typescript" | "ts" | "jsx" | "tsx" => tokenize_javascript(src),
        "shell" | "sh" | "bash" | "zsh" | "console" => tokenize_shell(src),
        "c" | "cpp" | "c++" | "h" | "hpp" | "cc" => tokenize_c(src),
        _ => Vec::new(),
    }
}

// ── 各语言扫描器 ──

const RUST_KW: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "fn", "for", "if",
    "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static",
    "struct", "super", "trait", "type", "union", "unsafe", "use", "where", "while",
];

/// Rust:行/嵌套块注释、字符串与原始字符串(r#"…"#,井号可多,b/br 前缀同)、
/// 字符字面量与生命周期的区分(生命周期不着色)、数字(0x/0o/0b/下划线/类型后缀)。
fn tokenize_rust(src: &str) -> Vec<(Range<usize>, TokenKind)> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                let end = scan_line_end(b, i + 2);
                out.push((i..end, TokenKind::Comment));
                i = end;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let end = scan_block_nested(b, i + 2);
                out.push((i..end, TokenKind::Comment));
                i = end;
            }
            b'"' => {
                let end = scan_quoted(b, i + 1, b'"');
                out.push((i..end, TokenKind::StringLit));
                i = end;
            }
            b'r' | b'b' => {
                // 原始 / 字节字符串前缀:r" r#" b" br" br#";不匹配则按标识符走。
                if let Some(end) = scan_rust_prefixed_string(b, i) {
                    out.push((i..end, TokenKind::StringLit));
                    i = end;
                } else {
                    i = push_ident(b, i, RUST_KW, &["true", "false"], &mut out);
                }
            }
            b'\'' => {
                // 'a'(字符,含转义与多字节)着色;'a / 'static(生命周期)不着色。
                if b.get(i + 1) == Some(&b'\\') {
                    let end = scan_quoted(b, i + 1, b'\'');
                    out.push((i..end, TokenKind::StringLit));
                    i = end;
                } else if let Some(&c1) = b.get(i + 1) {
                    let n = utf8_len(c1);
                    if b.get(i + 1 + n) == Some(&b'\'') {
                        out.push((i..i + 2 + n, TokenKind::StringLit));
                        i += 2 + n;
                    } else if is_ident_byte(c1) && !c1.is_ascii_digit() {
                        i = scan_ident_end(b, i + 1); // 生命周期
                    } else {
                        i += 1;
                    }
                } else {
                    i += 1;
                }
            }
            c if c.is_ascii_digit() => {
                let end = scan_code_number(b, i);
                out.push((i..end, TokenKind::Literal));
                i = end;
            }
            c if is_ident_start(c) => {
                i = push_ident(b, i, RUST_KW, &["true", "false"], &mut out);
            }
            _ => i += 1,
        }
    }
    out
}

/// JSON / JSONC / JSON5:字符串(双/单引号,对象键同款)、数字、true/false/null;
/// // 与 /* */ 超出 JSON 标准,按 JSONC 宽容着色为注释。
fn tokenize_json(src: &str) -> Vec<(Range<usize>, TokenKind)> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            q @ (b'"' | b'\'') => {
                let end = scan_quoted(b, i + 1, q);
                out.push((i..end, TokenKind::StringLit));
                i = end;
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                let end = scan_line_end(b, i + 2);
                out.push((i..end, TokenKind::Comment));
                i = end;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let end = scan_block_end(b, i + 2);
                out.push((i..end, TokenKind::Comment));
                i = end;
            }
            c if c.is_ascii_digit() || (c == b'-' && b.get(i + 1).is_some_and(u8::is_ascii_digit)) => {
                let end = scan_code_number(b, if c == b'-' { i + 1 } else { i }).max(i + 1);
                out.push((i..end, TokenKind::Literal));
                i = end;
            }
            c if is_ident_start(c) => {
                i = push_ident(b, i, &[], &["true", "false", "null"], &mut out);
            }
            _ => i += 1,
        }
    }
    out
}

/// TOML:# 注释、基本字符串(双引号,转义)与字面字符串(单引号,无转义)、
/// 三引号多行两款、数字与 true/false。键名与日期不强求。
fn tokenize_toml(src: &str) -> Vec<(Range<usize>, TokenKind)> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'#' => {
                let end = scan_line_end(b, i + 1);
                out.push((i..end, TokenKind::Comment));
                i = end;
            }
            b'"' if b.get(i + 1) == Some(&b'"') && b.get(i + 2) == Some(&b'"') => {
                let end = scan_triple(b, i + 3, b'"', true);
                out.push((i..end, TokenKind::StringLit));
                i = end;
            }
            b'"' => {
                let end = scan_quoted(b, i + 1, b'"');
                out.push((i..end, TokenKind::StringLit));
                i = end;
            }
            b'\'' if b.get(i + 1) == Some(&b'\'') && b.get(i + 2) == Some(&b'\'') => {
                let end = scan_triple(b, i + 3, b'\'', false);
                out.push((i..end, TokenKind::StringLit));
                i = end;
            }
            b'\'' => {
                let end = scan_raw_quoted(b, i + 1, b'\'');
                out.push((i..end, TokenKind::StringLit));
                i = end;
            }
            c if c.is_ascii_digit() => {
                let end = scan_code_number(b, i);
                out.push((i..end, TokenKind::Literal));
                i = end;
            }
            c if is_ident_start(c) => {
                i = push_ident(b, i, &[], &["true", "false"], &mut out);
            }
            _ => i += 1,
        }
    }
    out
}

const PY_KW: &[&str] = &[
    "and", "as", "assert", "async", "await", "break", "case", "class", "continue", "def", "del", "elif", "else",
    "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "match", "nonlocal", "not",
    "or", "pass", "raise", "return", "try", "while", "with", "yield",
];

/// Python:# 注释、单双引号与三引号字符串(r/b/f/u 前缀,可组合两枚)、数字、
/// 关键字(match/case 软关键字一并算)、True/False/None 按字面量。
fn tokenize_python(src: &str) -> Vec<(Range<usize>, TokenKind)> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'#' => {
                let end = scan_line_end(b, i + 1);
                out.push((i..end, TokenKind::Comment));
                i = end;
            }
            q @ (b'"' | b'\'') => {
                let end = scan_py_string(b, i, q);
                out.push((i..end, TokenKind::StringLit));
                i = end;
            }
            c if c.is_ascii_digit() => {
                let end = scan_code_number(b, i);
                out.push((i..end, TokenKind::Literal));
                i = end;
            }
            c if is_ident_start(c) => {
                // 字符串前缀(r/b/f/u 至多两枚,紧跟引号)整体并入字符串。
                let end = scan_ident_end(b, i);
                let word = &b[i..end];
                let is_prefix = word.len() <= 2
                    && word.iter().all(|c| matches!(c, b'r' | b'b' | b'f' | b'u' | b'R' | b'B' | b'F' | b'U'))
                    && matches!(b.get(end), Some(b'"' | b'\''));
                if is_prefix {
                    let q = b[end];
                    let send = scan_py_string(b, end, q);
                    out.push((i..send, TokenKind::StringLit));
                    i = send;
                } else {
                    i = push_ident(b, i, PY_KW, &["True", "False", "None"], &mut out);
                }
            }
            _ => i += 1,
        }
    }
    out
}

const JS_KW: &[&str] = &[
    "abstract",
    "as",
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "declare",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "finally",
    "for",
    "from",
    "function",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "keyof",
    "let",
    "namespace",
    "new",
    "of",
    "private",
    "protected",
    "public",
    "readonly",
    "return",
    "satisfies",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "try",
    "type",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

/// JS / TS:行/块注释、单双引号与模板字符串(整段按字符串,`${}` 不递归)、数字
/// (含 BigInt 的 n 后缀)、关键字(含 TS 常用),true/false/null/undefined 按字面量。
/// 正则字面量不识别(按普通文本)。
fn tokenize_javascript(src: &str) -> Vec<(Range<usize>, TokenKind)> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                let end = scan_line_end(b, i + 2);
                out.push((i..end, TokenKind::Comment));
                i = end;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let end = scan_block_end(b, i + 2);
                out.push((i..end, TokenKind::Comment));
                i = end;
            }
            q @ (b'"' | b'\'' | b'`') => {
                let end = scan_quoted(b, i + 1, q);
                out.push((i..end, TokenKind::StringLit));
                i = end;
            }
            c if c.is_ascii_digit() => {
                let end = scan_code_number(b, i);
                out.push((i..end, TokenKind::Literal));
                i = end;
            }
            c if is_ident_start(c) => {
                i = push_ident(b, i, JS_KW, &["true", "false", "null", "undefined", "NaN"], &mut out);
            }
            _ => i += 1,
        }
    }
    out
}

const SH_KW: &[&str] = &[
    "case", "do", "done", "elif", "else", "esac", "export", "fi", "for", "function", "if", "in", "local", "readonly",
    "return", "select", "then", "until", "while",
];

/// Shell(bash/sh):# 注释(`$#` 与 `${#var}` 不是注释)、单引号(无转义)与
/// 双引号(转义)字符串、流程关键字。数字与展开不强求,heredoc 按普通文本。
fn tokenize_shell(src: &str) -> Vec<(Range<usize>, TokenKind)> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            // 注释:行首,或前一字节是空白 / 分隔符——$# 前是 $、${#var} 前是 {,皆不命中。
            b'#' if i == 0 || matches!(b[i - 1], b' ' | b'\t' | b'\n' | b';' | b'|' | b'&' | b'(' | b'`') => {
                let end = scan_line_end(b, i + 1);
                out.push((i..end, TokenKind::Comment));
                i = end;
            }
            b'\'' => {
                let end = scan_raw_quoted(b, i + 1, b'\'');
                out.push((i..end, TokenKind::StringLit));
                i = end;
            }
            b'"' => {
                let end = scan_quoted(b, i + 1, b'"');
                out.push((i..end, TokenKind::StringLit));
                i = end;
            }
            c if is_ident_start(c) => {
                i = push_ident(b, i, SH_KW, &[], &mut out);
            }
            _ => i += 1,
        }
    }
    out
}

const C_KW: &[&str] = &[
    "auto",
    "bool",
    "break",
    "case",
    "catch",
    "char",
    "class",
    "const",
    "constexpr",
    "continue",
    "decltype",
    "default",
    "delete",
    "do",
    "double",
    "else",
    "enum",
    "explicit",
    "extern",
    "final",
    "float",
    "for",
    "friend",
    "goto",
    "if",
    "inline",
    "int",
    "long",
    "mutable",
    "namespace",
    "new",
    "noexcept",
    "operator",
    "override",
    "private",
    "protected",
    "public",
    "return",
    "short",
    "signed",
    "sizeof",
    "static",
    "struct",
    "switch",
    "template",
    "this",
    "throw",
    "try",
    "typedef",
    "typename",
    "unsigned",
    "using",
    "virtual",
    "void",
    "volatile",
    "while",
];

/// C / C++:行/块注释、字符串与字符字面量、数字(含 0x 与 u/l/f 后缀)、关键字、
/// 行首预处理指令(#include/#define…)整行按关键字;true/false/nullptr/NULL 按字面量。
fn tokenize_c(src: &str) -> Vec<(Range<usize>, TokenKind)> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'#' if at_line_start(b, i) => {
                let end = scan_line_end(b, i + 1);
                out.push((i..end, TokenKind::Keyword));
                i = end;
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                let end = scan_line_end(b, i + 2);
                out.push((i..end, TokenKind::Comment));
                i = end;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let end = scan_block_end(b, i + 2);
                out.push((i..end, TokenKind::Comment));
                i = end;
            }
            q @ (b'"' | b'\'') => {
                let end = scan_quoted(b, i + 1, q);
                out.push((i..end, TokenKind::StringLit));
                i = end;
            }
            c if c.is_ascii_digit() => {
                let end = scan_code_number(b, i);
                out.push((i..end, TokenKind::Literal));
                i = end;
            }
            c if is_ident_start(c) => {
                i = push_ident(b, i, C_KW, &["true", "false", "nullptr", "NULL"], &mut out);
            }
            _ => i += 1,
        }
    }
    out
}

// ── 共用字节扫描助手(token 起点都是 ASCII 字节,区间天然落在 char 边界) ──

/// 标识符整段扫完:命中关键字 / 字面量表则着色,否则跳过;返回末端。
fn push_ident(
    b: &[u8],
    i: usize,
    keywords: &[&str],
    literals: &[&str],
    out: &mut Vec<(Range<usize>, TokenKind)>,
) -> usize {
    let end = scan_ident_end(b, i);
    let word = &b[i..end];
    if literals.iter().any(|k| k.as_bytes() == word) {
        out.push((i..end, TokenKind::Literal));
    } else if keywords.iter().any(|k| k.as_bytes() == word) {
        out.push((i..end, TokenKind::Keyword));
    }
    end
}

/// 引号串:从引号后扫到同款闭引号(含),反斜杠吞下一字节;未闭合到文尾。
/// 转义可能跳进多字节字符中段,但 UTF-8 后续字节不等于 ASCII 引号,误闭不了。
fn scan_quoted(b: &[u8], mut i: usize, quote: u8) -> usize {
    while i < b.len() {
        if b[i] == b'\\' {
            i += 2;
        } else if b[i] == quote {
            return i + 1;
        } else {
            i += 1;
        }
    }
    b.len()
}

/// 字面引号串(无转义,shell 单引号 / TOML 字面串):扫到闭引号(含);未闭合到文尾。
fn scan_raw_quoted(b: &[u8], mut i: usize, quote: u8) -> usize {
    while i < b.len() {
        if b[i] == quote {
            return i + 1;
        }
        i += 1;
    }
    b.len()
}

/// Python 字符串:`i` 站在开引号上,三连引号走多行扫描,否则单行(均带转义)。
fn scan_py_string(b: &[u8], i: usize, q: u8) -> usize {
    if b.get(i + 1) == Some(&q) && b.get(i + 2) == Some(&q) {
        scan_triple(b, i + 3, q, true)
    } else {
        scan_quoted(b, i + 1, q)
    }
}

/// 三引号串(Python / TOML 多行):扫到三连引号(含);`esc` 开 = 反斜杠吞下一字节。
fn scan_triple(b: &[u8], mut i: usize, quote: u8, esc: bool) -> usize {
    while i < b.len() {
        if esc && b[i] == b'\\' {
            i += 2;
        } else if b[i] == quote && b.get(i + 1) == Some(&quote) && b.get(i + 2) == Some(&quote) {
            return i + 3;
        } else {
            i += 1;
        }
    }
    b.len()
}

/// 行注释扫到行尾(换行不归注释)。
fn scan_line_end(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i] != b'\n' {
        i += 1;
    }
    i
}

/// 块注释扫到 */(含);未闭合到文尾。
fn scan_block_end(b: &[u8], mut i: usize) -> usize {
    while i + 1 < b.len() {
        if b[i] == b'*' && b[i + 1] == b'/' {
            return i + 2;
        }
        i += 1;
    }
    b.len()
}

/// 嵌套块注释(Rust):/* 与 */ 配平;未闭合到文尾。
fn scan_block_nested(b: &[u8], mut i: usize) -> usize {
    let mut depth = 1usize;
    while i + 1 < b.len() {
        if b[i] == b'/' && b[i + 1] == b'*' {
            depth += 1;
            i += 2;
        } else if b[i] == b'*' && b[i + 1] == b'/' {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return i;
            }
        } else {
            i += 1;
        }
    }
    b.len()
}

/// Rust 原始 / 字节字符串:`r"…"` `r#"…"#`(井号可多)`b"…"` `br#"…"#`。
/// 起点 `i` 是 r 或 b;不构成该形态时返回 None(回落标识符)。
fn scan_rust_prefixed_string(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i;
    if b.get(j) == Some(&b'b') {
        j += 1;
    }
    let raw = b.get(j) == Some(&b'r');
    if raw {
        j += 1;
    }
    if !raw {
        // 仅 b"…":带转义的字节串。
        return (j > i && b.get(j) == Some(&b'"')).then(|| scan_quoted(b, j + 1, b'"'));
    }
    let hash_start = j;
    while b.get(j) == Some(&b'#') {
        j += 1;
    }
    let hashes = j - hash_start;
    if b.get(j) != Some(&b'"') {
        return None;
    }
    j += 1;
    // 找 `"` + hashes 枚 `#` 的闭合;原始串无转义。
    while j < b.len() {
        if b[j] == b'"' && b[j + 1..].len() >= hashes && b[j + 1..j + 1 + hashes].iter().all(|c| *c == b'#') {
            return Some(j + 1 + hashes);
        }
        j += 1;
    }
    Some(b.len())
}

/// 代码数字字面量:0x/0o/0b 前缀或十进制(下划线分隔、小数、指数),紧贴的
/// 标识符字节(类型后缀 i32 / u8 / f64 / n / ul…)并入;返回末端(恒 > 起点)。
fn scan_code_number(b: &[u8], start: usize) -> usize {
    let mut i = start;
    if b.get(i) == Some(&b'0') && matches!(b.get(i + 1), Some(b'x' | b'X' | b'o' | b'O' | b'b' | b'B')) {
        i += 2;
        while i < b.len() && (b[i].is_ascii_hexdigit() || b[i] == b'_') {
            i += 1;
        }
    } else {
        while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
            i += 1;
        }
        if b.get(i) == Some(&b'.') && b.get(i + 1).is_some_and(u8::is_ascii_digit) {
            i += 1;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
                i += 1;
            }
        }
        if matches!(b.get(i), Some(b'e' | b'E')) {
            let mut j = i + 1;
            if matches!(b.get(j), Some(b'+' | b'-')) {
                j += 1;
            }
            if b.get(j).is_some_and(u8::is_ascii_digit) {
                i = j;
            }
        }
    }
    while i < b.len() && is_ident_byte(b[i]) {
        i += 1;
    }
    i.max(start + 1)
}

/// 标识符整段扫到末端(起点须是 ASCII 字母或下划线)。
fn scan_ident_end(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && is_ident_byte(b[i]) {
        i += 1;
    }
    i
}

/// 标识符起始字节:ASCII 字母或下划线。
fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

/// 标识符字节:ASCII 字母数字下划线。
fn is_ident_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// 行首判定(可前置空白):预处理指令用。
fn at_line_start(b: &[u8], i: usize) -> bool {
    let mut j = i;
    while j > 0 {
        j -= 1;
        match b[j] {
            b' ' | b'\t' => continue,
            b'\n' => return true,
            _ => return false,
        }
    }
    true
}

/// UTF-8 字符长度(按首字节;延续/非法字节按 1,调用方逐字节推进兜底)。
fn utf8_len(first: u8) -> usize {
    match first {
        0xF0..=0xF7 => 4,
        0xE0..=0xEF => 3,
        0xC0..=0xDF => 2,
        _ => 1,
    }
}
