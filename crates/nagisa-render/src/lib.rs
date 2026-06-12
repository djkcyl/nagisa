//! 把一段带格式的文本 / 一份用构建器拼出的文档,排版渲染成图片字节。
//!
//! 给的是图片字节,不碰任何协议——送到 QQ 由调用方包一层(如 `Segment::image_bytes`)。
//! 管线:源(标记文本 | 构建器)→ 文档模型 [`Document`] → 版式(parley 整形 / 断行 /
//! 字体回退 / CJK+拉丁+emoji 混排)→ 光栅(tiny-skia + swash)→ 图片字节。
//!
//! # 两种写法
//!
//! 标记文本(类 Markdown,适合「一大段文字」):
//!
//! ```ignore
//! use nagisa_render::{render_markup, RenderOptions};
//!
//! let png = render_markup("# 标题\n\n正文 **加粗**、[彩色]{color=#e00}、==高亮==。", &RenderOptions::default())?;
//! ```
//!
//! Rust 构建器(类型安全,适合从数据生成卡片):
//!
//! ```ignore
//! use nagisa_render::{render_document, Doc, RenderOptions};
//!
//! let doc = Doc::new()
//!     .heading(1, |h| h.text("月度报告"))
//!     .paragraph(|p| { p.text("环比 ").bold("+12%").text("。"); })
//!     .table(|t| { t.head(["项", "值"]).row(["发言", "3450"]); })
//!     .build();
//! let png = render_document(&doc, &RenderOptions::default())?;
//! ```
//!
//! 两种写法产出同一个 [`Document`];也可以 [`parse_markup`] 拿到 `Document` 再用构建器接着改。
//!
//! # 能排什么
//!
//! 标题、段落、粗 / 细 / 任意字重、斜 / 下划 / 删除、颜色 / 高亮、字号 / 字族(黑 / 宋 /
//! 楷 / 等宽)、文字阴影、链接(图片点不了,取文字按强调色渲染)、行内与块级代码(块级带题头栏:`</>` 符 + 语言标签,
//! 块级带轻量语法上色:rust / json / toml / python / js / shell / c 系,四类词色随主题
//! [`CodePalette`],认不出的语言整块默认色)、有序 / 无序列表(可嵌套)、任务列表(`- [ ]` / `- [x]` → `□` / `✓`)、引用、
//! 分割线、图片(缩放 / 对齐 / 图注 + 装饰层:角标 / 边框 / 水印 / 圆角裁切 / 投影,见
//! [`ImageBuilder`])、左中右两端对齐、多栏并排([`Columns`])、面板([`Panel`]:底色 / 边框 /
//! 圆角 / 内边距 / 投影的卡片容器,作并排栏整栏时自动拉齐行高;`::: panel {bg=…}` /
//! [`Doc::panel`])、表格([`Table`]:自适应列宽 / 限宽 / 铺满可用宽(`expand`)/ 按列行格
//! 上色 / 紧凑度与网格可调)。标记语言对应是 Markdown 基底加少量扩展(`==高亮==`、
//! `[文字]{属性}`、`::: 围栏`、GFM 表格),见 [`parse_markup`];构建器见 [`Doc`]。
//!
//! # 输出与配置
//!
//! 都在 [`RenderOptions`]:
//!
//! - **格式** [`OutputFormat`]:`Png`(默认,通用)/ `PngFast`(更快、略大)/ `Webp`(无损,
//!   文字图体积最小且快;单边 > 16383px 报错)/ `WebpOrPng`(WebP 优先,超 WebP 上限自动落 PNG)。
//!   文字图建议 `.webp()`,长图怕超限用 `.webp_or_png()`。
//! - **清晰度** `scale`:`.fast()` / `.standard()` / `.sharp()`(默认 2×)/ `.ultra()`,或
//!   `.with_scale(f)`。越大越清晰、也越慢越大。
//! - **主题** [`Theme`]:亮 / 暗预设 + 自定义配色 / 字族 / 字号。
//! - **字体** [`FontHandle`]:内置兜底(黑体 + 等宽正斜,细 / 常规 / 粗皆真字形,zstd 压缩
//!   内嵌、首次使用时解压)+ 自备数据(`data()`,裸字节或 zstd 压缩皆可)+ 自定义目录 +
//!   系统,四来源合并。衬线 / 楷体角色不随包内置(默认字族名 Noto Serif SC / LXGW WenKai GB,
//!   自备对应字体即生效),缺字体时回退黑体。
//! - **页眉 / 页脚** [`PageChrome`]:与文档无关的固定标识(品牌 / 署名 / 出处),配在选项上
//!   所有出图统一带;富文本、左右分栏(`trailing`)、满幅色带(`band`)皆可。
//!
//! 入口:[`render_markup`] / [`render_document`](→ 图片字节)、[`render_to_rgba`](→ RGBA 图,
//! 供进一步合成)、[`measure_document`](只排版量尺寸不绘制,按高度上限把长内容切成多张图
//! 时先量再渲)。
#![forbid(unsafe_code)]

mod build;
mod error;
mod font;
mod highlight;
mod layout;
mod markup;
mod model;
mod paint;
mod theme;

pub use build::{
    BadgeBuilder, ColumnsBuilder, Doc, ImageBuilder, ListBuilder, PanelBuilder, ParaBuilder, ProgressBuilder,
    StyleBuilder, TableBuilder, WatermarkBuilder,
};
pub use error::{Error, Result};
pub use font::FontHandle;
pub use markup::parse as parse_markup;
pub use model::ImageSource;
pub use model::{
    Align, Anchor, Badge, Block, BlockImage, Cell, ColSpec, Color, Column, Columns, Document, DotMark, FontRole,
    Highlight, ImageBorder, ImageDecor, Inline, Length, List, ListItem, ListKind, Panel, PanelDecor, Progress,
    RingMark, Shadow, Table, TableGrid, TableStyle, TextStyle, Watermark,
};
pub use theme::{CodePalette, Insets, OutputFormat, PageChrome, RenderOptions, Theme};

/// 解析标记文本并渲染成图片字节(格式由 [`RenderOptions::format`] 决定,默认 PNG)。
pub fn render_markup(src: &str, opts: &RenderOptions) -> Result<Vec<u8>> {
    render_document(&markup::parse(src)?, opts)
}

/// 把一份文档排版渲染成图片字节(格式见 [`RenderOptions::format`])。
pub fn render_document(doc: &Document, opts: &RenderOptions) -> Result<Vec<u8>> {
    let layout = layout::layout_document(doc, opts)?;
    paint::paint(&layout, opts)
}

/// 渲染成(去预乘的)RGBA 图,供进一步合成,不编码。
pub fn render_to_rgba(doc: &Document, opts: &RenderOptions) -> Result<image::RgbaImage> {
    let layout = layout::layout_document(doc, opts)?;
    paint::paint_rgba(&layout, opts)
}

/// 只排版、不绘制:返回这份文档渲出来的图片尺寸(**物理像素**,即已含 `scale`)。
/// 供调用方做内容装箱——如按高度上限把长列表切成多张图,定好切分再真正渲染。
pub fn measure_document(doc: &Document, opts: &RenderOptions) -> Result<(u32, u32)> {
    let layout = layout::layout_document(doc, opts)?;
    Ok((layout.width_px, layout.height_px))
}
