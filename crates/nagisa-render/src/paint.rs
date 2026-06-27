//! 光栅化后端 —— 把 `layout` 的显示列表画进一张 `tiny_skia::Pixmap`,再按格式编码成图片字节
//! (PNG / WebP)。
//!
//! 矩形(背景 / 高亮 / 分割线 / 引用条 / 代码底 / 表格网格)走 tiny-skia 的抗锯齿填充;字形由
//! swash 栅格成覆盖率 / 彩色位图(经 [`crate::font`] 的字形缓存),按笔位 premultiplied-alpha
//! 合成进画布;内嵌图同样合成(可圆角裁切)。顺序:背景 → 垫底影(面板投影,衬在自家底色下)→
//! 衬底矩形 → 普通投影(图片)→ 图 → 角标底板/边框 → 字形(影先字后)→
//! 覆盖矩形(下划 / 删除)。

use image::codecs::png::{CompressionType, FilterType};
use image::{ExtendedColorType, ImageEncoder};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, PremultipliedColorU8, Rect, Transform};

use crate::error::{Error, Result};
use crate::font::GlyphImage;
use crate::layout::{DisplayItem, GlyphBatch, Layout, RectLayer, ShadowItem, StrokeItem};
use crate::model::Color;
use crate::theme::{OutputFormat, RenderOptions};

/// 渲染并按 `opts.format` 编码为图片字节。
pub(crate) fn paint(layout: &Layout, opts: &RenderOptions) -> Result<Vec<u8>> {
    let pix = render_pixmap(layout, opts)?;
    encode_pixmap(&pix, opts.format)
}

/// 把渲染好的画布按格式编码(也供 [`crate::canvas::Canvas`] 复用)。
pub(crate) fn encode_pixmap(pix: &Pixmap, format: OutputFormat) -> Result<Vec<u8>> {
    let enc = |e: image::ImageError| Error::Encode(e.to_string());
    let (w, h) = (pix.width(), pix.height());
    match format {
        // PNG 平衡:tiny-skia 直接编(预乘画布,自带 png 特性)。
        OutputFormat::Png => pix.encode_png().map_err(|e| Error::Encode(e.to_string())),
        // PNG 快压:image 的 PngEncoder + Fast 等级(去预乘的 RGBA)。
        OutputFormat::PngFast => {
            let rgba = pixmap_to_rgba_bytes(pix);
            let mut buf = Vec::new();
            image::codecs::png::PngEncoder::new_with_quality(&mut buf, CompressionType::Fast, FilterType::Adaptive)
                .write_image(&rgba, w, h, ExtendedColorType::Rgba8)
                .map_err(enc)?;
            Ok(buf)
        }
        // WebP:无损(image-webp 编码只支持无损)。单边上限 16383px:`Webp` 超限报明确 Err(不静默改
        // 格式),`WebpOrPng` 超限自动落 PNG(由调用方显式选定)。
        OutputFormat::Webp | OutputFormat::WebpOrPng => {
            if w > 16383 || h > 16383 {
                return if format == OutputFormat::WebpOrPng {
                    pix.encode_png().map_err(|e| Error::Encode(e.to_string()))
                } else {
                    Err(Error::Encode(format!(
                        "WebP 单边上限 16383px,当前 {w}×{h} 超限;改用 PNG(.png() / .webp_or_png())或调小 scale / width"
                    )))
                };
            }
            let rgba = pixmap_to_rgba_bytes(pix);
            let mut buf = Vec::new();
            image::codecs::webp::WebPEncoder::new_lossless(&mut buf)
                .write_image(&rgba, w, h, ExtendedColorType::Rgba8)
                .map_err(enc)?;
            Ok(buf)
        }
    }
}

/// 渲染为(去预乘的)RGBA 图,供合成用。
pub(crate) fn paint_rgba(layout: &Layout, opts: &RenderOptions) -> Result<image::RgbaImage> {
    let pix = render_pixmap(layout, opts)?;
    let (w, h) = (pix.width(), pix.height());
    image::RgbaImage::from_raw(w, h, pixmap_to_rgba_bytes(&pix))
        .ok_or_else(|| Error::Layout("RGBA 缓冲尺寸不符".into()))
}

/// 预乘画布 → 去预乘的 RGBA 字节(`straight = premul * 255 / a`;也供 [`crate::canvas::Canvas`] 复用)。
pub(crate) fn pixmap_to_rgba_bytes(pix: &Pixmap) -> Vec<u8> {
    let mut out = Vec::with_capacity((pix.width() * pix.height() * 4) as usize);
    for p in pix.pixels() {
        let a = p.alpha();
        if a == 0 {
            out.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            let un = |c: u8| ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
            out.extend_from_slice(&[un(p.red()), un(p.green()), un(p.blue()), a]);
        }
    }
    out
}

fn render_pixmap(layout: &Layout, opts: &RenderOptions) -> Result<Pixmap> {
    let mut pix = Pixmap::new(layout.width_px, layout.height_px)
        .ok_or_else(|| Error::Layout("画布尺寸非法(过大或为 0)".into()))?;
    pix.fill(to_skia(opts.theme.background));

    // 0) 垫底影(面板投影)——在自家底色之前,只染到画布背景上。
    for item in &layout.items {
        if let DisplayItem::Shadow(s) = item {
            if s.under {
                draw_shadow(&mut pix, s);
            }
        }
    }
    // 1) 衬底矩形(背景 / 高亮 / 底色,Under)——盖住垫底影的内腹。
    for item in &layout.items {
        if let DisplayItem::Rect { x, y, w, h, color, radius, layer: RectLayer::Under } = item {
            draw_rect(&mut pix, *x, *y, *w, *h, *color, *radius);
        }
    }
    // 2) 普通投影(图片)——压在衬底上、垫在图片下。
    for item in &layout.items {
        if let DisplayItem::Shadow(s) = item {
            if !s.under {
                draw_shadow(&mut pix, s);
            }
        }
    }
    // 3) 图片(M4 起填充 layout.images;radius > 0 圆角裁切)。
    for item in &layout.items {
        if let DisplayItem::Image { x, y, w, h, src, radius } = item {
            if let Some(img) = layout.images.get(*src) {
                draw_image(&mut pix, img, *x, *y, *w, *h, *radius);
            }
        }
    }
    // 4) 图面覆盖件:角标底板(Mid)与边框描边——在图之上、字形之下。
    for item in &layout.items {
        match item {
            DisplayItem::Rect { x, y, w, h, color, radius, layer: RectLayer::Mid } => {
                draw_rect(&mut pix, *x, *y, *w, *h, *color, *radius);
            }
            DisplayItem::StrokeRect(s) => {
                draw_stroke_rect(&mut pix, s);
            }
            DisplayItem::Ellipse { cx, cy, rx, ry, width, color } => {
                draw_ellipse(&mut pix, *cx, *cy, *rx, *ry, *width, *color);
            }
            DisplayItem::QuoteMark { x, y, size, color } => {
                draw_quote(&mut pix, *x, *y, *size, *color);
            }
            DisplayItem::CodeMark { x, y, size, color } => {
                draw_code_mark(&mut pix, *x, *y, *size, *color);
            }
            _ => {}
        }
    }
    // 5) 字形(带阴影的先画影、再画字)。
    for item in &layout.items {
        if let DisplayItem::Glyphs(batch) = item {
            draw_glyphs(&mut pix, opts, batch);
        }
    }
    // 6) 覆盖矩形(下划 / 删除,Over)——在字形之上。
    for item in &layout.items {
        if let DisplayItem::Rect { x, y, w, h, color, radius, layer: RectLayer::Over } = item {
            draw_rect(&mut pix, *x, *y, *w, *h, *color, *radius);
        }
    }
    Ok(pix)
}

/// 画投影:把(可圆角)矩形蒙版做三次盒模糊(逼近高斯),按色与 alpha 染色后合成。
/// `blur` 为软化半径(物理像素),≤ 0.5 退化为实心矩形。
fn draw_shadow(pix: &mut Pixmap, s: &ShadowItem) {
    let &ShadowItem { x, y, w, h, radius, blur, color, under: _ } = s;
    if w <= 0.0 || h <= 0.0 || color.a == 0 {
        return;
    }
    if blur <= 0.5 {
        draw_rect(pix, x, y, w, h, color, radius);
        return;
    }
    // 蒙版画布:四周各留 2×blur 余量装得下扩散。blur 先消毒(非有限 / 离谱值是
    // 上游脏数据,夹到 512px 影子已糊成雾),尺寸乘法用 checked 防回绕绕过守卫。
    let blur = if blur.is_finite() { blur.clamp(0.0, 512.0) } else { 0.0 };
    let pad = (blur * 2.0).ceil();
    let mw = (w + pad * 2.0).ceil() as usize;
    let mh = (h + pad * 2.0).ceil() as usize;
    if mw == 0 || mh == 0 || mw.checked_mul(mh).is_none_or(|n| n > 64_000_000) {
        return; // 蒙版过大(异常尺寸)直接放弃,不让一张影子吃光内存
    }
    // 1) 矩形(含圆角)覆盖率蒙版。
    let mut mask = vec![0f32; mw * mh];
    let r = radius.min(w / 2.0).min(h / 2.0).max(0.0);
    for (j, row) in mask.chunks_mut(mw).enumerate() {
        for (i, m) in row.iter_mut().enumerate() {
            let (fx, fy) = (i as f32 + 0.5 - pad, j as f32 + 0.5 - pad);
            if fx < 0.0 || fy < 0.0 || fx > w || fy > h {
                continue;
            }
            *m = if r > 0.0 { corner_coverage(fx, fy, w, h, r) } else { 1.0 };
        }
    }
    // 2) 三次盒模糊(半径 ≈ blur/2,三次叠加逼近高斯)。
    let br = ((blur / 2.0).round() as usize).max(1);
    for _ in 0..3 {
        box_blur_h(&mut mask, mw, mh, br);
        box_blur_v(&mut mask, mw, mh, br);
    }
    // 3) 染色成预乘小图,整张合成(tiny-skia 负责边界与混合)。
    let Some(mut sp) = Pixmap::new(mw as u32, mh as u32) else { return };
    {
        let px_buf = sp.pixels_mut();
        let (cr, cg, cb) = (color.r as f32, color.g as f32, color.b as f32);
        let ca = color.a as f32 / 255.0;
        for (k, &m) in mask.iter().enumerate() {
            let a = (m * ca * 255.0).round().clamp(0.0, 255.0) as u8;
            if a == 0 {
                continue;
            }
            let pm = |c: f32| ((c * a as f32) / 255.0).round() as u8;
            if let Some(c) = PremultipliedColorU8::from_rgba(pm(cr), pm(cg), pm(cb), a) {
                px_buf[k] = c;
            }
        }
    }
    pix.draw_pixmap(
        (x - pad).round() as i32,
        (y - pad).round() as i32,
        sp.as_ref(),
        &tiny_skia::PixmapPaint::default(),
        Transform::identity(),
        None,
    );
}

/// 水平盒模糊(滑动窗口均值,越界按边沿值钳住)。
fn box_blur_h(buf: &mut [f32], w: usize, h: usize, r: usize) {
    if w == 0 || r == 0 {
        return;
    }
    let mut row = vec![0f32; w];
    let norm = 1.0 / (2 * r + 1) as f32;
    for j in 0..h {
        let line = &buf[j * w..(j + 1) * w];
        let mut acc: f32 = 0.0;
        for k in -(r as isize)..=(r as isize) {
            acc += line[k.clamp(0, w as isize - 1) as usize];
        }
        for (i, out) in row.iter_mut().enumerate() {
            *out = acc * norm;
            let add = (i as isize + r as isize + 1).clamp(0, w as isize - 1) as usize;
            let sub = (i as isize - r as isize).clamp(0, w as isize - 1) as usize;
            acc += line[add] - line[sub];
        }
        buf[j * w..(j + 1) * w].copy_from_slice(&row);
    }
}

/// 垂直盒模糊(同 [`box_blur_h`],按列)。
fn box_blur_v(buf: &mut [f32], w: usize, h: usize, r: usize) {
    if h == 0 || r == 0 {
        return;
    }
    let mut col = vec![0f32; h];
    let norm = 1.0 / (2 * r + 1) as f32;
    for i in 0..w {
        let mut acc: f32 = 0.0;
        for k in -(r as isize)..=(r as isize) {
            acc += buf[k.clamp(0, h as isize - 1) as usize * w + i];
        }
        for (j, out) in col.iter_mut().enumerate() {
            *out = acc * norm;
            let add = (j as isize + r as isize + 1).clamp(0, h as isize - 1) as usize;
            let sub = (j as isize - r as isize).clamp(0, h as isize - 1) as usize;
            acc += buf[add * w + i] - buf[sub * w + i];
        }
        for (j, v) in col.iter().enumerate() {
            buf[j * w + i] = *v;
        }
    }
}

/// 画(可圆角)描边矩形:图片边框。线宽沿路径居中,内外各吃一半。
fn draw_stroke_rect(pix: &mut Pixmap, s: &StrokeItem) {
    let &StrokeItem { x, y, w, h, radius, width, color } = s;
    if w <= 0.0 || h <= 0.0 || width <= 0.0 {
        return;
    }
    let mut paint = Paint::default();
    paint.set_color_rgba8(color.r, color.g, color.b, color.a);
    paint.anti_alias = true;
    let path = if radius > 0.0 {
        rounded_rect(x, y, w, h, radius)
    } else {
        Rect::from_xywh(x, y, w, h).and_then(|r| {
            let mut pb = PathBuilder::new();
            pb.push_rect(r);
            pb.finish()
        })
    };
    if let Some(path) = path {
        let stroke = tiny_skia::Stroke { width, ..tiny_skia::Stroke::default() };
        pix.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

/// 画一个描边椭圆(文字圈注):以 `(cx, cy)` 为心、`rx`/`ry` 为半轴的椭圆轮廓。
fn draw_ellipse(pix: &mut Pixmap, cx: f32, cy: f32, rx: f32, ry: f32, width: f32, color: Color) {
    if rx <= 0.0 || ry <= 0.0 || width <= 0.0 {
        return;
    }
    let Some(rect) = Rect::from_xywh(cx - rx, cy - ry, rx * 2.0, ry * 2.0) else {
        return;
    };
    let mut pb = PathBuilder::new();
    pb.push_oval(rect);
    let Some(path) = pb.finish() else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color_rgba8(color.r, color.g, color.b, color.a);
    paint.anti_alias = true;
    let stroke = tiny_skia::Stroke { width, ..tiny_skia::Stroke::default() };
    pix.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
}

/// 画代码符号图标 `</>`:两枚尖括号 + 中间一道斜杠,圆头描边,矢量自绘。
/// `(x, y)` 为图标盒左上角,`size` 为盒高;整体宽约 `size * 1.55`。
fn draw_code_mark(pix: &mut Pixmap, x: f32, y: f32, size: f32, color: Color) {
    if size <= 0.0 {
        return;
    }
    let s = size;
    let w = s * 1.55;
    let mut pb = PathBuilder::new();
    // 左尖括号 <
    pb.move_to(x + s * 0.40, y + s * 0.16);
    pb.line_to(x + s * 0.08, y + s * 0.50);
    pb.line_to(x + s * 0.40, y + s * 0.84);
    // 右尖括号 >
    pb.move_to(x + w - s * 0.40, y + s * 0.16);
    pb.line_to(x + w - s * 0.08, y + s * 0.50);
    pb.line_to(x + w - s * 0.40, y + s * 0.84);
    // 斜杠 /
    pb.move_to(x + w / 2.0 + s * 0.13, y + s * 0.08);
    pb.line_to(x + w / 2.0 - s * 0.13, y + s * 0.92);
    let Some(path) = pb.finish() else { return };
    let mut paint = Paint::default();
    paint.set_color_rgba8(color.r, color.g, color.b, color.a);
    paint.anti_alias = true;
    let stroke = tiny_skia::Stroke {
        width: (s * 0.14).max(1.0),
        line_cap: tiny_skia::LineCap::Round,
        line_join: tiny_skia::LineJoin::Round,
        ..tiny_skia::Stroke::default()
    };
    pix.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
}

/// 画引号图标:两枚「6」形反引号(圆头贴底 + 上扬尾),矢量自绘不依赖字体字形。
/// `(x, y)` 为图标盒左上角,`size` 为盒高;整体宽约 `size * 1.3`。
fn draw_quote(pix: &mut Pixmap, x: f32, y: f32, size: f32, color: Color) {
    if size <= 0.0 {
        return;
    }
    let r = size * 0.30; // 球半径
    let mut pb = PathBuilder::new();
    for k in 0..2 {
        let bx = x + r + k as f32 * (size * 0.72); // 球心 x
        let by = y + size - r; // 球心 y(贴底)
        if let Some(ball) = Rect::from_xywh(bx - r, by - r, r * 2.0, r * 2.0) {
            pb.push_oval(ball);
        }
        // 尾:外缘从球左切点扬到右上尖端,内缘贴着外缘收回球顶,与球叠成细钩的「6」。
        let tip = (bx + r * 1.15, y);
        pb.move_to(bx - r, by);
        pb.cubic_to(bx - r, by - r * 1.7, bx - r * 0.1, y + r * 0.5, tip.0, tip.1);
        pb.cubic_to(bx + r * 0.25, y + r * 0.6, bx + r * 0.35, by - r * 1.6, bx + r * 0.5, by - r * 0.85);
        pb.close();
    }
    let Some(path) = pb.finish() else { return };
    let mut paint = Paint::default();
    paint.set_color_rgba8(color.r, color.g, color.b, color.a);
    paint.anti_alias = true;
    pix.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
}

/// 画一个(可圆角)实心矩形。
fn draw_rect(pix: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, color: Color, radius: f32) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let mut paint = Paint::default();
    paint.set_color_rgba8(color.r, color.g, color.b, color.a);
    paint.anti_alias = true;
    let id = Transform::identity();
    if radius > 0.0 {
        if let Some(path) = rounded_rect(x, y, w, h, radius) {
            pix.fill_path(&path, &paint, FillRule::Winding, id, None);
        }
    } else if let Some(rect) = Rect::from_xywh(x, y, w, h) {
        pix.fill_rect(rect, &paint, id, None);
    }
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let r = r.min(w / 2.0).min(h / 2.0);
    // 圆弧的三次贝塞尔近似(kappa):单段 quad 拐角外凸约 6%,与投影蒙版的精确
    // 圆角(corner_coverage)对不齐,叠在一起能看出错位。
    let k = r * 0.552_285;
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

/// 把一批字形合成进画布:先画整批的阴影(免得后字的影压在前字上),再画字形本体。
fn draw_glyphs(pix: &mut Pixmap, opts: &RenderOptions, batch: &GlyphBatch) {
    let pw = pix.width() as i32;
    let ph = pix.height() as i32;
    let pixels = pix.pixels_mut();
    opts.fonts.with_raster(|raster| {
        for g in &batch.glyphs {
            let Some(sh) = g.shadow else { continue };
            let Some(gf) = batch.fonts.get(g.font as usize) else { continue };
            let Some(img) = raster.image(gf, g.id) else { continue };
            let (sx, sy) = (g.x.saturating_add(sh.dx), g.y.saturating_add(sh.dy));
            if sh.blur <= 0.5 {
                blit_glyph(pixels, pw, ph, img, sx, sy, sh.color);
            } else {
                // 软影:中心 + 八方环形多次合成逼近模糊(单字形代价可控,免逐字离屏)。
                let r = (sh.blur * 0.6).round() as i32;
                let ring: [(i32, i32); 8] = [
                    (r, 0),
                    (-r, 0),
                    (0, r),
                    (0, -r),
                    (r * 7 / 10, r * 7 / 10),
                    (-r * 7 / 10, r * 7 / 10),
                    (r * 7 / 10, -r * 7 / 10),
                    (-r * 7 / 10, -r * 7 / 10),
                ];
                let soft = Color { a: (sh.color.a as u16 * 2 / 5) as u8, ..sh.color };
                blit_glyph(pixels, pw, ph, img, sx, sy, soft);
                for (dx, dy) in ring {
                    let faint = Color { a: (sh.color.a as u16 / 5) as u8, ..sh.color };
                    blit_glyph(pixels, pw, ph, img, sx + dx, sy + dy, faint);
                }
            }
        }
        for g in &batch.glyphs {
            let Some(gf) = batch.fonts.get(g.font as usize) else { continue };
            if let Some(img) = raster.image(gf, g.id) {
                blit_glyph(pixels, pw, ph, img, g.x, g.y, g.color);
            }
        }
    });
}

/// 按笔位 `pen_x` / 基线 `base_y` 把一个字形位图合成进画布。
fn blit_glyph(
    pixels: &mut [PremultipliedColorU8],
    pw: i32,
    ph: i32,
    img: &GlyphImage,
    pen_x: i32,
    base_y: i32,
    color: Color,
) {
    // 笔位加偏移用饱和加,防极端坐标下的 i32 溢出 panic。
    let ox = pen_x.saturating_add(img.left);
    let oy = base_y.saturating_sub(img.top);
    let gw = img.width as i32;
    let gh = img.height as i32;
    for j in 0..gh {
        let py = oy.saturating_add(j);
        if py < 0 || py >= ph {
            continue;
        }
        for i in 0..gw {
            let px = ox.saturating_add(i);
            if px < 0 || px >= pw {
                continue;
            }
            // py/px 已 ≥ 0,用 usize 算下标,避免 py*pw 在 i32 下溢出。
            let dst = &mut pixels[py as usize * pw as usize + px as usize];
            if img.color {
                // 彩色字形(emoji):像素自带色,但传入色的 alpha 仍要乘进去——
                // 软影的淡色副本、半透明水印里的 emoji 不该实心。
                let k = ((j * gw + i) * 4) as usize;
                let (r, g, b, a) = (img.data[k], img.data[k + 1], img.data[k + 2], img.data[k + 3]);
                blend(dst, r, g, b, color.a, a);
            } else {
                let cov = img.data[(j * gw + i) as usize];
                blend(dst, color.r, color.g, color.b, color.a, cov);
            }
        }
    }
}

/// source-over 合成:源色 `(r,g,b)` 直通、源 alpha = `a_color * cov`,叠到预乘的目标像素上。
fn blend(dst: &mut PremultipliedColorU8, r: u8, g: u8, b: u8, a_color: u8, cov: u8) {
    let sa = (cov as f32 / 255.0) * (a_color as f32 / 255.0);
    if sa <= 0.0 {
        return;
    }
    let inv = 1.0 - sa;
    let (dr, dg, db, da) = (dst.red() as f32, dst.green() as f32, dst.blue() as f32, dst.alpha() as f32);
    let nr = (r as f32 * sa + dr * inv).round() as u8;
    let ng = (g as f32 * sa + dg * inv).round() as u8;
    let nb = (b as f32 * sa + db * inv).round() as u8;
    let na = (255.0 * sa + da * inv).round() as u8;
    if let Some(c) = PremultipliedColorU8::from_rgba(nr, ng, nb, na) {
        *dst = c;
    }
}

/// 把一张 RGBA 图缩放贴入目标矩形。尺寸不同先重采样(缩小 Lanczos3 / 放大 CatmullRom)
/// 再逐像素合成——最近邻在头像类下采样会出锯齿 / 摩尔纹。`radius > 0` 时圆角裁切:
/// 角区像素按到圆角圆心的距离算覆盖率,乘进源 alpha(1px 软边抗锯齿)。
fn draw_image(pix: &mut Pixmap, img: &image::RgbaImage, x: f32, y: f32, w: f32, h: f32, radius: f32) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let (dw, dh) = ((w.round() as u32).max(1), (h.round() as u32).max(1));
    let resized;
    let src = if (dw, dh) == (img.width(), img.height()) {
        img
    } else {
        let filter = if dw < img.width() || dh < img.height() {
            image::imageops::FilterType::Lanczos3
        } else {
            image::imageops::FilterType::CatmullRom
        };
        resized = image::imageops::resize(img, dw, dh, filter);
        &resized
    };
    let r = radius.min(w / 2.0).min(h / 2.0).max(0.0);
    let (pw, ph) = (pix.width() as i32, pix.height() as i32);
    let (ox, oy) = (x.round() as i32, y.round() as i32);
    let pixels = pix.pixels_mut();
    for j in 0..dh as i32 {
        let py = oy.saturating_add(j);
        if py < 0 || py >= ph {
            continue;
        }
        for i in 0..dw as i32 {
            let px = ox.saturating_add(i);
            if px < 0 || px >= pw {
                continue;
            }
            let p = src.get_pixel(i as u32, j as u32).0;
            let mut a = p[3] as f32;
            if r > 0.0 {
                a *= corner_coverage(i as f32 + 0.5, j as f32 + 0.5, dw as f32, dh as f32, r);
            }
            blend(
                &mut pixels[py as usize * pw as usize + px as usize],
                p[0],
                p[1],
                p[2],
                255,
                a.round().clamp(0.0, 255.0) as u8,
            );
        }
    }
}

/// 点 `(fx, fy)`(矩形局部坐标)在「圆角半径 `r` 的 `w×h` 圆角矩形」内的覆盖率:
/// 角区按到圆心的距离给 1px 软边,其余区域恒 1。
fn corner_coverage(fx: f32, fy: f32, w: f32, h: f32, r: f32) -> f32 {
    // 离最近的水平/垂直边的「向内深度」;两向都浅于 r 才落在角区。
    let cx = if fx < r {
        r - fx
    } else if fx > w - r {
        fx - (w - r)
    } else {
        return 1.0;
    };
    let cy = if fy < r {
        r - fy
    } else if fy > h - r {
        fy - (h - r)
    } else {
        return 1.0;
    };
    let d = (cx * cx + cy * cy).sqrt();
    (r - d + 0.5).clamp(0.0, 1.0)
}

fn to_skia(c: Color) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(c.r, c.g, c.b, c.a)
}
