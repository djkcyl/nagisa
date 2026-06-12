//! 标记语言解析器 —— Markdown 基底(标题 / 列表 / 任务列表 / 引用 / 代码 / 分割线 / 表格 /
//! 链接)+ 少量扩展(`==高亮==`、`[文字]{属性}`、`::: 对齐` / `::: columns` 围栏),把标记文本
//! 解析成 [`Document`](crate::Document)。行式扫描:块级在 [`parse_blocks`],嵌套(引用 / 列表项 /
//! 围栏)靠**抽出内层行 + 递归**实现。行内解析在 [`inline`](mod@inline)。
//!
//! 解析很宽容:认不出的写法退化成普通文字,基本不报错(签名仍返回 `Result` 以备将来收严)。
//! 引用 `>` 后的空格可省;分割线认 `---` / `***` / `___`(3 个起步)。

use crate::error::Result;
use crate::model::{
    Align, Block, BlockImage, Cell, ColSpec, Color, Column, Columns, Document, ImageBorder, ImageSource, List,
    ListItem, ListKind, Panel, PanelDecor, Shadow, Table, TableStyle,
};

mod attrs;
mod inline;

pub(crate) use attrs::{parse_attrs, Attr};

/// 解析标记文本为文档。
pub fn parse(src: &str) -> Result<Document> {
    let lines: Vec<String> = src.lines().map(|l| l.to_string()).collect();
    Ok(Document { blocks: parse_blocks(&lines) })
}

/// 前导空白的字节数(空格 / Tab 各计 1)。
fn indent_of(s: &str) -> usize {
    s.len() - s.trim_start().len()
}

/// 去掉至多 `n` 个前导空格。
fn dedent(s: &str, n: usize) -> String {
    let strip = s.bytes().take_while(|b| *b == b' ').count().min(n);
    s[strip..].to_string()
}

/// 把一串行解析成块序列。
fn parse_blocks(lines: &[String]) -> Vec<Block> {
    parse_blocks_at(lines, 0)
}

/// 嵌套容器(引用 / 围栏 / 列表 / 栏)的最大递归深度:外部输入构造的深嵌套
/// 会真把解析栈打爆(abort 不可捕获),超限的内层一律按普通段落收。
const MAX_DEPTH: usize = 64;

fn parse_blocks_at(lines: &[String], depth: usize) -> Vec<Block> {
    if depth > MAX_DEPTH {
        let text = lines.join("\n");
        let t = text.trim();
        if t.is_empty() {
            return Vec::new();
        }
        return vec![Block::Paragraph { inlines: inline::parse_inlines(t), align: Align::Left }];
    }
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = &lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        let ind = indent_of(line);
        let content = line[ind..].to_string();

        // 代码围栏 ```lang ... ```(开栏反引号可多于 3,闭栏须同长及以上且不带别的字)
        if content.starts_with("```") {
            let ticks = content.bytes().take_while(|b| *b == b'`').count();
            let lang = content[ticks..].trim().to_string();
            let mut text = Vec::new();
            i += 1;
            while i < lines.len() && !is_code_fence_close(&lines[i], ticks) {
                text.push(lines[i].clone());
                i += 1;
            }
            i += 1; // 跳过闭合（缺失也无妨）
            blocks.push(Block::Code { lang: if lang.is_empty() { None } else { Some(lang) }, text: text.join("\n") });
            continue;
        }

        // 围栏 ::: word ... :::(支持嵌套)。word=对齐 → 对齐下沉;word=columns → 并排栏;
        // word=panel → 面板(可带 `{bg=… border=… rounded=…}` 装饰属性)。
        if is_fence_open(&content) {
            let (word, attrs) = split_fence_word(content[3..].trim());
            let inner = gather_div(lines, &mut i); // i 已跳到闭合之后
            if word == "columns" {
                let (cols, mut stray) = parse_columns(&inner, depth + 1);
                blocks.push(Block::Columns(Columns { cols, gap: None }));
                blocks.append(&mut stray); // 栏外散行不丢:排在栏块之后
            } else if word == "panel" {
                blocks.push(Block::Panel(Panel {
                    blocks: parse_blocks_at(&inner, depth + 1),
                    decor: panel_decor(attrs),
                }));
            } else if let Some(align) = align_from_word(&word) {
                let mut sub = parse_blocks_at(&inner, depth + 1);
                apply_align(&mut sub, align);
                blocks.append(&mut sub);
            } else {
                blocks.append(&mut parse_blocks_at(&inner, depth + 1)); // 未知围栏:透明容器
            }
            continue;
        }

        // 标题 #..######
        if let Some((level, rest)) = heading(&content) {
            let (text, align) = split_trailing_attrs(rest);
            blocks.push(Block::Heading { level, inlines: inline::parse_inlines(&text), align });
            i += 1;
            continue;
        }

        // 分割线 ---(也认 *** / ___,3 个起步的同字符行)
        if is_hr(&content) {
            blocks.push(Block::Divider);
            i += 1;
            continue;
        }

        // 引用 > ...(`>` 后的一个空格可省;`>>` 嵌套靠递归)
        if content.starts_with('>') {
            let mut inner = Vec::new();
            while i < lines.len() {
                let t = lines[i].trim_start();
                let Some(r) = t.strip_prefix('>') else { break };
                inner.push(r.strip_prefix(' ').unwrap_or(r).to_string());
                i += 1;
            }
            blocks.push(Block::Quote(parse_blocks_at(&inner, depth + 1)));
            continue;
        }

        // 块级图 ![cap](src) 单独成行
        if let Some(img) = block_image(&content) {
            blocks.push(Block::Image(img));
            i += 1;
            continue;
        }

        // 列表
        if list_marker(&content).is_some() {
            let (list, next) = parse_list(lines, i, ind);
            blocks.push(Block::List(list));
            i = next;
            continue;
        }

        // 表格(GFM):本行含 `|`,且下一行是分隔行(:?-+:?)。
        if content.contains('|')
            && i + 1 < lines.len()
            && is_table_delim(lines[i + 1].trim())
            && split_row(lines[i + 1].trim()).len() == split_row(content.trim()).len()
        {
            let (table, next) = parse_table(lines, i);
            blocks.push(Block::Table(table));
            i = next;
            continue;
        }

        // 段落:聚合连续的普通行。行尾 `\` = 硬换行(往缓冲塞 `\n`,行内解析时变 LineBreak)。
        let mut para = String::new();
        while i < lines.len() {
            let l = &lines[i];
            if l.trim().is_empty() {
                break;
            }
            let c = l[indent_of(l)..].to_string();
            if is_block_start(&c) {
                break;
            }
            let mut piece = c.trim();
            let hard = piece.ends_with('\\');
            if hard {
                piece = piece[..piece.len() - 1].trim_end();
            }
            append_soft(&mut para, piece);
            if hard {
                para.push('\n');
            }
            i += 1;
        }
        let (text, align) = split_trailing_attrs(&para);
        blocks.push(Block::Paragraph { inlines: inline::parse_inlines(&text), align });
    }
    blocks
}

/// 某行(去前导空白后的内容)是否是一个非段落块的起始。用于段落聚合时及时收住。
fn is_block_start(c: &str) -> bool {
    c.starts_with("```")
        || is_fence_open(c)
        || is_hr(c)
        || c.starts_with('>')
        || heading(c).is_some()
        || list_marker(c).is_some()
        || block_image(c).is_some()
}

/// 分割线行:3 个起步、清一色的 `-` / `*` / `_`。
fn is_hr(c: &str) -> bool {
    let b = c.as_bytes();
    b.len() >= 3 && matches!(b[0], b'-' | b'*' | b'_') && b.iter().all(|x| *x == b[0])
}

/// 解析一个列表(从 `lines[start]` 起、缩进 `base`),返回列表与下一行下标。
/// 列表项内容(含更深缩进的续行 / 子列表)抽出后递归 [`parse_blocks`]。
fn parse_list(lines: &[String], start: usize, base: usize) -> (List, usize) {
    let (ordered, first_start, _) = list_marker(&lines[start][base..]).unwrap();
    let kind = if ordered { ListKind::Ordered } else { ListKind::Unordered };
    let mut items = Vec::new();
    let mut i = start;
    while i < lines.len() {
        let line = &lines[i];
        if line.trim().is_empty() {
            // 项间空行:后面还有同级 / 更深内容才算列表内部,否则列表结束。
            if next_nonblank_indent(lines, i + 1).map(|n| n >= base).unwrap_or(false) {
                i += 1;
                continue;
            }
            break;
        }
        let ind = indent_of(line);
        if ind < base {
            break;
        }
        let Some((ord, _, off)) = list_marker(&line[ind..]) else {
            break; // 同 / 深缩进但不是 marker → 列表到此为止
        };
        if ind != base || ord != ordered {
            break; // 更深缩进的 marker 归上一项续行;有序 / 无序切换则另起一个列表
        }
        // 收本项:首行内容 + 后续「更深缩进 / 空行」的续行(去掉本项内容缩进)。
        let content_indent = base + off;
        let (first_line, check) = split_task_mark(&line[ind..][off..]);
        let mut item_lines = vec![first_line];
        i += 1;
        while i < lines.len() {
            let l = &lines[i];
            if l.trim().is_empty() {
                if next_nonblank_indent(lines, i + 1).map(|n| n > base).unwrap_or(false) {
                    item_lines.push(String::new());
                    i += 1;
                    continue;
                }
                break;
            }
            if indent_of(l) > base {
                item_lines.push(dedent(l, content_indent));
                i += 1;
            } else {
                break;
            }
        }
        items.push(ListItem { blocks: parse_blocks(&item_lines), check });
    }
    (List { kind, start: first_start.max(1), items }, i)
}

/// 摘掉项首的任务标记 `[ ]` / `[x]` / `[X]`(GFM 任务列表),返回 `(剩余内容, 完成态)`。
/// 标记后须是空白或行尾;不是任务标记则原样返回。
fn split_task_mark(s: &str) -> (String, Option<bool>) {
    let done = match s.get(..3) {
        Some("[ ]") => false,
        Some("[x]") | Some("[X]") => true,
        _ => return (s.to_string(), None),
    };
    match s[3..].chars().next() {
        None => (String::new(), Some(done)),
        Some(c) if c.is_whitespace() => (s[3 + c.len_utf8()..].to_string(), Some(done)),
        _ => (s.to_string(), None),
    }
}

/// 之后第一条非空行的缩进(没有则 `None`)。
fn next_nonblank_indent(lines: &[String], from: usize) -> Option<usize> {
    lines[from..].iter().find(|l| !l.trim().is_empty()).map(|l| indent_of(l))
}

/// 标题:前导 1..=6 个 `#` 且其后跟空格。返回 `(level, 标题文字)`。
fn heading(c: &str) -> Option<(u8, &str)> {
    let hashes = c.bytes().take_while(|b| *b == b'#').count();
    if (1..=6).contains(&hashes) && c.as_bytes().get(hashes) == Some(&b' ') {
        Some((hashes as u8, c[hashes + 1..].trim()))
    } else {
        None
    }
}

/// 列表 marker:返回 `(是否有序, 起始序号, marker 含尾分隔的宽度)`。marker 与内容间空格或 Tab 都认。
fn list_marker(c: &str) -> Option<(bool, u32, usize)> {
    let b = c.as_bytes();
    // 无序:- / * / + 后跟空格或 Tab
    if matches!(b.first(), Some(b'-' | b'*' | b'+')) && matches!(b.get(1), Some(b' ' | b'\t')) {
        return Some((false, 0, 2));
    }
    // 有序:数字 + ('.'|')') + (空格|Tab)
    let digits = c.bytes().take_while(|x| x.is_ascii_digit()).count();
    if digits > 0 && matches!(b.get(digits), Some(b'.' | b')')) && matches!(b.get(digits + 1), Some(b' ' | b'\t')) {
        let n = c[..digits].parse::<u32>().unwrap_or(1);
        return Some((true, n, digits + 2));
    }
    None
}

/// 块级图 `![cap](src)`(整行)。`src` 以 `@` 开头 → 具名引用,否则按磁盘路径。
fn block_image(c: &str) -> Option<BlockImage> {
    let c = c.trim();
    let rest = c.strip_prefix("![")?;
    let close_alt = rest.find("](")?;
    let after_src = &rest[close_alt + 2..];
    let close_paren = after_src.find(')')?;
    let src = &after_src[..close_paren];
    if src.is_empty() {
        return None;
    }
    // 右括号后只允许空白或 `{属性}`,有别的尾巴就不是块级图(退回段落,不吞文字)。
    let tail = after_src[close_paren + 1..].trim();
    let attrs = if tail.is_empty() {
        ""
    } else if tail.starts_with('{') && tail.ends_with('}') {
        &tail[1..tail.len() - 1]
    } else {
        return None;
    };
    let alt = &rest[..close_alt];
    let mut img = BlockImage {
        src: image_source(src),
        width: None,
        align: Align::Left,
        caption: if alt.trim().is_empty() { None } else { Some(inline::parse_inlines(alt.trim())) },
        decor: crate::model::ImageDecor::default(),
    };
    apply_image_attrs(&mut img, attrs);
    Some(img)
}

/// 块级图尾部属性:`width=50%|320`(百分比或逻辑像素)、`align=center|right|left`、
/// `rounded=px`、`shadow`(标志)、`border=#hex`(线宽固定 2,要细调走构建器)。
fn apply_image_attrs(img: &mut BlockImage, attrs: &str) {
    for a in parse_attrs(attrs) {
        match a {
            Attr::Kv(k, v) => match k.as_str() {
                "width" => {
                    if let Some(pct) = v.strip_suffix('%') {
                        if let Ok(x) = pct.parse::<f32>() {
                            if x.is_finite() && x > 0.0 {
                                img.width = Some(crate::model::Length::Percent(x));
                            }
                        }
                    } else if let Ok(x) = v.parse::<f32>() {
                        if x.is_finite() && x > 0.0 {
                            img.width = Some(crate::model::Length::Px(x));
                        }
                    }
                }
                "align" => {
                    if let Some(al) = align_from_word(&v) {
                        img.align = al;
                    }
                }
                "rounded" => {
                    if let Ok(r) = v.parse::<f32>() {
                        if r.is_finite() && r > 0.0 {
                            img.decor.radius = r;
                        }
                    }
                }
                "border" => {
                    if let Some(color) = Color::hex(&v) {
                        img.decor.border = Some(ImageBorder { width: 2.0, color });
                    }
                }
                _ => {}
            },
            Attr::Flag(f) => {
                if f == "shadow" {
                    img.decor.shadow = Some(Shadow::default());
                }
            }
        }
    }
}

/// `@名字` → `Named`,否则 `Path`。
pub(crate) fn image_source(src: &str) -> ImageSource {
    match src.strip_prefix('@') {
        Some(name) => ImageSource::Named(name.to_string()),
        None => ImageSource::Path(src.into()),
    }
}

/// 把对齐词转成 [`Align`]。
fn align_from_word(w: &str) -> Option<Align> {
    match w {
        "center" | "centre" => Some(Align::Center),
        "right" => Some(Align::Right),
        "left" => Some(Align::Left),
        "justify" => Some(Align::Justify),
        _ => None,
    }
}

/// 是不是一个围栏开启行(`::: word`,word 非空)。裸 `:::` 是闭合,不算开启。
fn is_fence_open(c: &str) -> bool {
    c.starts_with(":::") && c.len() > 3 && !c[3..].trim().is_empty()
}

/// 从围栏开启行(`lines[*i]`)起,深度感知地收集内层行,`*i` 推进到匹配闭合 `:::` 之后。
fn gather_div(lines: &[String], i: &mut usize) -> Vec<String> {
    *i += 1;
    let mut inner = Vec::new();
    let mut depth = 1usize;
    let mut code_ticks = 0usize; // > 0 = 在代码围栏里,::: 不算数
    while *i < lines.len() {
        let t = lines[*i].trim();
        if code_ticks > 0 {
            if is_code_fence_close(&lines[*i], code_ticks) {
                code_ticks = 0;
            }
        } else if t.starts_with("```") {
            code_ticks = t.bytes().take_while(|b| *b == b'`').count();
        } else if t == ":::" {
            depth -= 1;
            if depth == 0 {
                *i += 1;
                break; // 匹配闭合不计入内层
            }
        } else if is_fence_open(t) {
            depth += 1;
        }
        inner.push(lines[*i].clone());
        *i += 1;
    }
    inner
}

/// 代码围栏闭合行:去缩进后是 ≥ `ticks` 枚反引号、且没有别的非空内容。
fn is_code_fence_close(line: &str, ticks: usize) -> bool {
    let t = line.trim();
    let n = t.bytes().take_while(|b| *b == b'`').count();
    n >= ticks && t[n..].trim().is_empty()
}

/// 把 `::: columns` 的内层解析成若干栏:每个直接的 `::: col [权重]` 子围栏一栏。
fn parse_columns(inner: &[String], depth: usize) -> (Vec<Column>, Vec<Block>) {
    let mut cols = Vec::new();
    let mut stray_lines: Vec<String> = Vec::new();
    let mut i = 0;
    while i < inner.len() {
        let (head, attrs) = split_fence_word(inner[i].trim().strip_prefix(":::").unwrap_or("").trim());
        let mut parts = head.split_whitespace();
        if parts.next() == Some("col") {
            let weight =
                parts.next().and_then(|s| s.parse::<f32>().ok()).filter(|w| w.is_finite() && *w > 0.0).unwrap_or(1.0);
            let col_lines = gather_div(inner, &mut i);
            let mut blocks = parse_blocks_at(&col_lines, depth);
            // 带装饰属性的栏 = 整栏一个面板(layout 把它拉齐到本行最高栏)。
            if !attrs.is_empty() {
                blocks = vec![Block::Panel(Panel { blocks, decor: panel_decor(attrs) })];
            }
            cols.push(Column { blocks, weight });
        } else {
            stray_lines.push(inner[i].clone()); // 栏外行收着,随后按普通块解析
            i += 1;
        }
    }
    (cols, parse_blocks_at(&stray_lines, depth))
}

/// 围栏开启词拆成「词(含权重等)+ `{}` 内的属性串」;无属性时属性串为空。
fn split_fence_word(s: &str) -> (String, &str) {
    match (s.find('{'), s.rfind('}')) {
        (Some(a), Some(b)) if b > a => (s[..a].trim().to_string(), &s[a + 1..b]),
        _ => (s.trim().to_string(), ""),
    }
}

/// 解析面板装饰属性:`bg=#hex`、`border=#hex`、`border-width=px`(默认 1.5)、
/// `rounded=px`、`pad=px`、`shadow`(标志)。非法值忽略。
fn panel_decor(attrs: &str) -> PanelDecor {
    let mut d = PanelDecor::default();
    let mut border_color: Option<Color> = None;
    let mut border_width = 1.5f32;
    for a in parse_attrs(attrs) {
        match a {
            Attr::Kv(k, v) => match k.as_str() {
                "bg" => d.bg = Color::hex(&v).or(d.bg),
                "border" => border_color = Color::hex(&v).or(border_color),
                "border-width" => {
                    if let Ok(w) = v.parse::<f32>() {
                        if w.is_finite() && w > 0.0 {
                            border_width = w;
                        }
                    }
                }
                "rounded" => {
                    if let Ok(r) = v.parse::<f32>() {
                        if r.is_finite() && r >= 0.0 {
                            d.radius = Some(r);
                        }
                    }
                }
                "pad" => {
                    if let Ok(p) = v.parse::<f32>() {
                        if p.is_finite() && p >= 0.0 {
                            d.pad = Some(p);
                        }
                    }
                }
                _ => {}
            },
            Attr::Flag(f) => {
                if f == "shadow" {
                    d.shadow = Some(Shadow::default());
                }
            }
        }
    }
    d.border = border_color.map(|color| ImageBorder { width: border_width, color });
    d
}

/// GFM 表格分隔行?每个非空单元格只含 `-`/`:` 且至少一个 `-`。
fn is_table_delim(t: &str) -> bool {
    let cells = split_row(t);
    !cells.is_empty()
        && cells.iter().all(|c| !c.is_empty() && c.contains('-') && c.bytes().all(|b| b == b'-' || b == b':'))
}

/// 按 `|` 切一行的单元格(去掉首尾的 `|`,各段去空白)。
/// `\|` 转义竖线与 `` `行内码` `` 内的竖线不当列分隔(转义本身留给行内解析处理)。
fn split_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut in_code = false;
    let mut chars = t.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '`' => {
                in_code = !in_code;
                cur.push('`');
            }
            // 保留 `\X`(含 `\|`):其中的 `|` 不算列分隔,转义语义交给行内解析。
            '\\' if !in_code => {
                cur.push('\\');
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            '|' if !in_code => {
                cells.push(cur.trim().to_string());
                cur = String::new();
            }
            _ => cur.push(ch),
        }
    }
    cells.push(cur.trim().to_string());
    cells
}

/// 分隔行 → 各列对齐(`:--` 左 / `:-:` 中 / `--:` 右)。
fn parse_align_row(line: &str) -> Vec<Align> {
    split_row(line)
        .iter()
        .map(|c| match (c.starts_with(':'), c.ends_with(':')) {
            (true, true) => Align::Center,
            (false, true) => Align::Right,
            _ => Align::Left,
        })
        .collect()
}

/// 解析一张 GFM 表格(`start` 表头行,`start+1` 分隔行,之后是数据行直到空行 / 无 `|` 行)。
fn parse_table(lines: &[String], start: usize) -> (Table, usize) {
    let to_cells = |t: &str| -> Vec<Cell> {
        split_row(t).iter().map(|s| Cell { inlines: inline::parse_inlines(s), bg: None }).collect()
    };
    let header = Some(to_cells(lines[start].trim()));
    let cols: Vec<ColSpec> =
        parse_align_row(lines[start + 1].trim()).into_iter().map(|a| ColSpec { align: a, width: None }).collect();
    let mut rows = Vec::new();
    let mut i = start + 2;
    while i < lines.len() {
        let t = lines[i].trim();
        if t.is_empty() || !t.contains('|') {
            break;
        }
        rows.push(to_cells(t));
        i += 1;
    }
    (Table { header, rows, cols, style: TableStyle::default() }, i)
}

/// 给一串块整体设对齐(围栏对齐下沉用):标题 / 段落直接设;引用 / 列表项 / 面板递归下沉。
fn apply_align(blocks: &mut [Block], align: Align) {
    for b in blocks {
        match b {
            Block::Heading { align: a, .. } | Block::Paragraph { align: a, .. } => *a = align,
            Block::Quote(inner) => apply_align(inner, align),
            Block::Panel(p) => apply_align(&mut p.blocks, align),
            Block::Image(bi) => bi.align = align,
            Block::List(list) => {
                for it in &mut list.items {
                    apply_align(&mut it.blocks, align);
                }
            }
            _ => {}
        }
    }
}

/// 从文字尾部摘出 `{属性}`(要求 `{` 前是空白),解析其中的 `align`。返回 `(正文, 对齐)`。
fn split_trailing_attrs(s: &str) -> (String, Align) {
    let t = s.trim_end();
    if t.ends_with('}') {
        if let Some(open) = t.rfind('{') {
            let before = &t[..open];
            if before.ends_with(' ') || before.is_empty() {
                let inside = &t[open + 1..t.len() - 1];
                // 只认得 align:认不出的 {…} 保留为正文,不吞。
                if let Some(align) = parse_attrs(inside).iter().find_map(|a| match a {
                    Attr::Kv(k, v) if k == "align" => align_from_word(v),
                    Attr::Flag(f) => align_from_word(f),
                    _ => None,
                }) {
                    return (before.trim_end().to_string(), align);
                }
            }
        }
    }
    (t.to_string(), Align::Left)
}

/// 段落软换行拼接:两侧都非 CJK 才插空格(CJK 行间不加空格)。
fn append_soft(buf: &mut String, next: &str) {
    if next.is_empty() {
        return;
    }
    if let (Some(a), Some(b)) = (buf.chars().last(), next.chars().next()) {
        // 紧跟硬换行(`\n`)后不加前导空格;否则两侧都非 CJK 才插空格。
        if a != '\n' && needs_space(a, b) {
            buf.push(' ');
        }
    }
    buf.push_str(next);
}

fn needs_space(a: char, b: char) -> bool {
    // CJK 标点 / 符号 / 表意文字(含 2E80–9FFF)+ 全角形(FF00–FFEF)。
    fn cjk(c: char) -> bool {
        matches!(c, '\u{2E80}'..='\u{9FFF}' | '\u{FF00}'..='\u{FFEF}')
    }
    !cjk(a) && !cjk(b)
}
