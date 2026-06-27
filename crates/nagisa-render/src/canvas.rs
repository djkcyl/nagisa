//! 自由画布 —— 在 `Document` 文档流之外,直接往一张 `tiny_skia` 画布上画形状(圆角矩形 / 渐变 /
//! 线 / 圆 / 弧 / 多边形 / 雷达图)并合成文字盒,供数据卡片这类「图形为主、文字为辅」的出图。
//!
//! 文档引擎([`render_document`](crate::render_document))擅长「一大段排版」;卡片(如游戏面板、
//! 战报、雷达图属性卡)需要精确摆放形状与短文本,文档流不趁手。本模块给一个像素级画布:
//!
//! - **形状**走 tiny-skia 抗锯齿填充 / 描边(与 [`crate::paint`] 同后端,观感一致)。
//! - **文字**复用整条版式管线([`Doc`] → layout → paint):把一段样式化段落渲成透明底小图再合成,
//!   于是 CJK 整形 / 字体回退 / 抗锯齿全部白拿,不另造字体轮子。
//!
//! 坐标与尺寸都是**逻辑像素**,内部统一乘 `scale` 换物理像素(与 [`RenderOptions::scale`] 一致)。
//!
//! ```ignore
//! use nagisa_render::{Canvas, Color, RenderOptions, Align};
//! let opts = RenderOptions::default();
//! let mut c = Canvas::new(520.0, 300.0, 2.0)?;
//! c.rect(0.0, 0.0, 520.0, 300.0, 24.0, Color::rgb(0x16, 0x1b, 0x22)); // 卡底
//! c.radar(120.0, 150.0, 80.0, &[0.8, 0.6, 0.9, 0.4, 0.7], &Default::default());
//! c.text(220.0, 24.0, 280.0, &opts, |p| { p.styled("疾风", |s| { s.weight(700).size(1.4); }); })?;
//! let png = c.encode(nagisa_render::OutputFormat::Png)?;
//! ```

// 形状绘制 API 以坐标 / 尺寸 / 颜色为参,天然多参;不强拆成 struct(调用处更啰嗦)。
#![allow(clippy::too_many_arguments)]

use image::RgbaImage;
use tiny_skia::{
    FillRule, GradientStop, LinearGradient, Paint, PathBuilder, Pixmap, PixmapPaint, Point, PremultipliedColorU8, Rect,
    Shader, SpreadMode, Stroke, Transform,
};

use crate::build::Doc;
use crate::error::{Error, Result};
use crate::model::Color;
use crate::theme::{Insets, OutputFormat, RenderOptions};

/// 雷达图样式。`values` 为各轴 0..1 归一值;轴数 = 顶点数。
#[derive(Clone, Debug)]
pub struct Radar {
    /// 数据多边形填充色(通常带透明度)。
    pub fill: Color,
    /// 数据多边形描边色。
    pub stroke: Color,
    /// 数据多边形描边宽(逻辑像素)。
    pub stroke_w: f32,
    /// 网格(同心多边形 + 轴辐)色。
    pub grid: Color,
    /// 网格线宽(逻辑像素)。
    pub grid_w: f32,
    /// 同心网格圈数(≥1)。
    pub rings: u32,
    /// 顶点小圆点:`(半径, 色)`;`None` = 不画。
    pub vertex_dot: Option<(f32, Color)>,
    /// 第一个轴的角度(度,0 = 右、-90 = 正上;默认正上)。
    pub start_deg: f32,
}

impl Default for Radar {
    fn default() -> Self {
        Self {
            fill: Color::rgba(0x4c, 0x63, 0xb6, 0x66),
            stroke: Color::rgb(0x4c, 0x63, 0xb6),
            stroke_w: 2.0,
            grid: Color::rgba(0x8b, 0x94, 0x9e, 0x55),
            grid_w: 1.0,
            rings: 4,
            vertex_dot: Some((3.0, Color::rgb(0x4c, 0x63, 0xb6))),
            start_deg: -90.0,
        }
    }
}

/// 像素级自由画布(内部 `tiny_skia::Pixmap`,逻辑坐标 × `scale`)。
pub struct Canvas {
    pix: Pixmap,
    scale: f32,
}

impl Canvas {
    /// 新建透明底画布:逻辑 `w`×`h`,物理尺寸 = 逻辑 × `scale`。
    pub fn new(w: f32, h: f32, scale: f32) -> Result<Canvas> {
        let scale = if scale.is_finite() { scale.clamp(0.25, 8.0) } else { 2.0 };
        let pw = (w * scale).round().max(1.0) as u32;
        let ph = (h * scale).round().max(1.0) as u32;
        let pix = Pixmap::new(pw, ph).ok_or_else(|| Error::Layout("画布尺寸非法(过大或为 0)".into()))?;
        Ok(Canvas { pix, scale })
    }

    /// 超采样系数。
    pub fn scale(&self) -> f32 {
        self.scale
    }
    /// 物理宽(像素)。
    pub fn width_px(&self) -> u32 {
        self.pix.width()
    }
    /// 物理高(像素)。
    pub fn height_px(&self) -> u32 {
        self.pix.height()
    }

    /// 逻辑值 → 物理值。
    fn s(&self, v: f32) -> f32 {
        v * self.scale
    }

    /// 整张填充某色(铺底)。
    pub fn fill(&mut self, color: Color) {
        self.pix.fill(skia(color));
    }

    /// 实心(可圆角)矩形。
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, color: Color) {
        if w <= 0.0 || h <= 0.0 || color.a == 0 {
            return;
        }
        let mut paint = Paint::default();
        paint.set_color_rgba8(color.r, color.g, color.b, color.a);
        paint.anti_alias = true;
        self.fill_rrect(x, y, w, h, radius, &paint);
    }

    /// 描边(可圆角)矩形。线宽沿路径居中。
    pub fn stroke_rect(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, line_w: f32, color: Color) {
        if w <= 0.0 || h <= 0.0 || line_w <= 0.0 || color.a == 0 {
            return;
        }
        let Some(path) = self.rrect_path(x, y, w, h, radius) else { return };
        let mut paint = Paint::default();
        paint.set_color_rgba8(color.r, color.g, color.b, color.a);
        paint.anti_alias = true;
        let stroke = Stroke { width: self.s(line_w), ..Stroke::default() };
        self.pix.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }

    /// 竖直线性渐变的(可圆角)矩形:顶 `top` → 底 `bottom`。
    pub fn v_gradient(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, top: Color, bottom: Color) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let (px, py, pw, ph) = (self.s(x), self.s(y), self.s(w), self.s(h));
        let shader = LinearGradient::new(
            Point::from_xy(px, py),
            Point::from_xy(px, py + ph),
            vec![GradientStop::new(0.0, skia(top)), GradientStop::new(1.0, skia(bottom))],
            SpreadMode::Pad,
            Transform::identity(),
        );
        let paint = Paint {
            shader: shader.unwrap_or_else(|| Shader::SolidColor(skia(top))),
            anti_alias: true,
            ..Default::default()
        };
        // 路径用物理坐标手搓(已乘 scale),故走 raw 版本。
        if let Some(path) = rrect_path_px(px, py, pw, ph, self.s(radius)) {
            self.pix.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        }
    }

    /// 直线段(圆头)。
    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, line_w: f32, color: Color) {
        if line_w <= 0.0 || color.a == 0 {
            return;
        }
        let mut pb = PathBuilder::new();
        pb.move_to(self.s(x0), self.s(y0));
        pb.line_to(self.s(x1), self.s(y1));
        let Some(path) = pb.finish() else { return };
        let mut paint = Paint::default();
        paint.set_color_rgba8(color.r, color.g, color.b, color.a);
        paint.anti_alias = true;
        let stroke = Stroke { width: self.s(line_w), line_cap: tiny_skia::LineCap::Round, ..Stroke::default() };
        self.pix.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }

    /// 实心圆。
    pub fn disc(&mut self, cx: f32, cy: f32, r: f32, color: Color) {
        if r <= 0.0 || color.a == 0 {
            return;
        }
        let Some(path) = oval_path_px(self.s(cx), self.s(cy), self.s(r)) else { return };
        let mut paint = Paint::default();
        paint.set_color_rgba8(color.r, color.g, color.b, color.a);
        paint.anti_alias = true;
        self.pix.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }

    /// 描边圆环。
    pub fn ring(&mut self, cx: f32, cy: f32, r: f32, line_w: f32, color: Color) {
        if r <= 0.0 || line_w <= 0.0 || color.a == 0 {
            return;
        }
        let Some(path) = oval_path_px(self.s(cx), self.s(cy), self.s(r)) else { return };
        let mut paint = Paint::default();
        paint.set_color_rgba8(color.r, color.g, color.b, color.a);
        paint.anti_alias = true;
        let stroke = Stroke { width: self.s(line_w), ..Stroke::default() };
        self.pix.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }

    /// 圆弧(圆头描边):从 `start_deg` 起、扫过 `sweep_deg`(度,0=右、顺时针为正)。用折线逼近,
    /// 适合做环形进度 / 仪表。
    pub fn arc(&mut self, cx: f32, cy: f32, r: f32, start_deg: f32, sweep_deg: f32, line_w: f32, color: Color) {
        if r <= 0.0 || line_w <= 0.0 || color.a == 0 || sweep_deg == 0.0 {
            return;
        }
        let (cx, cy, r) = (self.s(cx), self.s(cy), self.s(r));
        let steps = ((sweep_deg.abs() / 4.0).ceil() as usize).max(2);
        let mut pb = PathBuilder::new();
        for i in 0..=steps {
            let t = start_deg + sweep_deg * (i as f32 / steps as f32);
            let (x, y) = (cx + r * t.to_radians().cos(), cy + r * t.to_radians().sin());
            if i == 0 {
                pb.move_to(x, y);
            } else {
                pb.line_to(x, y);
            }
        }
        let Some(path) = pb.finish() else { return };
        let mut paint = Paint::default();
        paint.set_color_rgba8(color.r, color.g, color.b, color.a);
        paint.anti_alias = true;
        let stroke = Stroke { width: self.s(line_w), line_cap: tiny_skia::LineCap::Round, ..Stroke::default() };
        self.pix.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }

    /// 多边形:`pts` 为逻辑坐标顶点;可填充、可描边(或都给)。
    pub fn polygon(&mut self, pts: &[(f32, f32)], fill: Option<Color>, stroke: Option<(f32, Color)>) {
        if pts.len() < 2 {
            return;
        }
        let Some(path) = self.poly_path(pts) else { return };
        if let Some(c) = fill {
            if c.a > 0 {
                let mut paint = Paint::default();
                paint.set_color_rgba8(c.r, c.g, c.b, c.a);
                paint.anti_alias = true;
                self.pix.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
            }
        }
        if let Some((lw, c)) = stroke {
            if lw > 0.0 && c.a > 0 {
                let mut paint = Paint::default();
                paint.set_color_rgba8(c.r, c.g, c.b, c.a);
                paint.anti_alias = true;
                let st = Stroke { width: self.s(lw), line_join: tiny_skia::LineJoin::Round, ..Stroke::default() };
                self.pix.stroke_path(&path, &paint, &st, Transform::identity(), None);
            }
        }
    }

    /// 雷达图:以 `(cx, cy)` 为心、`r` 为外接半径,`values`(各轴 0..1)画数据多边形 + 网格。
    pub fn radar(&mut self, cx: f32, cy: f32, r: f32, values: &[f32], st: &Radar) {
        let n = values.len();
        if n < 3 || r <= 0.0 {
            return;
        }
        let angle = |i: usize| (st.start_deg + 360.0 * i as f32 / n as f32).to_radians();
        // 网格同心多边形。
        let rings = st.rings.max(1);
        for ring in 1..=rings {
            let rr = r * ring as f32 / rings as f32;
            let pts: Vec<(f32, f32)> = (0..n).map(|i| (cx + rr * angle(i).cos(), cy + rr * angle(i).sin())).collect();
            self.polygon(&pts, None, Some((st.grid_w, st.grid)));
        }
        // 轴辐。
        for i in 0..n {
            let (ex, ey) = (cx + r * angle(i).cos(), cy + r * angle(i).sin());
            self.line(cx, cy, ex, ey, st.grid_w, st.grid);
        }
        // 数据多边形。
        let data: Vec<(f32, f32)> = (0..n)
            .map(|i| {
                let v = values[i].clamp(0.0, 1.0);
                (cx + r * v * angle(i).cos(), cy + r * v * angle(i).sin())
            })
            .collect();
        self.polygon(&data, Some(st.fill), Some((st.stroke_w, st.stroke)));
        if let Some((dr, dc)) = st.vertex_dot {
            for &(x, y) in &data {
                self.disc(x, y, dr, dc);
            }
        }
    }

    /// 把一段样式化段落渲成透明底小图并合成到 `(x, y)`,占位宽 `box_w`(逻辑像素;段落对齐在此宽内生效)。
    /// 返回该文本盒的渲染高度(逻辑像素),便于纵向流式排版。复用整条版式管线(CJK 整形 / 抗锯齿白拿)。
    pub fn text(
        &mut self,
        x: f32,
        y: f32,
        box_w: f32,
        opts: &RenderOptions,
        build: impl FnOnce(&mut crate::build::ParaBuilder),
    ) -> Result<f32> {
        let mut doc = Doc::new();
        doc.paragraph(build);
        self.text_doc(x, y, box_w, opts, &doc.build())
    }

    /// 同 [`text`](Self::text),但直接给一份 [`Document`](crate::Document)(可多段 / 表格等)。
    pub fn text_doc(&mut self, x: f32, y: f32, box_w: f32, opts: &RenderOptions, doc: &crate::Document) -> Result<f32> {
        let img = render_text_block(doc, opts, box_w, self.scale)?;
        let h = img.height() as f32 / self.scale;
        self.blit(&img, self.s(x).round() as i32, self.s(y).round() as i32);
        Ok(h)
    }

    /// 同 [`text`](Self::text),但把文字的**实际墨迹**纵向居中于中线 `cy`(逻辑像素),`x` 仍是左缘。给
    /// 「标签 / 数值 / 进度条 / 圆牌同一行居中」的卡片行用——按行盒居中会偏高(行盒底部留白多),故取首末非透明
    /// 行的中点对齐 `cy`,文字与同心线上的形状才真正齐平,不同字号也一致。
    ///
    /// 返回墨迹**右缘相对 `x` 的逻辑宽度**(全透明返回 0):据此把下一段接着往右摆,且每段各自居中于同一 `cy`,
    /// 不会因不同字号共用基线而让小字下沉。
    pub fn text_mid(
        &mut self,
        x: f32,
        cy: f32,
        box_w: f32,
        opts: &RenderOptions,
        build: impl FnOnce(&mut crate::build::ParaBuilder),
    ) -> Result<f32> {
        let mut doc = Doc::new();
        doc.paragraph(build);
        let img = render_text_block(&doc.build(), opts, box_w, self.scale)?;
        let (ink_cy, advance) = match ink_box(&img) {
            Some((_, y0, x1, y1)) => ((y0 + y1) as f32 / 2.0, (x1 + 1) as f32 / self.scale),
            None => (img.height() as f32 / 2.0, 0.0),
        };
        let py = (self.s(cy) - ink_cy).round() as i32;
        self.blit(&img, self.s(x).round() as i32, py);
        Ok(advance)
    }

    /// 把一张 RGBA 图按物理像素坐标 `(px, py)` 直接合成(source-over,不缩放)。
    pub fn blit(&mut self, img: &RgbaImage, px: i32, py: i32) {
        if img.width() == 0 || img.height() == 0 {
            return;
        }
        let Some(src) = rgba_to_pixmap(img) else { return };
        self.pix.draw_pixmap(px, py, src.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    }

    /// 按 `format` 编码为图片字节。
    pub fn encode(&self, format: OutputFormat) -> Result<Vec<u8>> {
        crate::paint::encode_pixmap(&self.pix, format)
    }

    /// 取(去预乘的)RGBA 图,供进一步合成。
    pub fn into_rgba(self) -> Result<RgbaImage> {
        let (w, h) = (self.pix.width(), self.pix.height());
        RgbaImage::from_raw(w, h, crate::paint::pixmap_to_rgba_bytes(&self.pix))
            .ok_or_else(|| Error::Layout("RGBA 缓冲尺寸不符".into()))
    }

    // —— 私有路径助手(逻辑坐标 → 物理) ——

    fn fill_rrect(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, paint: &Paint) {
        if let Some(path) = self.rrect_path(x, y, w, h, radius) {
            self.pix.fill_path(&path, paint, FillRule::Winding, Transform::identity(), None);
        }
    }

    fn rrect_path(&self, x: f32, y: f32, w: f32, h: f32, radius: f32) -> Option<tiny_skia::Path> {
        rrect_path_px(self.s(x), self.s(y), self.s(w), self.s(h), self.s(radius))
    }

    fn poly_path(&self, pts: &[(f32, f32)]) -> Option<tiny_skia::Path> {
        let mut pb = PathBuilder::new();
        pb.move_to(self.s(pts[0].0), self.s(pts[0].1));
        for &(x, y) in &pts[1..] {
            pb.line_to(self.s(x), self.s(y));
        }
        pb.close();
        pb.finish()
    }
}

/// 物理坐标的(可圆角)矩形路径。
fn rrect_path_px(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    if r <= 0.0 {
        return Rect::from_xywh(x, y, w, h).and_then(|rect| {
            let mut pb = PathBuilder::new();
            pb.push_rect(rect);
            pb.finish()
        });
    }
    let k = r * 0.552_285; // 圆弧三次贝塞尔近似
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

/// 物理坐标的圆形路径。
fn oval_path_px(cx: f32, cy: f32, r: f32) -> Option<tiny_skia::Path> {
    let rect = Rect::from_xywh(cx - r, cy - r, r * 2.0, r * 2.0)?;
    let mut pb = PathBuilder::new();
    pb.push_oval(rect);
    pb.finish()
}

/// 把一份文档渲成透明底、零边距、无页眉脚的 RGBA 小图(给画布合成文字用)。
fn render_text_block(doc: &crate::Document, base: &RenderOptions, box_w: f32, scale: f32) -> Result<RgbaImage> {
    let mut o = base.clone();
    o.width = box_w.max(1.0);
    o.padding = Insets::all(0.0);
    o.scale = scale;
    o.header = None;
    o.footer = None;
    o.theme.background = Color::rgba(0, 0, 0, 0); // 透明底,只留字
    let layout = crate::layout::layout_document(doc, &o)?;
    crate::paint::paint_rgba(&layout, &o)
}

/// 一张透明底文字图的**墨迹包围盒**(物理像素,含端点 `(x0, y0, x1, y1)`):任意通道 alpha 超阈值即算墨迹。
/// 全透明返回 `None`。供文字纵向居中(取 y 中点)与按实宽接排(取 x 右缘)共用。
fn ink_box(img: &RgbaImage) -> Option<(u32, u32, u32, u32)> {
    let (w, h) = (img.width(), img.height());
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    let mut any = false;
    for y in 0..h {
        for x in 0..w {
            if img.get_pixel(x, y).0[3] > 12 {
                any = true;
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    any.then_some((x0, y0, x1, y1))
}

/// `image::RgbaImage`(直 alpha)→ 预乘 `Pixmap`。
fn rgba_to_pixmap(img: &RgbaImage) -> Option<Pixmap> {
    let mut p = Pixmap::new(img.width(), img.height())?;
    let buf = p.pixels_mut();
    for (i, px) in img.pixels().enumerate() {
        let [r, g, b, a] = px.0;
        let pm = |c: u8| ((c as u16 * a as u16 + 127) / 255) as u8;
        buf[i] = PremultipliedColorU8::from_rgba(pm(r), pm(g), pm(b), a)
            .unwrap_or_else(|| PremultipliedColorU8::from_rgba(0, 0, 0, 0).unwrap());
    }
    Some(p)
}

fn skia(c: Color) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(c.r, c.g, c.b, c.a)
}
