//! 文档模型(IR)—— 两个前端(标记解析 [`markup`](crate::parse_markup) / 构建器
//! [`Doc`](crate::Doc))的共同产物,也是版式引擎的唯一输入。块级 + 行内 + 可叠加的文字样式,
//! 全是普通数据,不含渲染状态。一般不直接构造,用构建器或标记文本得到。

use std::path::PathBuf;

/// 一份文档:从上到下排布的块序列。
#[derive(Clone, Debug, Default)]
pub struct Document {
    /// 块序列。
    pub blocks: Vec<Block>,
}

/// 块级元素。
#[derive(Clone, Debug)]
pub enum Block {
    /// 标题(h1–h6)。
    Heading {
        /// 级别,取 1..=6。
        level: u8,
        /// 行内内容。
        inlines: Vec<Inline>,
        /// 水平对齐。
        align: Align,
    },
    /// 段落。
    Paragraph {
        /// 行内内容。
        inlines: Vec<Inline>,
        /// 水平对齐。
        align: Align,
    },
    /// 有序 / 无序列表(项内容是块序列,可嵌套、可多段)。
    List(List),
    /// 引用块(裹块,可嵌套)。
    Quote(Vec<Block>),
    /// 代码块(等宽、带底色;不做语法高亮)。
    Code {
        /// 语言标签;有则在代码盒右上角渲染成小标签,可缺。
        lang: Option<String>,
        /// 代码原文(保留换行)。
        text: String,
    },
    /// 分割线。
    Divider,
    /// 块级图片(可带宽度 / 对齐 / 图注)。
    Image(BlockImage),
    /// 多栏并排。
    Columns(Columns),
    /// 表格。
    Table(Table),
}

/// 表格。`cols` 给各列对齐与可选限宽(短于列数时,缺的列按默认:左对齐 + 自适应)。
#[derive(Clone, Debug)]
pub struct Table {
    /// 表头行;`None` = 无表头。
    pub header: Option<Vec<Cell>>,
    /// 数据行。
    pub rows: Vec<Vec<Cell>>,
    /// 各列规格(对齐 / 限宽)。
    pub cols: Vec<ColSpec>,
    /// 紧凑度与网格样式。
    pub style: TableStyle,
}

/// 表格的紧凑度与网格样式。
#[derive(Clone, Debug)]
pub struct TableStyle {
    /// 单元格左右内边距(逻辑像素);`None` = 默认。越小列越紧凑。
    pub pad_x: Option<f32>,
    /// 单元格上下内边距(逻辑像素);`None` = 默认。越小行越紧凑(行距越小)。
    pub pad_y: Option<f32>,
    /// 网格线开关。
    pub grid: TableGrid,
    /// 表头浅底,默认开。
    pub header_fill: bool,
}

impl Default for TableStyle {
    fn default() -> Self {
        Self { pad_x: None, pad_y: None, grid: TableGrid::default(), header_fill: true }
    }
}

/// 网格线开关,默认全开。
#[derive(Clone, Copy, Debug)]
pub struct TableGrid {
    /// 外框线。
    pub outer: bool,
    /// 列竖线。
    pub vertical: bool,
    /// 行横线。
    pub horizontal: bool,
}

impl Default for TableGrid {
    fn default() -> Self {
        Self { outer: true, vertical: true, horizontal: true }
    }
}

/// 列规格:对齐 + 可选限宽。
#[derive(Clone, Debug)]
pub struct ColSpec {
    /// 该列对齐。
    pub align: Align,
    /// 限宽;`None` = 按内容自适应。
    pub width: Option<Length>,
}

impl Default for ColSpec {
    fn default() -> Self {
        Self { align: Align::Left, width: None }
    }
}

/// 单元格:行内内容(按列宽自动换行)+ 可选背景填色。
#[derive(Clone, Debug)]
pub struct Cell {
    /// 单元格的行内内容。
    pub inlines: Vec<Inline>,
    /// 背景填色;`None` = 无(随表)。
    pub bg: Option<Color>,
}

/// 多栏容器:各栏并排,行高取最高栏。
#[derive(Clone, Debug)]
pub struct Columns {
    /// 各栏。
    pub cols: Vec<Column>,
    /// 栏间距(逻辑像素);`None` = 主题默认。
    pub gap: Option<f32>,
}

/// 一栏:块内容 + 宽度权重(按权重瓜分可用宽,默认 1.0)。
#[derive(Clone, Debug)]
pub struct Column {
    /// 栏内容。
    pub blocks: Vec<Block>,
    /// 宽度权重。
    pub weight: f32,
}

/// 列表。
#[derive(Clone, Debug)]
pub struct List {
    /// 有序 / 无序。
    pub kind: ListKind,
    /// 有序列表的起始序号(无序忽略)。
    pub start: u32,
    /// 列表项。
    pub items: Vec<ListItem>,
}

/// 列表种类。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListKind {
    /// 无序(项目符号)。
    Unordered,
    /// 有序(序号)。
    Ordered,
}

/// 列表项:内容是块序列,故支持多段与嵌套子列表。
#[derive(Clone, Debug)]
pub struct ListItem {
    /// 项内容。
    pub blocks: Vec<Block>,
    /// 任务复选标记:`None` = 普通项;`Some(已完成)` = 渲染成复选标记(`□` / `✓`),
    /// 对应标记文本的 `- [ ]` / `- [x]`。
    pub check: Option<bool>,
}

/// 块级图片。
#[derive(Clone, Debug)]
pub struct BlockImage {
    /// 图片来源。
    pub src: ImageSource,
    /// 显示宽度;`None` = 适配内容宽(不超出)。
    pub width: Option<Length>,
    /// 水平对齐。
    pub align: Align,
    /// 图注(排在图下方,居中小字);`None` = 无。
    pub caption: Option<Vec<Inline>>,
}

/// 行内元素。
#[derive(Clone, Debug)]
pub enum Inline {
    /// 一段带样式的文字。
    Text {
        /// 文字。
        text: String,
        /// 样式。
        style: TextStyle,
    },
    /// 行内代码(等宽 + 浅底)。
    Code(String),
    /// 硬换行。
    LineBreak,
}

/// 可叠加的文字样式。span 嵌套时逐字段合并。
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    /// 字重(CSS 习惯值:细 300 / 常规 400 / 粗 700,内置字体 100–900 都有真实例)。
    /// `None` = 跟随语境:正文常规,标题 / 表头加粗。
    pub weight: Option<u16>,
    /// 斜体。
    pub italic: bool,
    /// 下划线。
    pub underline: bool,
    /// 删除线。
    pub strike: bool,
    /// 文字色;`None` = 用主题文字色。
    pub color: Option<Color>,
    /// 高亮底色;`None` = 无高亮。
    pub highlight: Option<Highlight>,
    /// 相对基准字号的倍率(默认 1.0)。
    pub size: f32,
    /// 字族角色。
    pub font: FontRole,
    /// 链接文字(标记文本 `[文字](URL)` 的产物):无显式 `color` 时按主题强调色渲染。
    /// 图片不可点,URL 本身不展示。
    pub link: bool,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            weight: None,
            italic: false,
            underline: false,
            strike: false,
            color: None,
            highlight: None,
            size: 1.0,
            font: FontRole::Sans,
            link: false,
        }
    }
}

/// 高亮底色来源。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Highlight {
    /// 跟随主题默认高亮色(随亮 / 暗变)。
    Theme,
    /// 指定具体色。
    Custom(Color),
}

/// 字族角色。`Named` 直接按字族名匹配,匹配不到回退 Sans。
#[derive(Clone, Debug, PartialEq)]
pub enum FontRole {
    /// 无衬线(默认正文)。
    Sans,
    /// 衬线。
    Serif,
    /// 等宽。
    Mono,
    /// 楷体。
    Kai,
    /// 指定字族名。
    Named(String),
}

/// 水平对齐。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Align {
    /// 左对齐(默认)。
    #[default]
    Left,
    /// 居中。
    Center,
    /// 右对齐。
    Right,
    /// 两端对齐。
    Justify,
}

/// RGBA 颜色(每通道 8 位,非预乘)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    /// 红。
    pub r: u8,
    /// 绿。
    pub g: u8,
    /// 蓝。
    pub b: u8,
    /// 不透明度(255 = 不透明)。
    pub a: u8,
}

impl Color {
    /// 不透明色。
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// 带 alpha 的色。
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// 解析十六进制色:`#rgb` / `#rrggbb` / `#rrggbbaa`(井号可省,大小写不限)。
    /// 非法返回 `None`。`#rgb` 每位扩成两位(`f` → `ff`)。
    pub fn hex(s: &str) -> Option<Self> {
        let h = s.strip_prefix('#').unwrap_or(s);
        if !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let n = |slice: &str| u8::from_str_radix(slice, 16).ok();
        match h.len() {
            3 => {
                let b = h.as_bytes();
                let dup = |c: u8| {
                    let d = (c as char).to_digit(16)? as u8;
                    Some(d << 4 | d)
                };
                Some(Self::rgb(dup(b[0])?, dup(b[1])?, dup(b[2])?))
            }
            6 => Some(Self::rgb(n(&h[0..2])?, n(&h[2..4])?, n(&h[4..6])?)),
            8 => Some(Self::rgba(n(&h[0..2])?, n(&h[2..4])?, n(&h[4..6])?, n(&h[6..8])?)),
            _ => None,
        }
    }
}

/// 长度:绝对像素,或相对内容宽的百分比。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Length {
    /// 绝对逻辑像素。
    Px(f32),
    /// 内容宽的百分比(0–100)。
    Percent(f32),
}

/// 图片来源。引擎不联网:URL 由调用方下好,以 `Bytes` 传入。
#[derive(Clone, Debug)]
pub enum ImageSource {
    /// 已加载的图片字节。
    Bytes(Vec<u8>),
    /// 磁盘路径,渲染时读取。
    Path(PathBuf),
    /// 具名引用(标记文本里的 `@名字`),渲染时从 [`RenderOptions::images`](crate::RenderOptions::images) 取字节。
    Named(String),
}
