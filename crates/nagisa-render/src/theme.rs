//! 主题与渲染配置。`RenderOptions` 是 `render_*` 的入参;`Theme` 是配色 / 字族 / 字号等
//! 视觉口径,带亮 / 暗预设。所有尺寸是**逻辑值**,layout 前统一乘 `scale` 换设备像素。

use std::collections::HashMap;

use crate::build::ParaBuilder;
use crate::font::FontHandle;
use crate::model::{Align, Color, Inline, TextStyle};

/// 四边内边距(逻辑像素)。
#[derive(Clone, Copy, Debug)]
pub struct Insets {
    /// 上。
    pub top: f32,
    /// 右。
    pub right: f32,
    /// 下。
    pub bottom: f32,
    /// 左。
    pub left: f32,
}

impl Insets {
    /// 四边相等。
    pub const fn all(v: f32) -> Self {
        Self { top: v, right: v, bottom: v, left: v }
    }
    /// `v` = 上下,`h` = 左右。
    pub const fn symmetric(v: f32, h: f32) -> Self {
        Self { top: v, right: h, bottom: v, left: h }
    }
}

/// 视觉主题:配色 + 字族 + 字号体系。预设见 [`Theme::light`] / [`Theme::dark`]。
#[derive(Clone, Debug)]
pub struct Theme {
    /// 画布背景色。
    pub background: Color,
    /// 正文文字色。
    pub text: Color,
    /// 引用条 / 序号 / 链接等强调色。
    pub accent: Color,
    /// 图注 / 次要文字。
    pub muted: Color,
    /// 代码块 / 行内代码底色。
    pub code_bg: Color,
    /// 代码文字色。
    pub code_text: Color,
    /// `==高亮==` 的默认底色。
    pub highlight: Color,
    /// 表格 / 网格的边框线色(比 `muted` 更淡)。
    pub border: Color,
    /// 无衬线字族名。
    pub font_sans: String,
    /// 衬线字族名。
    pub font_serif: String,
    /// 等宽字族名。
    pub font_mono: String,
    /// 楷体字族名。
    pub font_kai: String,
    /// 彩色 emoji 字族名(emoji 表现序列统一切到它,黑体的单色字面不抢跑;
    /// 不随包内置,默认指系统 / 自备的 Noto Color Emoji,缺则回退)。
    pub font_emoji: String,
    /// 基准字号(逻辑像素)。
    pub base_size: f32,
    /// 行高倍率。
    pub line_height: f32,
    /// h1..h6 相对基准字号的倍率。
    pub heading_scale: [f32; 6],
}

impl Theme {
    /// 亮色预设。
    pub fn light() -> Self {
        Self {
            background: Color::rgb(0xff, 0xff, 0xff),
            text: Color::rgb(0x1f, 0x23, 0x28),
            accent: Color::rgb(0x25, 0x63, 0xeb),
            muted: Color::rgb(0x6e, 0x77, 0x81),
            code_bg: Color::rgb(0xf3, 0xf4, 0xf6),
            code_text: Color::rgb(0x1f, 0x23, 0x28),
            highlight: Color::rgb(0xff, 0xf1, 0xa8),
            border: Color::rgb(0xe5, 0xe7, 0xeb),
            ..Self::common()
        }
    }

    /// 暗色预设。
    pub fn dark() -> Self {
        Self {
            background: Color::rgb(0x0d, 0x11, 0x17),
            text: Color::rgb(0xe6, 0xed, 0xf3),
            accent: Color::rgb(0x58, 0xa6, 0xff),
            muted: Color::rgb(0x8b, 0x94, 0x9e),
            code_bg: Color::rgb(0x16, 0x1b, 0x22),
            code_text: Color::rgb(0xe6, 0xed, 0xf3),
            highlight: Color::rgb(0x57, 0x4a, 0x1a),
            border: Color::rgb(0x30, 0x36, 0x3d),
            ..Self::common()
        }
    }

    /// 亮 / 暗共享的非配色部分(字族 / 字号 / 行高 / 标题阶梯)。黑体 / 等宽随包内置;
    /// 衬线 / 楷体不内置,字族名是给使用方注入字体对的口径,缺则回退黑体。
    fn common() -> Self {
        Self {
            background: Color::rgb(0, 0, 0),
            text: Color::rgb(0, 0, 0),
            accent: Color::rgb(0, 0, 0),
            muted: Color::rgb(0, 0, 0),
            code_bg: Color::rgb(0, 0, 0),
            code_text: Color::rgb(0, 0, 0),
            highlight: Color::rgb(0, 0, 0),
            border: Color::rgb(0, 0, 0),
            font_sans: "Noto Sans SC".to_string(), // 内置
            font_serif: "Noto Serif SC".to_string(), // 不内置:使用方注入思源宋体即生效,缺则回退黑体
            font_mono: "JetBrains Mono".to_string(), // 内置(CJK 在等宽语境回退 Noto)
            font_kai: "LXGW WenKai GB".to_string(),  // 不内置:使用方注入霞鹜文楷即生效,缺则回退黑体
            font_emoji: "Noto Color Emoji".to_string(), // 不内置:系统或自备,缺则回退
            base_size: 30.0,
            line_height: 1.5,
            heading_scale: [2.0, 1.6, 1.35, 1.15, 1.0, 0.9],
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::light()
    }
}

/// 输出图片格式。文字图首选 `Webp`(最小 + 快);`Png` 通用兜底;`PngFast` 要 PNG 又要快。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputFormat {
    /// PNG(无损,平衡压缩,默认——通用兼容)。
    Png,
    /// PNG(无损,快压缩:约 8 倍快、体积大 ~40%)。必须出 PNG 又要快时用。
    PngFast,
    /// WebP(无损;通常体积最小、速度也好)。文字图首选;画布单边 > 16383px(WebP 上限)时编码报错。
    Webp,
    /// WebP 优先,画布单边 > 16383px 时自动落 PNG。要 WebP 的体积、又不想为超长图单独处理报错时用
    /// ——超限会**改格式**,显式选了才发生。
    WebpOrPng,
}

/// 渲染入参。链式覆写;`default()` = 960 逻辑宽、亮色、scale 2、PNG、默认字体句柄。
#[derive(Clone)]
pub struct RenderOptions {
    /// 逻辑内容宽(含左右内边距),默认 960。
    pub width: f32,
    /// 页边距(逻辑像素)。
    pub padding: Insets,
    /// 超采样系数(输出 = 逻辑尺寸 × scale),默认 2.0。越大越清晰也越慢 / 越大。
    pub scale: f32,
    /// 视觉主题。
    pub theme: Theme,
    /// 字体栈句柄。
    pub fonts: FontHandle,
    /// 输出格式,默认 PNG。
    pub format: OutputFormat,
    /// 标记文本里 `@名字` 图片 → 字节。
    pub images: HashMap<String, Vec<u8>>,
    /// 页眉条(可选):一行小字排在内容上方,与文档无关的固定标识(品牌 / 出处)。
    pub header: Option<PageChrome>,
    /// 页脚条(可选):一行小字排在内容下方,常放项目水印(如「abot · github.com/…」)。
    pub footer: Option<PageChrome>,
}

/// 页眉 / 页脚条:一行小字(可富文本)+ 可选的与内容之间的细分割线。参与布局高度
/// ([`measure_document`](crate::measure_document) 自然包含),不归文档内容管——同一品牌
/// 标识配在 `RenderOptions` 上,所有出图统一带。
#[derive(Clone, Debug)]
pub struct PageChrome {
    /// 行内内容(纯文字经 [`new`](Self::new),富文本经 [`rich`](Self::rich))。
    pub inlines: Vec<Inline>,
    /// 行尾内容(可选):与 `inlines` 同一行,右对齐——「左 logo 右署名」的分栏形态。
    /// 设了它,`align` 只管 `inlines`(通常配左对齐)。
    pub trailing: Option<Vec<Inline>>,
    /// 水平对齐(`with_header` 默认左、`with_footer` 默认居中)。
    pub align: Align,
    /// 缺省文字色:未显式上色的 span 用它;`None` = 主题次要色(muted)。
    pub color: Option<Color>,
    /// 相对基准字号的倍率(默认 0.72)。
    pub size: f32,
    /// 与内容之间画一条细线(默认开;设了 `band` 自动不画)。
    pub rule: bool,
    /// 满幅色带(可选,仅页脚生效):整条画布宽的底色带贴住画布底,文字坐在带内——
    /// 分享卡式的「底栏」。设色深时记得给 span 配亮色文字。
    pub band: Option<Color>,
}

impl PageChrome {
    /// 默认形态:次要色小字、带细线、左对齐。
    pub fn new(text: impl Into<String>) -> Self {
        Self::from_inlines(vec![Inline::Text { text: text.into(), style: TextStyle::default() }])
    }

    /// 富文本形态:闭包拼行内(粗细 / 色 / 字号倍率皆可,如品牌名加重、连接词浅色)。
    /// `p.styled("abot", |s| { s.weight(600); })` 这类未显式上色的 span 仍按缺省色染。
    pub fn rich<R>(f: impl FnOnce(&mut ParaBuilder) -> R) -> Self {
        let mut pb = ParaBuilder::new();
        let _ = f(&mut pb);
        Self::from_inlines(pb.into_inlines())
    }

    fn from_inlines(inlines: Vec<Inline>) -> Self {
        Self {
            inlines,
            trailing: None,
            align: Align::Left,
            color: None,
            size: 0.72,
            rule: true,
            band: None,
        }
    }

    /// 设行尾内容(右对齐,与主内容同一行):「左 logo 右署名」分栏。
    pub fn trailing<R>(mut self, f: impl FnOnce(&mut ParaBuilder) -> R) -> Self {
        let mut pb = ParaBuilder::new();
        let _ = f(&mut pb);
        self.trailing = Some(pb.into_inlines());
        self
    }

    /// 设满幅色带(十六进制;非法忽略)。仅页脚生效,自动不再画细线。
    pub fn band(mut self, hex: &str) -> Self {
        if let Some(c) = Color::hex(hex) {
            self.band = Some(c);
        }
        self
    }
    /// 设对齐。
    pub fn align(mut self, a: Align) -> Self {
        self.align = a;
        self
    }
    /// 设文字色(十六进制;非法忽略)。
    pub fn color(mut self, hex: &str) -> Self {
        if let Some(c) = Color::hex(hex) {
            self.color = Some(c);
        }
        self
    }
    /// 设字号倍率(非法忽略)。
    pub fn size(mut self, mult: f32) -> Self {
        if mult.is_finite() && mult > 0.0 {
            self.size = mult;
        }
        self
    }
    /// 不画细线。
    pub fn no_rule(mut self) -> Self {
        self.rule = false;
        self
    }
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            width: 960.0,
            padding: Insets::symmetric(32.0, 40.0),
            scale: 2.0,
            theme: Theme::light(),
            fonts: FontHandle::shared_default(),
            format: OutputFormat::Png,
            images: HashMap::new(),
            header: None,
            footer: None,
        }
    }
}

impl RenderOptions {
    /// 设逻辑内容宽。
    pub fn with_width(mut self, w: f32) -> Self {
        self.width = w;
        self
    }
    /// 设页边距(逻辑像素)。
    pub fn with_padding(mut self, p: Insets) -> Self {
        self.padding = p;
        self
    }
    /// 设主题。
    pub fn with_theme(mut self, t: Theme) -> Self {
        self.theme = t;
        self
    }
    /// 设字体句柄。
    pub fn with_fonts(mut self, f: FontHandle) -> Self {
        self.fonts = f;
        self
    }
    /// 设超采样系数(清晰度档位,见 `fast`/`sharp`/`ultra` 预设)。
    pub fn with_scale(mut self, s: f32) -> Self {
        self.scale = s.clamp(0.25, 8.0);
        self
    }
    /// 清晰度预设:快(scale 1)——最省、体积小,清晰度一般。
    pub fn fast(self) -> Self {
        self.with_scale(1.0)
    }
    /// 清晰度预设:标准(scale 1.5)。
    pub fn standard(self) -> Self {
        self.with_scale(1.5)
    }
    /// 清晰度预设:清晰(scale 2,默认)。
    pub fn sharp(self) -> Self {
        self.with_scale(2.0)
    }
    /// 清晰度预设:超清(scale 3)——最清晰也最慢 / 最大。
    pub fn ultra(self) -> Self {
        self.with_scale(3.0)
    }
    /// 设页眉(默认形态:左对齐次要色小字 + 细线);要微调用 [`with_header_chrome`](Self::with_header_chrome)。
    pub fn with_header(self, text: impl Into<String>) -> Self {
        self.with_header_chrome(PageChrome::new(text))
    }
    /// 设页眉(完整形态)。
    pub fn with_header_chrome(mut self, c: PageChrome) -> Self {
        self.header = Some(c);
        self
    }
    /// 设页脚(默认形态:居中次要色小字 + 细线,适合项目水印);微调用 [`with_footer_chrome`](Self::with_footer_chrome)。
    pub fn with_footer(self, text: impl Into<String>) -> Self {
        self.with_footer_chrome(PageChrome::new(text).align(Align::Center))
    }
    /// 设页脚(完整形态)。
    pub fn with_footer_chrome(mut self, c: PageChrome) -> Self {
        self.footer = Some(c);
        self
    }
    /// 设输出格式。
    pub fn with_format(mut self, f: OutputFormat) -> Self {
        self.format = f;
        self
    }
    /// 输出 PNG(无损,平衡压缩)。
    pub fn png(self) -> Self {
        self.with_format(OutputFormat::Png)
    }
    /// 输出 PNG(无损,快压缩——更快但更大)。
    pub fn png_fast(self) -> Self {
        self.with_format(OutputFormat::PngFast)
    }
    /// 输出 WebP(无损,文字图首选)。画布单边 > 16383px 时编码报错。
    pub fn webp(self) -> Self {
        self.with_format(OutputFormat::Webp)
    }
    /// 输出 WebP,但画布单边 > 16383px(WebP 上限)时自动落 PNG。
    pub fn webp_or_png(self) -> Self {
        self.with_format(OutputFormat::WebpOrPng)
    }
}
