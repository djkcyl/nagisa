//! Rust 构建器 API —— 用链式调用拼出一个 [`Document`](crate::Document),作为 markup 之外的
//! 另一前端(从结构化数据生成文档时更顺手)。两前端产物相同。
//!
//! 风格:块级构建器([`Doc`] / [`ListBuilder`])用 `&mut self -> &mut Self` 链式;子内容
//! 用闭包配置(`|p| ...` 拿到 `&mut` 子构建器,表达式或语句写法都行,返回值忽略)。
//!
//! ```ignore
//! let doc = Doc::new()
//!     .heading(1, |h| h.align(Align::Center).text("月度报告"))
//!     .paragraph(|p| { p.bold("本月").text("亮点:").highlight("达标"); })
//!     .list(ListKind::Unordered, |l| { l.item(|i| i.text("任务一")); })
//!     .divider()
//!     .build();
//! ```

use std::path::PathBuf;

use crate::model::{
    Align, Anchor, Badge, Block, BlockImage, Cell, ColSpec, Color, Column, Columns, Document,
    FontRole, Highlight, ImageBorder, ImageDecor, Inline, Length, List, ListItem, ListKind,
    Progress, Shadow, Table, TableStyle, TextStyle, Watermark,
};

/// 文档 / 块序列构建器。也用作引用、列表项的内层块容器。
#[derive(Default)]
pub struct Doc {
    blocks: Vec<Block>,
}

impl Doc {
    /// 新建一个空文档构建器。
    pub fn new() -> Self {
        Self { blocks: Vec::new() }
    }

    /// 收尾成 [`Document`]。
    pub fn build(&self) -> Document {
        Document { blocks: self.blocks.clone() }
    }

    /// 标题(`level` 取 1..=6,越界夹到范围)。
    pub fn heading<R>(&mut self, level: u8, f: impl FnOnce(&mut ParaBuilder) -> R) -> &mut Self {
        let mut pb = ParaBuilder::new();
        let _ = f(&mut pb);
        self.blocks.push(Block::Heading {
            level: level.clamp(1, 6),
            inlines: pb.inlines,
            align: pb.align,
        });
        self
    }

    /// 段落。
    pub fn paragraph<R>(&mut self, f: impl FnOnce(&mut ParaBuilder) -> R) -> &mut Self {
        let mut pb = ParaBuilder::new();
        let _ = f(&mut pb);
        self.blocks.push(Block::Paragraph { inlines: pb.inlines, align: pb.align });
        self
    }

    /// 便捷:一行纯文字段落。
    pub fn text(&mut self, s: impl Into<String>) -> &mut Self {
        self.paragraph(|p| {
            p.text(s);
        })
    }

    /// 引用块(内层是块容器,可嵌套)。
    pub fn quote<R>(&mut self, f: impl FnOnce(&mut Doc) -> R) -> &mut Self {
        let mut inner = Doc::new();
        let _ = f(&mut inner);
        self.blocks.push(Block::Quote(inner.blocks));
        self
    }

    /// 列表(有序 / 无序)。
    pub fn list<R>(&mut self, kind: ListKind, f: impl FnOnce(&mut ListBuilder) -> R) -> &mut Self {
        let mut lb = ListBuilder { kind, start: 1, items: Vec::new() };
        let _ = f(&mut lb);
        self.blocks.push(Block::List(List { kind: lb.kind, start: lb.start, items: lb.items }));
        self
    }

    /// 代码块。`lang` 空串 = 无语言标签。
    pub fn code(&mut self, lang: impl Into<String>, text: impl Into<String>) -> &mut Self {
        let lang = lang.into();
        self.blocks.push(Block::Code {
            lang: if lang.is_empty() { None } else { Some(lang) },
            text: text.into(),
        });
        self
    }

    /// 分割线。
    pub fn divider(&mut self) -> &mut Self {
        self.blocks.push(Block::Divider);
        self
    }

    /// 显式并排栏:闭包里用 `.col(..)` / `.col_weighted(w, ..)` 加栏。
    pub fn columns<R>(&mut self, f: impl FnOnce(&mut ColumnsBuilder) -> R) -> &mut Self {
        let mut cb = ColumnsBuilder { gap: None, cols: Vec::new() };
        let _ = f(&mut cb);
        self.blocks.push(Block::Columns(Columns { cols: cb.cols, gap: cb.gap }));
        self
    }

    /// 表格:闭包里用 `.head([..])` / `.row([..])` / `.align([..])` / `.width(列, 长)`。
    pub fn table<R>(&mut self, f: impl FnOnce(&mut TableBuilder) -> R) -> &mut Self {
        let mut tb = TableBuilder {
            header: None,
            rows: Vec::new(),
            cols: Vec::new(),
            style: TableStyle::default(),
        };
        let _ = f(&mut tb);
        self.blocks.push(Block::Table(Table {
            header: tb.header,
            rows: tb.rows,
            cols: tb.cols,
            style: tb.style,
        }));
        self
    }

    /// 进度条:`value` 取 0–1(越界渲染时夹取),样式经闭包调
    /// (`.height(..)` / `.fill(..)` / `.track(..)` / `.radius(..)` / `.width_px(..)` /
    /// `.width_percent(..)` / `.align(..)`),全缺省即「铺满内容宽的胶囊条,主题强调色」。
    pub fn progress<R>(&mut self, value: f32, f: impl FnOnce(&mut ProgressBuilder) -> R) -> &mut Self {
        let mut pb = ProgressBuilder {
            p: Progress {
                value,
                height: 10.0,
                fill: None,
                track: None,
                radius: None,
                width: None,
                align: Align::Left,
            },
        };
        let _ = f(&mut pb);
        self.blocks.push(Block::Progress(pb.p));
        self
    }

    /// 块级图(字节来源)。
    pub fn image_bytes<R>(
        &mut self,
        bytes: Vec<u8>,
        f: impl FnOnce(&mut ImageBuilder) -> R,
    ) -> &mut Self {
        self.push_block_image(ImageSource::Bytes(bytes), f)
    }

    /// 块级图(磁盘路径)。
    pub fn image_path<R>(
        &mut self,
        path: impl Into<PathBuf>,
        f: impl FnOnce(&mut ImageBuilder) -> R,
    ) -> &mut Self {
        self.push_block_image(ImageSource::Path(path.into()), f)
    }

    fn push_block_image<R>(
        &mut self,
        src: ImageSource,
        f: impl FnOnce(&mut ImageBuilder) -> R,
    ) -> &mut Self {
        let mut ib = ImageBuilder {
            width: None,
            align: Align::Left,
            caption: None,
            decor: ImageDecor::default(),
        };
        let _ = f(&mut ib);
        self.blocks.push(Block::Image(BlockImage {
            src,
            width: ib.width,
            align: ib.align,
            caption: ib.caption,
            decor: ib.decor,
        }));
        self
    }
}

use crate::model::ImageSource;

/// 段落 / 标题的行内内容构建器(也用于图注)。
pub struct ParaBuilder {
    inlines: Vec<Inline>,
    align: Align,
}

impl ParaBuilder {
    pub(crate) fn new() -> Self {
        Self { inlines: Vec::new(), align: Align::Left }
    }

    /// 取走已累积的行内序列(页眉/页脚的富文本构造用)。
    pub(crate) fn into_inlines(self) -> Vec<Inline> {
        self.inlines
    }

    /// 设对齐。
    pub fn align(&mut self, a: Align) -> &mut Self {
        self.align = a;
        self
    }

    /// 普通文字。
    pub fn text(&mut self, s: impl Into<String>) -> &mut Self {
        self.push(s, TextStyle::default())
    }

    /// 粗体文字。
    pub fn bold(&mut self, s: impl Into<String>) -> &mut Self {
        self.push(s, TextStyle { weight: Some(700), ..Default::default() })
    }

    /// 细体文字(字重 300)。
    pub fn light(&mut self, s: impl Into<String>) -> &mut Self {
        self.push(s, TextStyle { weight: Some(300), ..Default::default() })
    }

    /// 斜体文字。
    pub fn italic(&mut self, s: impl Into<String>) -> &mut Self {
        self.push(s, TextStyle { italic: true, ..Default::default() })
    }

    /// 下划线文字。
    pub fn underline(&mut self, s: impl Into<String>) -> &mut Self {
        self.push(s, TextStyle { underline: true, ..Default::default() })
    }

    /// 删除线文字。
    pub fn strike(&mut self, s: impl Into<String>) -> &mut Self {
        self.push(s, TextStyle { strike: true, ..Default::default() })
    }

    /// 高亮文字(主题默认高亮色)。
    pub fn highlight(&mut self, s: impl Into<String>) -> &mut Self {
        self.push(s, TextStyle { highlight: Some(Highlight::Theme), ..Default::default() })
    }

    /// 指定色文字(十六进制;非法则用默认色)。
    pub fn color(&mut self, hex: &str, s: impl Into<String>) -> &mut Self {
        self.push(s, TextStyle { color: Color::hex(hex), ..Default::default() })
    }

    /// 行内代码。
    pub fn code(&mut self, s: impl Into<String>) -> &mut Self {
        self.inlines.push(Inline::Code(s.into()));
        self
    }

    /// 任意样式文字:闭包里配置 [`StyleBuilder`]。
    pub fn styled<R>(
        &mut self,
        s: impl Into<String>,
        f: impl FnOnce(&mut StyleBuilder) -> R,
    ) -> &mut Self {
        let mut sb = StyleBuilder { style: TextStyle::default() };
        let _ = f(&mut sb);
        self.push(s, sb.style)
    }

    /// 硬换行。
    pub fn line_break(&mut self) -> &mut Self {
        self.inlines.push(Inline::LineBreak);
        self
    }

    fn push(&mut self, s: impl Into<String>, style: TextStyle) -> &mut Self {
        self.inlines.push(Inline::Text { text: s.into(), style });
        self
    }
}

/// 文字样式构建器(给 [`ParaBuilder::styled`])。
pub struct StyleBuilder {
    style: TextStyle,
}

impl StyleBuilder {
    /// 加粗(字重 700)。
    pub fn bold(&mut self) -> &mut Self {
        self.style.weight = Some(700);
        self
    }
    /// 细体(字重 300)。
    pub fn light(&mut self) -> &mut Self {
        self.style.weight = Some(300);
        self
    }
    /// 任意字重(CSS 习惯值 1–1000,常用 100–900;越界忽略)。
    pub fn weight(&mut self, w: u16) -> &mut Self {
        if (1..=1000).contains(&w) {
            self.style.weight = Some(w);
        }
        self
    }
    /// 斜体。
    pub fn italic(&mut self) -> &mut Self {
        self.style.italic = true;
        self
    }
    /// 下划线。
    pub fn underline(&mut self) -> &mut Self {
        self.style.underline = true;
        self
    }
    /// 删除线。
    pub fn strike(&mut self) -> &mut Self {
        self.style.strike = true;
        self
    }
    /// 文字色(十六进制;非法忽略)。
    pub fn color(&mut self, hex: &str) -> &mut Self {
        if let Some(c) = Color::hex(hex) {
            self.style.color = Some(c);
        }
        self
    }
    /// 高亮底色(十六进制;非法忽略)。
    pub fn bg(&mut self, hex: &str) -> &mut Self {
        if let Some(c) = Color::hex(hex) {
            self.style.highlight = Some(Highlight::Custom(c));
        }
        self
    }
    /// 字号倍率(相对基准)。非有限或 ≤ 0 忽略(保持默认),与标记前端一致——
    /// 避免 0 字号把 cosmic-text 整形拖进死循环。
    pub fn size(&mut self, mult: f32) -> &mut Self {
        if mult.is_finite() && mult > 0.0 {
            self.style.size = mult;
        }
        self
    }
    /// 字族角色。
    pub fn font(&mut self, role: FontRole) -> &mut Self {
        self.style.font = role;
        self
    }
    /// 圈注:以这段文字为中心画一圈椭圆描边(缺省按文字宽窄自适应、颜色跟随墨色;
    /// 不占布局尺寸,圈溢出到行距)。尺寸经 [`ring_radius`](Self::ring_radius) /
    /// [`ring_radii`](Self::ring_radii) 给定后与文字宽窄无关。
    pub fn ring(&mut self) -> &mut Self {
        self.style.ring.get_or_insert_default();
        self
    }
    /// 圈注描边色(十六进制;非法忽略,跟随墨色)。
    pub fn ring_color(&mut self, hex: &str) -> &mut Self {
        let r = self.style.ring.get_or_insert_default();
        r.color = Color::hex(hex).or(r.color);
        self
    }
    /// 圈注定径:**正圆**,半径 `r`(逻辑像素)——多字与单字圈出同样大的圈。
    pub fn ring_radius(&mut self, r: f32) -> &mut Self {
        self.ring_radii(r, r)
    }
    /// 圈注定径:**椭圆**,横/纵半径(逻辑像素)。非有限或 ≤ 0 的分量忽略(保持自适应)。
    pub fn ring_radii(&mut self, rx: f32, ry: f32) -> &mut Self {
        let r = self.style.ring.get_or_insert_default();
        if rx.is_finite() && rx > 0.0 {
            r.rx = Some(rx);
        }
        if ry.is_finite() && ry > 0.0 {
            r.ry = Some(ry);
        }
        self
    }
    /// 圈注线宽(逻辑像素;非有限或 ≤ 0 忽略,保持 0.07 倍字号缺省)。
    pub fn ring_stroke(&mut self, w: f32) -> &mut Self {
        let r = self.style.ring.get_or_insert_default();
        if w.is_finite() && w > 0.0 {
            r.width = Some(w);
        }
        self
    }
    /// 逐字圈:整段一字一圈(空白跳过),未定径时按字取**正圆**。缺省是整段一个圈
    /// (范围圈,自适应为扁椭圆)。
    pub fn ring_each(&mut self) -> &mut Self {
        self.style.ring.get_or_insert_default().each = true;
        self
    }
    /// 着重点:这段文字正下方一枚实心小点(颜色跟随墨色;画进行距,不占高度)。
    pub fn dot(&mut self) -> &mut Self {
        self.style.dot.get_or_insert_default();
        self
    }
    /// 着重点颜色(十六进制;非法忽略,跟随墨色)。
    pub fn dot_color(&mut self, hex: &str) -> &mut Self {
        let d = self.style.dot.get_or_insert_default();
        d.color = Color::hex(hex).or(d.color);
        self
    }
    /// 着重点半径(逻辑像素;非有限或 ≤ 0 忽略,保持 0.09 倍字号缺省)。
    pub fn dot_radius(&mut self, r: f32) -> &mut Self {
        let d = self.style.dot.get_or_insert_default();
        if r.is_finite() && r > 0.0 {
            d.radius = Some(r);
        }
        self
    }
    /// 逐字点:一字一点(中文着重号的正字法;空白跳过)。缺省是整段中线下一点。
    pub fn dot_each(&mut self) -> &mut Self {
        self.style.dot.get_or_insert_default().each = true;
        self
    }
    /// 文字阴影(默认形态:下坠 2 逻辑像素、软化 6、黑 25%)。
    /// 边注挂右:这段挂到本行内容的右外侧,参与绘制不参与布局——居中 / 对齐按其余
    /// 内容算,边注不挤不偏(「当前」「✓」这类行尾标记用)。
    pub fn aside_right(&mut self) -> &mut Self {
        self.style.aside = Some(crate::model::AsideSide::Right);
        self
    }
    /// 边注挂左:同 [`aside_right`](Self::aside_right),停靠行首左外侧。
    pub fn aside_left(&mut self) -> &mut Self {
        self.style.aside = Some(crate::model::AsideSide::Left);
        self
    }
    pub fn shadow(&mut self) -> &mut Self {
        self.style.shadow = Some(Shadow::default());
        self
    }
    /// 文字阴影(自定形态:偏移/软化为逻辑像素,色为十六进制,非法色忽略整条)。
    pub fn shadow_with(&mut self, dx: f32, dy: f32, blur: f32, hex: &str) -> &mut Self {
        if let Some(color) = Color::hex(hex) {
            self.style.shadow = Some(Shadow { dx, dy, blur: blur.max(0.0), color });
        }
        self
    }
}

/// 表格构建器(纯文字单元格)。
pub struct TableBuilder {
    header: Option<Vec<Cell>>,
    rows: Vec<Vec<Cell>>,
    cols: Vec<ColSpec>,
    style: TableStyle,
}

impl TableBuilder {
    /// 设表头。
    pub fn head<I, S>(&mut self, cells: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.header = Some(cells.into_iter().map(text_cell).collect());
        self
    }
    /// 加一数据行。
    pub fn row<I, S>(&mut self, cells: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.rows.push(cells.into_iter().map(text_cell).collect());
        self
    }
    /// 设各列对齐(从第 0 列起)。
    pub fn align<I: IntoIterator<Item = Align>>(&mut self, aligns: I) -> &mut Self {
        for (k, a) in aligns.into_iter().enumerate() {
            self.ensure_col(k).align = a;
        }
        self
    }
    /// 给某列(0 起)限宽。
    pub fn width(&mut self, col: usize, w: Length) -> &mut Self {
        self.ensure_col(col).width = Some(w);
        self
    }
    fn ensure_col(&mut self, k: usize) -> &mut ColSpec {
        while self.cols.len() <= k {
            self.cols.push(ColSpec::default());
        }
        &mut self.cols[k]
    }

    // ── 按列 / 行 / 格设文字样式 + 背景(在已加的单元格上叠加;先加行,再设样式) ──

    /// 整列(含表头)的文字样式。
    pub fn col_style<R>(&mut self, col: usize, f: impl Fn(&mut StyleBuilder) -> R) -> &mut Self {
        if let Some(h) = self.header.as_mut().and_then(|h| h.get_mut(col)) {
            style_cell(h, &f);
        }
        for row in &mut self.rows {
            if let Some(c) = row.get_mut(col) {
                style_cell(c, &f);
            }
        }
        self
    }
    /// 整行(数据行,0 起)的文字样式。
    pub fn row_style<R>(&mut self, row: usize, f: impl Fn(&mut StyleBuilder) -> R) -> &mut Self {
        if let Some(r) = self.rows.get_mut(row) {
            for c in r.iter_mut() {
                style_cell(c, &f);
            }
        }
        self
    }
    /// 单格(数据行 / 列,0 起)的文字样式。
    pub fn cell_style<R>(
        &mut self,
        row: usize,
        col: usize,
        f: impl Fn(&mut StyleBuilder) -> R,
    ) -> &mut Self {
        if let Some(c) = self.rows.get_mut(row).and_then(|r| r.get_mut(col)) {
            style_cell(c, &f);
        }
        self
    }
    /// 整列(含表头)背景填色。
    pub fn col_fill(&mut self, col: usize, hex: &str) -> &mut Self {
        let bg = Color::hex(hex);
        if let Some(h) = self.header.as_mut().and_then(|h| h.get_mut(col)) {
            h.bg = bg;
        }
        for row in &mut self.rows {
            if let Some(c) = row.get_mut(col) {
                c.bg = bg;
            }
        }
        self
    }
    /// 整行(数据行)背景填色。
    pub fn row_fill(&mut self, row: usize, hex: &str) -> &mut Self {
        let bg = Color::hex(hex);
        if let Some(r) = self.rows.get_mut(row) {
            for c in r.iter_mut() {
                c.bg = bg;
            }
        }
        self
    }
    /// 单格(数据行 / 列)背景填色。
    pub fn cell_fill(&mut self, row: usize, col: usize, hex: &str) -> &mut Self {
        if let Some(c) = self.rows.get_mut(row).and_then(|r| r.get_mut(col)) {
            c.bg = Color::hex(hex);
        }
        self
    }

    // ── 紧凑度 + 网格线 ──

    /// 列内边距(单元格左右,逻辑像素);越小列越紧凑。
    pub fn pad_x(&mut self, px: f32) -> &mut Self {
        self.style.pad_x = Some(px.max(0.0));
        self
    }
    /// 行内边距(单元格上下,逻辑像素);越小行越紧凑、行距越小。
    pub fn pad_y(&mut self, px: f32) -> &mut Self {
        self.style.pad_y = Some(px.max(0.0));
        self
    }
    /// 拉伸铺满可用宽(富余宽度按比例分给自适应列;全固定列则整体等比放大)。
    pub fn expand(&mut self) -> &mut Self {
        self.style.expand = true;
        self
    }
    /// 外框线开关。
    pub fn grid_outer(&mut self, on: bool) -> &mut Self {
        self.style.grid.outer = on;
        self
    }
    /// 列竖线开关。
    pub fn grid_vertical(&mut self, on: bool) -> &mut Self {
        self.style.grid.vertical = on;
        self
    }
    /// 行横线开关。
    pub fn grid_horizontal(&mut self, on: bool) -> &mut Self {
        self.style.grid.horizontal = on;
        self
    }
    /// 去掉所有网格线(外框 / 竖线 / 横线)。
    pub fn no_grid(&mut self) -> &mut Self {
        self.style.grid.outer = false;
        self.style.grid.vertical = false;
        self.style.grid.horizontal = false;
        self
    }
    /// 表头浅底开关。
    pub fn header_fill(&mut self, on: bool) -> &mut Self {
        self.style.header_fill = on;
        self
    }
}

/// 纯文字单元格。
fn text_cell(s: impl Into<String>) -> Cell {
    Cell { inlines: vec![Inline::Text { text: s.into(), style: TextStyle::default() }], bg: None }
}

/// 给一个单元格的所有文字段叠加样式(从各段现有样式起、合并闭包改动的字段)。
fn style_cell<R>(cell: &mut Cell, f: &impl Fn(&mut StyleBuilder) -> R) {
    for inl in &mut cell.inlines {
        if let Inline::Text { style, .. } = inl {
            let mut sb = StyleBuilder { style: style.clone() };
            let _ = f(&mut sb);
            *style = sb.style;
        }
    }
}

/// 并排栏构建器。
pub struct ColumnsBuilder {
    gap: Option<f32>,
    cols: Vec<Column>,
}

impl ColumnsBuilder {
    /// 栏间距(逻辑像素)。
    pub fn gap(&mut self, g: f32) -> &mut Self {
        self.gap = Some(g);
        self
    }
    /// 一栏(权重 1.0)。
    pub fn col<R>(&mut self, f: impl FnOnce(&mut Doc) -> R) -> &mut Self {
        self.col_weighted(1.0, f)
    }
    /// 一栏(指定宽度权重)。
    pub fn col_weighted<R>(&mut self, weight: f32, f: impl FnOnce(&mut Doc) -> R) -> &mut Self {
        let mut inner = Doc::new();
        let _ = f(&mut inner);
        self.cols.push(Column { blocks: inner.blocks, weight });
        self
    }
}

/// 进度条构建器([`Doc::progress`] 的闭包参数)。
pub struct ProgressBuilder {
    p: Progress,
}

impl ProgressBuilder {
    /// 条高(逻辑像素,默认 10)。
    pub fn height(&mut self, h: f32) -> &mut Self {
        self.p.height = h;
        self
    }
    /// 填充色(默认主题强调色)。
    pub fn fill(&mut self, hex: &str) -> &mut Self {
        self.p.fill = Color::hex(hex).or(self.p.fill);
        self
    }
    /// 底槽色(默认主题边框色)。
    pub fn track(&mut self, hex: &str) -> &mut Self {
        self.p.track = Color::hex(hex).or(self.p.track);
        self
    }
    /// 圆角半径(逻辑像素,默认半高即胶囊形;0 = 直角)。
    pub fn radius(&mut self, r: f32) -> &mut Self {
        self.p.radius = Some(r);
        self
    }
    /// 条宽(绝对逻辑像素;默认铺满内容宽)。
    pub fn width_px(&mut self, px: f32) -> &mut Self {
        self.p.width = Some(Length::Px(px));
        self
    }
    /// 条宽(内容宽的百分比)。
    pub fn width_percent(&mut self, pct: f32) -> &mut Self {
        self.p.width = Some(Length::Percent(pct));
        self
    }
    /// 水平对齐(窄于内容宽时生效)。
    pub fn align(&mut self, a: Align) -> &mut Self {
        self.p.align = a;
        self
    }
}

/// 列表构建器。
pub struct ListBuilder {
    kind: ListKind,
    start: u32,
    items: Vec<ListItem>,
}

impl ListBuilder {
    /// 有序列表起始序号。
    pub fn start(&mut self, n: u32) -> &mut Self {
        self.start = n;
        self
    }
    /// 一个列表项(内容是块容器,可放多段 / 嵌套子列表)。
    pub fn item<R>(&mut self, f: impl FnOnce(&mut Doc) -> R) -> &mut Self {
        let mut inner = Doc::new();
        let _ = f(&mut inner);
        self.items.push(ListItem { blocks: inner.blocks, check: None });
        self
    }
    /// 一个任务项(`done` = 已完成):标记渲染成 `✓` / `□`,对应标记文本的 `- [x]` / `- [ ]`。
    pub fn task<R>(&mut self, done: bool, f: impl FnOnce(&mut Doc) -> R) -> &mut Self {
        let mut inner = Doc::new();
        let _ = f(&mut inner);
        self.items.push(ListItem { blocks: inner.blocks, check: Some(done) });
        self
    }
}

/// 块级图片构建器。
pub struct ImageBuilder {
    width: Option<Length>,
    align: Align,
    caption: Option<Vec<Inline>>,
    decor: ImageDecor,
}

impl ImageBuilder {
    /// 绝对宽度(逻辑像素)。
    pub fn width_px(&mut self, px: f32) -> &mut Self {
        self.width = Some(Length::Px(px));
        self
    }
    /// 相对内容宽的百分比宽度。
    pub fn width_percent(&mut self, pct: f32) -> &mut Self {
        self.width = Some(Length::Percent(pct));
        self
    }
    /// 对齐。
    pub fn align(&mut self, a: Align) -> &mut Self {
        self.align = a;
        self
    }
    /// 纯文字图注。
    pub fn caption(&mut self, s: impl Into<String>) -> &mut Self {
        self.caption = Some(vec![Inline::Text { text: s.into(), style: TextStyle::default() }]);
        self
    }
    /// 富文字图注(闭包配置行内)。
    pub fn caption_with<R>(&mut self, f: impl FnOnce(&mut ParaBuilder) -> R) -> &mut Self {
        let mut pb = ParaBuilder::new();
        let _ = f(&mut pb);
        self.caption = Some(pb.inlines);
        self
    }

    // ── 装饰层:角标 / 边框 / 水印 / 圆角 / 阴影(画在图面上,不改布局尺寸) ──

    /// 角标(默认右上角、黑底白字),闭包微调:`im.badge("动图", |b| b.anchor(..).bg(..))`。
    pub fn badge<R>(
        &mut self,
        text: impl Into<String>,
        f: impl FnOnce(&mut BadgeBuilder) -> R,
    ) -> &mut Self {
        let mut bb = BadgeBuilder { badge: Badge::new(text) };
        let _ = f(&mut bb);
        self.decor.badge = Some(bb.badge);
        self
    }
    /// 边框:线宽(逻辑像素)+ 十六进制色(非法色忽略整条);有圆角时随圆角描边。
    pub fn border(&mut self, width: f32, hex: &str) -> &mut Self {
        if width > 0.0 && width.is_finite() {
            if let Some(color) = Color::hex(hex) {
                self.decor.border = Some(ImageBorder { width, color });
            }
        }
        self
    }
    /// 水印(默认右下角、白 40%),闭包微调:`im.watermark("abot", |w| w.anchor(..))`。
    pub fn watermark<R>(
        &mut self,
        text: impl Into<String>,
        f: impl FnOnce(&mut WatermarkBuilder) -> R,
    ) -> &mut Self {
        let mut wb = WatermarkBuilder { wm: Watermark::new(text) };
        let _ = f(&mut wb);
        self.decor.watermark = Some(wb.wm);
        self
    }
    /// 圆角半径(逻辑像素):裁切图面四角。
    pub fn rounded(&mut self, radius: f32) -> &mut Self {
        if radius.is_finite() && radius > 0.0 {
            self.decor.radius = radius;
        }
        self
    }
    /// 投影(默认形态:下坠 2 逻辑像素、软化 6、黑 25%)。
    pub fn shadow(&mut self) -> &mut Self {
        self.decor.shadow = Some(Shadow::default());
        self
    }
    /// 投影(自定形态:偏移/软化为逻辑像素,色为十六进制,非法色忽略整条)。
    pub fn shadow_with(&mut self, dx: f32, dy: f32, blur: f32, hex: &str) -> &mut Self {
        if let Some(color) = Color::hex(hex) {
            self.decor.shadow = Some(Shadow { dx, dy, blur: blur.max(0.0), color });
        }
        self
    }
}

/// 角标微调构建器。
pub struct BadgeBuilder {
    badge: Badge,
}

impl BadgeBuilder {
    /// 停靠角。
    pub fn anchor(&mut self, a: Anchor) -> &mut Self {
        self.badge.anchor = a;
        self
    }
    /// 底板色(十六进制,可含 alpha;非法忽略)。
    pub fn bg(&mut self, hex: &str) -> &mut Self {
        if let Some(c) = Color::hex(hex) {
            self.badge.bg = c;
        }
        self
    }
    /// 文字色(十六进制;非法忽略)。
    pub fn fg(&mut self, hex: &str) -> &mut Self {
        if let Some(c) = Color::hex(hex) {
            self.badge.fg = c;
        }
        self
    }
    /// 字号倍率(相对基准;非法忽略)。
    pub fn size(&mut self, mult: f32) -> &mut Self {
        if mult.is_finite() && mult > 0.0 {
            self.badge.size = mult;
        }
        self
    }
}

/// 水印微调构建器。
pub struct WatermarkBuilder {
    wm: Watermark,
}

impl WatermarkBuilder {
    /// 停靠处(四角或正中)。
    pub fn anchor(&mut self, a: Anchor) -> &mut Self {
        self.wm.anchor = a;
        self
    }
    /// 颜色(十六进制,可含 alpha;非法忽略)。
    pub fn color(&mut self, hex: &str) -> &mut Self {
        if let Some(c) = Color::hex(hex) {
            self.wm.color = c;
        }
        self
    }
    /// 字号倍率(相对基准;非法忽略)。
    pub fn size(&mut self, mult: f32) -> &mut Self {
        if mult.is_finite() && mult > 0.0 {
            self.wm.size = mult;
        }
        self
    }
}

