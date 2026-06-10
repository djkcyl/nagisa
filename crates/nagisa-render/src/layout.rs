//! 版式引擎 —— 把文档 IR 排成一份**显示列表**(带物理像素坐标的字形 / 矩形 / 图片),交给
//! `paint` 光栅化。纵向块流:维护游标 `y`,逐块测高、定位。行内排版交给 cosmic-text(整形 /
//! 断行 / 字体回退 / CJK+拉丁混排 / 对齐)。
//!
//! **单位**:所有逻辑尺寸(宽 / 内边距 / 字号)在此乘 `scale` 换成设备像素,即一切坐标都是
//! 物理像素。块容器(引用 / 列表项 / 栏 / 单元格)递归排版时收窄可用宽。

use cosmic_text::{Attrs, Buffer, Family, Metrics, Shaping, Style, Weight};

use crate::error::{Error, Result};
use crate::font::FontHandle;
use std::collections::HashMap;

use crate::model::{
    Align, Block, BlockImage, Cell, Color, Column, Columns, Document, FontRole, ImageSource,
    Inline, Length, List, ListKind, Table, TextStyle,
};
use crate::theme::{RenderOptions, Theme};

/// 画布单边像素上限(防超大尺寸的整数溢出 / 巨幅分配)。
const MAX_DIM: u32 = 30_000;
/// 画布总像素上限(~40MP,约 160MB RGBA):防 OOM,也保证 i32 像素下标不溢出。
const MAX_AREA: u64 = 40_000_000;
/// 字号(设备像素)安全下限:必须 > 0,否则 cosmic-text 整形零步进会死循环。
const MIN_PX: f32 = 1.0;
/// 字号(设备像素)安全上限:防字形栅格(zeno)整数溢出与单字撑爆画布。
const MAX_PX: f32 = 2_000.0;
/// 图片来源单文件最大字节数(防 `/dev/zero` 之类无 EOF 文件被无界读 → OOM)。
const MAX_IMAGE_BYTES: u64 = 32 * 1024 * 1024;

/// 把字号(设备像素)夹到安全区间:非有限 → 下限;否则夹进 `[MIN_PX, MAX_PX]`。
/// 用户可经 `size` 倍率 / `Theme::base_size` / `heading_scale` 把字号推到 0 或天文数字,
/// 全在此一处兜住。
fn safe_px(px: f32) -> f32 {
    if px.is_finite() {
        px.clamp(MIN_PX, MAX_PX)
    } else {
        MIN_PX
    }
}

/// 排好版的一页:显示列表 + 画布尺寸 + 解码好的内嵌图。
pub(crate) struct Layout {
    pub items: Vec<DisplayItem>,
    pub width_px: u32,
    pub height_px: u32,
    pub images: Vec<image::RgbaImage>,
}

/// 一条绘制原语(坐标均为物理像素)。
pub(crate) enum DisplayItem {
    /// 一批字形(各自带颜色,可带文字阴影)。
    Glyphs(Vec<PlacedGlyph>),
    /// 实心(可圆角)矩形:背景 / 高亮 / 分割线 / 引用条 / 代码底 / 角标底板 / 下划删除等。
    /// 绘制层见 [`RectLayer`]。
    Rect { x: f32, y: f32, w: f32, h: f32, color: Color, radius: f32, layer: RectLayer },
    /// 一张图(`src` 是 `Layout::images` 的下标);`radius > 0` 时圆角裁切图面。
    Image { x: f32, y: f32, w: f32, h: f32, src: usize, radius: f32 },
    /// 投影:模糊圆角矩形,画在图片之前。
    Shadow(ShadowItem),
    /// 描边(可圆角)矩形:图片边框。画在图片之后、字形之前。
    StrokeRect(StrokeItem),
}

/// 投影绘制参数(均物理像素;`x`/`y` 已含偏移)。
pub(crate) struct ShadowItem {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub radius: f32,
    pub blur: f32,
    pub color: Color,
}

/// 描边矩形绘制参数(图片边框;线宽沿路径居中)。
pub(crate) struct StrokeItem {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub radius: f32,
    pub width: f32,
    pub color: Color,
}

/// 实心矩形的绘制层:`Under` 在图片与字形之前(衬底:高亮 / 底色 / 分割线),
/// `Mid` 在图片之后、字形之前(角标底板),`Over` 在字形之后(下划 / 删除)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RectLayer {
    Under,
    Mid,
    Over,
}

/// 定位好的字形:缓存键(给 SwashCache 取位图)+ 笔位(x)/基线(y)+ 颜色 + 可选阴影。
pub(crate) struct PlacedGlyph {
    pub cache_key: cosmic_text::CacheKey,
    pub x: i32,
    pub y: i32,
    pub color: Color,
    pub shadow: Option<GlyphShadow>,
}

/// 字形阴影(物理像素;由 `TextStyle::shadow` 按 scale 折算)。
#[derive(Clone, Copy, Debug)]
pub(crate) struct GlyphShadow {
    pub dx: i32,
    pub dy: i32,
    pub blur: f32,
    pub color: Color,
}

/// 把文档排成显示列表。
pub(crate) fn layout_document(doc: &Document, opts: &RenderOptions) -> Result<Layout> {
    let sc = opts.scale;
    if opts.width <= 0.0 || sc <= 0.0 || !opts.width.is_finite() || !sc.is_finite() {
        return Err(Error::Layout("宽度 / scale 必须为正且有限".into()));
    }
    let pad = opts.padding;
    if [pad.top, pad.right, pad.bottom, pad.left].iter().any(|v| !v.is_finite() || *v < 0.0) {
        return Err(Error::Layout("内边距必须为非负且有限".into()));
    }
    let content_w = ((opts.width - pad.left - pad.right) * sc).max(1.0);
    let x_left = pad.left * sc;

    let mut ctx = LayoutCtx { opts, sc, items: Vec::new(), images: Vec::new(), y: pad.top * sc };
    for (i, block) in doc.blocks.iter().enumerate() {
        ctx.block(block, x_left, content_w, i == 0);
    }

    let height_f = ctx.y + pad.bottom * sc;
    if !height_f.is_finite() {
        return Err(Error::Layout("内容高度非有限(检查字号 / 图宽 / 内边距)".into()));
    }
    let width_px = (opts.width * sc).round().max(1.0) as u32;
    let height_px = height_f.round().max(1.0) as u32;
    if width_px > MAX_DIM || height_px > MAX_DIM || width_px as u64 * height_px as u64 > MAX_AREA {
        return Err(Error::Layout(format!(
            "画布过大:{width_px}×{height_px}(超出 {MAX_DIM}px 单边 / {MAX_AREA} 像素上限,调小 width / scale 或拆分内容)"
        )));
    }
    Ok(Layout { items: ctx.items, width_px, height_px, images: ctx.images })
}

/// 排版游标 + 输出累积。块在内容区 `[x, x+w)` 内排,递归(引用 / 列表项)时收窄。
struct LayoutCtx<'a> {
    opts: &'a RenderOptions,
    sc: f32,
    items: Vec<DisplayItem>,
    images: Vec<image::RgbaImage>,
    y: f32,
}

impl LayoutCtx<'_> {
    /// 排一个块(`first` 控制是否抑制顶部间距)。
    fn block(&mut self, b: &Block, x: f32, w: f32, first: bool) {
        let base = self.opts.theme.base_size;
        let sc = self.sc;
        match b {
            Block::Heading { level, inlines, align } => {
                let k = self.opts.theme.heading_scale[(*level as usize).clamp(1, 6) - 1];
                let before = if first { 0.0 } else { base * sc * 0.6 };
                self.text_block(inlines, *align, base * k, true, x, w, before, base * sc * 0.3);
            }
            Block::Paragraph { inlines, align } => {
                self.text_block(inlines, *align, base, false, x, w, 0.0, base * sc * 0.55);
            }
            Block::Divider => {
                self.y += base * sc * 0.45;
                let th = (2.0 * sc).max(1.0);
                self.items.push(DisplayItem::Rect {
                    x,
                    y: self.y,
                    w,
                    h: th,
                    color: self.opts.theme.muted,
                    radius: 0.0,
                    layer: RectLayer::Under,
                });
                self.y += th + base * sc * 0.45;
            }
            Block::Quote(inner) => self.quote(inner, x, w),
            Block::Code { lang, text } => self.code(lang.as_deref(), text, x, w),
            Block::List(list) => self.list(list, x, w),
            Block::Image(bi) => self.image(bi, x, w),
            Block::Columns(c) => self.columns(c, x, w),
            Block::Table(t) => self.table(t, x, w),
        }
    }

    /// 文字块:间距 + 整形定位 + 推进游标。
    #[allow(clippy::too_many_arguments)]
    fn text_block(
        &mut self,
        inlines: &[Inline],
        align: Align,
        base_logical: f32,
        bold: bool,
        x: f32,
        w: f32,
        before: f32,
        after: f32,
    ) {
        self.y += before;
        let h = self.emit_text(inlines, align, base_logical, bold, x, self.y, w);
        self.y += h + after;
    }

    /// 在 `(x, y)` 起、宽 `w` 处整形一段行内内容,推入显示列表,返回其高度(不推进游标)。
    #[allow(clippy::too_many_arguments)]
    fn emit_text(
        &mut self,
        inlines: &[Inline],
        align: Align,
        base_logical: f32,
        bold: bool,
        x: f32,
        y: f32,
        w: f32,
    ) -> f32 {
        let (glyphs, decos, h) = shape_text(
            &self.opts.fonts,
            &self.opts.theme,
            inlines,
            align,
            base_logical,
            bold,
            self.sc,
            w,
            x,
            y,
        );
        self.items.extend(decos);
        if !glyphs.is_empty() {
            self.items.push(DisplayItem::Glyphs(glyphs));
        }
        h
    }

    /// 引用:左侧强调色竖条 + 内缩,递归排内部块;竖条高度按内容算(内容排完再补)。
    fn quote(&mut self, inner: &[Block], x: f32, w: f32) {
        let (base, sc) = (self.opts.theme.base_size, self.sc);
        self.y += base * sc * 0.3;
        let bar_w = (4.0 * sc).max(2.0);
        let gap = base * sc * 0.45;
        let ix = x + bar_w + gap;
        let iw = (w - bar_w - gap).max(1.0);
        let y0 = self.y;
        for (i, b) in inner.iter().enumerate() {
            self.block(b, ix, iw, i == 0);
        }
        let h = (self.y - y0).max(0.0);
        self.items.push(DisplayItem::Rect {
            x,
            y: y0,
            w: bar_w,
            h,
            color: self.opts.theme.accent,
            radius: bar_w / 2.0,
            layer: RectLayer::Under,
        });
        self.y += base * sc * 0.3;
    }

    /// 代码块:等宽 + 圆角底色盒 + 内边距 + 软换行;有语言标签则在盒内右上角渲染成小字。
    fn code(&mut self, lang: Option<&str>, text: &str, x: f32, w: f32) {
        let (base, sc) = (self.opts.theme.base_size, self.sc);
        self.y += base * sc * 0.4;
        let pad = base * sc * 0.45;
        let ix = x + pad;
        let iw = (w - 2.0 * pad).max(1.0);
        let y_bg = self.y;
        let mut y_text = y_bg + pad;
        if let Some(l) = lang.map(str::trim).filter(|l| !l.is_empty()) {
            let tag = vec![Inline::Text {
                text: l.to_string(),
                style: TextStyle {
                    font: FontRole::Mono,
                    color: Some(self.opts.theme.muted),
                    size: 0.72,
                    ..TextStyle::default()
                },
            }];
            let th = self.emit_text(&tag, Align::Right, base, false, ix, y_bg + pad * 0.5, iw);
            y_text = y_bg + pad * 0.5 + th + base * sc * 0.1;
        }
        let inlines = vec![Inline::Text {
            text: text.to_string(),
            style: TextStyle {
                font: FontRole::Mono,
                color: Some(self.opts.theme.code_text),
                ..TextStyle::default()
            },
        }];
        let h = self.emit_text(&inlines, Align::Left, base, false, ix, y_text, iw);
        let bg_h = y_text + h + pad - y_bg;
        self.items.push(DisplayItem::Rect {
            x,
            y: y_bg,
            w,
            h: bg_h,
            color: self.opts.theme.code_bg,
            radius: 8.0 * sc,
            layer: RectLayer::Under,
        });
        self.y = y_bg + bg_h + base * sc * 0.4;
    }

    /// 列表:gutter 画标记(符号 / 序号 / 任务勾选),内容内缩递归(支持嵌套)。
    /// 标记区按最宽标记自适应(多位数序号不挤不折行),标记右对齐到内容侧。
    fn list(&mut self, list: &List, x: f32, w: f32) {
        let (base, sc) = (self.opts.theme.base_size, self.sc);
        self.y += base * sc * 0.2;
        let markers: Vec<Vec<Inline>> = list
            .items
            .iter()
            .enumerate()
            .map(|(idx, item)| vec![marker_inline(list, idx, item.check, &self.opts.theme)])
            .collect();
        let zone = markers
            .iter()
            .map(|m| measure_natural(&self.opts.fonts, &self.opts.theme, m, base, false, sc))
            .fold(base * sc, f32::max)
            + 1.0; // 1px 余量,防舍入导致标记在区内折行
        let gap = base * sc * 0.5;
        let gutter = zone + gap;
        let ix = x + gutter;
        let iw = (w - gutter).max(1.0);
        for (item, marker) in list.items.iter().zip(&markers) {
            let y_item = self.y;
            // 标记与该项首行同基线:右对齐排在标记区,不推进游标。
            self.emit_text(marker, Align::Right, base, false, x, y_item, zone);
            for (i, b) in item.blocks.iter().enumerate() {
                self.block(b, ix, iw, i == 0);
            }
            // 空项也至少占一行,避免重叠。
            if self.y <= y_item {
                self.y = y_item + base * sc * self.opts.theme.line_height;
            }
        }
        self.y += base * sc * 0.2;
    }

    /// 块级图:解码 → 等比缩放到目标宽 → 按对齐定位 → 可挂图注。解码失败给个占位提示。
    fn image(&mut self, bi: &BlockImage, x: f32, w: f32) {
        let (base, sc) = (self.opts.theme.base_size, self.sc);
        self.y += base * sc * 0.3;

        let Some(rgba) = decode_image(&bi.src, &self.opts.images) else {
            let ph = vec![Inline::Text {
                text: "⟨图片缺失⟩".to_string(),
                style: TextStyle { color: Some(self.opts.theme.muted), ..TextStyle::default() },
            }];
            let h = self.emit_text(&ph, Align::Left, base, false, x, self.y, w);
            self.y += h + base * sc * 0.4;
            return;
        };

        let (iw, ih) = (rgba.width() as f32, rgba.height() as f32);
        let req = match bi.width {
            Some(Length::Px(p)) => p * sc,
            Some(Length::Percent(pct)) => w * (pct / 100.0),
            None => iw.min(w), // 不放大超过原图,过宽则缩到内容宽
        };
        // 非有限(NaN/inf)请求宽退回自然宽,否则游标会变 NaN 让整页坍成 1px(静默丢内容)。
        let dw = if req.is_finite() { req } else { iw.min(w) }.clamp(1.0, w.max(1.0));
        let dh = if iw > 0.0 { dw * ih / iw } else { dw };

        let ix = match bi.align {
            Align::Center => x + (w - dw) / 2.0,
            Align::Right => x + (w - dw),
            _ => x,
        };
        let src = self.images.len();
        self.images.push(rgba);
        let radius = if bi.decor.radius.is_finite() { (bi.decor.radius * sc).max(0.0) } else { 0.0 };
        // 投影先于图片入列(画在衬底之后、图片之前),位置已含偏移。
        if let Some(sh) = &bi.decor.shadow {
            self.items.push(DisplayItem::Shadow(ShadowItem {
                x: ix + sh.dx * sc,
                y: self.y + sh.dy * sc,
                w: dw,
                h: dh,
                radius,
                blur: (sh.blur * sc).max(0.0),
                color: sh.color,
            }));
        }
        self.items.push(DisplayItem::Image { x: ix, y: self.y, w: dw, h: dh, src, radius });
        self.image_overlay(&bi.decor, ix, self.y, dw, dh, radius);
        self.y += dh;

        if let Some(cap) = &bi.caption {
            self.y += base * sc * 0.2;
            // 图注居中到图片正下方(不是整个内容宽)——图不居中时也不脱节;
            // 极窄图给个 4em 下限,免得图注一字一行。
            let cap_w = dw.max(base * sc * 4.0).min(w.max(1.0));
            let cap_x = (ix + dw / 2.0 - cap_w / 2.0).clamp(x, x + (w - cap_w).max(0.0));
            let h = self.emit_text(cap, Align::Center, base * 0.85, false, cap_x, self.y, cap_w);
            self.y += h;
        }
        self.y += base * sc * 0.4;
    }

    /// 图面装饰层:边框(描边随圆角)/ 角标(圆角底板 + 文字)/ 水印(半透明文字)。
    /// 全部叠在图面坐标系内,不动游标、不改布局尺寸。`radius` 已是物理像素。
    fn image_overlay(
        &mut self,
        decor: &crate::model::ImageDecor,
        ix: f32,
        iy: f32,
        dw: f32,
        dh: f32,
        radius: f32,
    ) {
        let (base, sc) = (self.opts.theme.base_size, self.sc);
        let line_mult = self.opts.theme.line_height;

        if let Some(b) = &decor.border {
            self.items.push(DisplayItem::StrokeRect(StrokeItem {
                x: ix,
                y: iy,
                w: dw,
                h: dh,
                radius,
                width: (b.width * sc).max(1.0),
                color: b.color,
            }));
        }

        if let Some(badge) = &decor.badge {
            let px = base * badge.size; // 逻辑字号
            let inl = [Inline::Text {
                text: badge.text.clone(),
                style: TextStyle { color: Some(badge.fg), ..TextStyle::default() },
            }];
            let tw = measure_natural(&self.opts.fonts, &self.opts.theme, &inl, px, false, sc);
            let line_h = px * sc * line_mult;
            let (pad_x, pad_y) = (px * sc * 0.45, px * sc * 0.12);
            let (bw, bh) = (tw + pad_x * 2.0, line_h + pad_y * 2.0);
            let margin = px * sc * 0.5;
            let (bx, by) = anchor_pos(badge.anchor, (ix, iy, dw, dh), (bw, bh), margin);
            self.items.push(DisplayItem::Rect {
                x: bx,
                y: by,
                w: bw,
                h: bh,
                color: badge.bg,
                radius: bh * 0.25,
                layer: RectLayer::Mid,
            });
            // +2px 余量防舍入折行;字形阶段天然画在 Mid 底板之上。
            self.emit_text(&inl, Align::Left, px, false, bx + pad_x, by + pad_y, tw + 2.0);
        }

        if let Some(wm) = &decor.watermark {
            let px = base * wm.size;
            let inl = [Inline::Text {
                text: wm.text.clone(),
                style: TextStyle { color: Some(wm.color), ..TextStyle::default() },
            }];
            let tw = measure_natural(&self.opts.fonts, &self.opts.theme, &inl, px, false, sc);
            let line_h = px * sc * line_mult;
            let margin = px * sc * 0.5;
            let (wx, wy) = anchor_pos(wm.anchor, (ix, iy, dw, dh), (tw, line_h), margin);
            self.emit_text(&inl, Align::Left, px, false, wx, wy, tw + 2.0);
        }
    }

    /// 显式并排栏:按 `weight` 瓜分(减去栏间距后的)可用宽,每栏独立排块,行高取最高栏(顶对齐)。
    fn columns(&mut self, c: &Columns, x: f32, w: f32) {
        let (base, sc) = (self.opts.theme.base_size, self.sc);
        let cols: Vec<&Column> = c.cols.iter().filter(|col| col.weight > 0.0).collect();
        if cols.is_empty() {
            return;
        }
        self.y += base * sc * 0.3;
        let gap = c.gap.map(|g| g * sc).unwrap_or(base * sc * 0.6);
        let avail = (w - gap * (cols.len() - 1) as f32).max(1.0);
        let total_w: f32 = cols.iter().map(|col| col.weight).sum();

        let y_top = self.y;
        let mut cx = x;
        let mut max_h = 0.0f32;
        for col in cols {
            let cw = (avail * col.weight / total_w).max(1.0);
            let (items, images, y_bottom) = self.sub_layout(&col.blocks, cx, y_top, cw);
            self.merge(items, images);
            max_h = max_h.max(y_bottom - y_top);
            cx += cw + gap;
        }
        self.y = y_top + max_h + base * sc * 0.3;
    }

    /// 子布局:在 `(x, y_top)` 起、宽 `w`,把一段块排进一个独立累积,返回 `(绘制项, 解码图, 底部 y)`。
    /// 不动主游标;坐标已是绝对物理像素。
    fn sub_layout(
        &self,
        blocks: &[Block],
        x: f32,
        y_top: f32,
        w: f32,
    ) -> (Vec<DisplayItem>, Vec<image::RgbaImage>, f32) {
        let mut sub = LayoutCtx {
            opts: self.opts,
            sc: self.sc,
            items: Vec::new(),
            images: Vec::new(),
            y: y_top,
        };
        for (i, b) in blocks.iter().enumerate() {
            sub.block(b, x, w, i == 0);
        }
        (sub.items, sub.images, sub.y)
    }

    /// 把子布局结果并入主累积:图片 `src` 下标按当前图集长度偏移。
    fn merge(&mut self, items: Vec<DisplayItem>, images: Vec<image::RgbaImage>) {
        let offset = self.images.len();
        self.images.extend(images);
        for mut it in items {
            if let DisplayItem::Image { src, .. } = &mut it {
                *src += offset;
            }
            self.items.push(it);
        }
    }

    /// 表格:求列宽(自适应 + 手动限宽)→ 逐行排(表头加粗+浅底)→ 行底分隔线。
    fn table(&mut self, t: &Table, x: f32, w: f32) {
        let (base, sc) = (self.opts.theme.base_size, self.sc);
        let ncols = t
            .header
            .as_ref()
            .map(|h| h.len())
            .into_iter()
            .chain(t.rows.iter().map(|r| r.len()))
            .chain(std::iter::once(t.cols.len()))
            .max()
            .unwrap_or(0);
        if ncols == 0 {
            return;
        }
        self.y += base * sc * 0.3;
        let pad = t.style.pad_x.unwrap_or(base * 0.32) * sc;
        let pad_v = t.style.pad_y.unwrap_or(base * 0.26) * sc;
        let widths = self.solve_widths(t, ncols, w, pad);

        let table_w: f32 = widths.iter().sum();
        let mut col_x = Vec::with_capacity(ncols);
        let mut cx = x;
        for &cw in &widths {
            col_x.push(cx);
            cx += cw;
        }

        // 排各行,记录行间内部边界 y(用于横线)。
        let table_top = self.y;
        let mut inner = Vec::new();
        if let Some(h) = &t.header {
            self.table_row(h, t, &widths, &col_x, x, table_w, true, pad, pad_v);
            inner.push(self.y);
        }
        for row in &t.rows {
            self.table_row(row, t, &widths, &col_x, x, table_w, false, pad, pad_v);
            inner.push(self.y);
        }
        let table_bottom = self.y;
        inner.pop(); // 末行底边归外框,不算内部横线

        // 网格线(细、淡 border 色),按开关画。
        let line = (1.0 * sc).max(1.0);
        let border = self.opts.theme.border;
        let grid = t.style.grid;
        if grid.horizontal {
            for &yb in &inner {
                self.items.push(hrule(x, yb, table_w, line, border));
            }
        }
        if grid.outer {
            for yb in [table_top, table_bottom] {
                self.items.push(hrule(x, yb, table_w, line, border));
            }
        }
        if grid.vertical {
            for &vx in col_x.iter().skip(1) {
                self.items.push(vrule(vx, table_top, table_bottom, line, border));
            }
        }
        if grid.outer {
            for vx in [x, x + table_w] {
                self.items.push(vrule(vx, table_top, table_bottom, line, border));
            }
        }
        self.y += base * sc * 0.3;
    }

    /// 排一行(表头或数据行):各单元格在列内按对齐排,行高取最高;表头浅底(可关)+ 单元格背景。
    /// 不画分隔线(交给 [`Self::table`] 统一按开关画);游标推进到行底。
    #[allow(clippy::too_many_arguments)]
    fn table_row(
        &mut self,
        cells: &[Cell],
        t: &Table,
        widths: &[f32],
        col_x: &[f32],
        table_x: f32,
        table_w: f32,
        header: bool,
        pad: f32,
        pad_v: f32,
    ) {
        let (base, sc) = (self.opts.theme.base_size, self.sc);
        let row_top = self.y;
        let y_text = row_top + pad_v;
        let mut content_h = 0.0f32;
        for (k, cell) in cells.iter().enumerate() {
            if k >= widths.len() {
                break;
            }
            let align = t.cols.get(k).map(|c| c.align).unwrap_or(Align::Left);
            let cwidth = (widths[k] - 2.0 * pad).max(1.0);
            let h = self.emit_text(&cell.inlines, align, base, header, col_x[k] + pad, y_text, cwidth);
            content_h = content_h.max(h);
        }
        if content_h <= 0.0 {
            content_h = base * sc * self.opts.theme.line_height;
        }
        let row_bottom = y_text + content_h + pad_v;
        if header && t.style.header_fill {
            self.items.push(DisplayItem::Rect {
                x: table_x,
                y: row_top,
                w: table_w,
                h: row_bottom - row_top,
                color: self.opts.theme.code_bg,
                radius: 0.0,
                layer: RectLayer::Under,
            });
        }
        // 单元格背景填色(盖在表头浅底之上、字形之下)。
        for (k, cell) in cells.iter().enumerate() {
            if k >= widths.len() {
                break;
            }
            if let Some(bg) = cell.bg {
                self.items.push(DisplayItem::Rect {
                    x: col_x[k],
                    y: row_top,
                    w: widths[k],
                    h: row_bottom - row_top,
                    color: bg,
                    radius: 0.0,
                    layer: RectLayer::Under,
                });
            }
        }
        self.y = row_bottom;
    }

    /// 求各列总宽(含内边距):手动限宽列固定;其余列取自然宽,总宽超出可用时按自然宽比例压缩。
    fn solve_widths(&self, t: &Table, ncols: usize, avail_w: f32, pad: f32) -> Vec<f32> {
        let (base, sc) = (self.opts.theme.base_size, self.sc);
        let min_w = 2.0 * pad + 1.0;

        // 各自动列的自然内容宽(表头按加粗测、更宽)。
        let mut natural = vec![0f32; ncols];
        let measure = |cells: &[Cell], bold: bool, natural: &mut [f32]| {
            for (k, cell) in cells.iter().enumerate() {
                if k < ncols {
                    let nw =
                        measure_natural(&self.opts.fonts, &self.opts.theme, &cell.inlines, base, bold, sc);
                    natural[k] = natural[k].max(nw);
                }
            }
        };
        if let Some(h) = &t.header {
            measure(h, true, &mut natural);
        }
        for row in &t.rows {
            measure(row, false, &mut natural);
        }

        let mut widths = vec![0f32; ncols];
        let mut auto = Vec::new();
        let (mut auto_sum, mut fixed_sum) = (0.0f32, 0.0f32);
        for (k, wid) in widths.iter_mut().enumerate() {
            match t.cols.get(k).and_then(|c| c.width) {
                // 非有限限宽值按自适应处理(否则污染总宽 / 越界)。
                Some(Length::Px(p)) if p.is_finite() => {
                    *wid = (p * sc).max(min_w);
                    fixed_sum += *wid;
                }
                Some(Length::Percent(pct)) if pct.is_finite() => {
                    *wid = (avail_w * pct / 100.0).clamp(min_w, avail_w.max(min_w));
                    fixed_sum += *wid;
                }
                _ => {
                    *wid = natural[k] + 2.0 * pad;
                    auto.push(k);
                    auto_sum += *wid;
                }
            }
        }
        let total: f32 = widths.iter().sum();
        if total > avail_w && !auto.is_empty() && auto_sum > 0.0 {
            // 先让自适应列吸收溢出。
            let remaining = (avail_w - fixed_sum).max(min_w * auto.len() as f32);
            for &k in &auto {
                widths[k] = (widths[k] / auto_sum * remaining).max(min_w);
            }
        }
        // 仍超出(固定列 / 百分比之和本就 > 可用宽)→ 整体等比缩回画布,
        // 否则超出部分会画到画布外、内容静默丢失。
        let total: f32 = widths.iter().sum();
        if total > avail_w && total > 0.0 {
            let factor = avail_w / total;
            for wid in widths.iter_mut() {
                *wid = (*wid * factor).max(1.0);
            }
        }
        // expand:列宽合计不足可用宽 → 富余按比例分给自适应列(全固定列则整体等比放大)。
        let total: f32 = widths.iter().sum();
        if t.style.expand && total > 0.0 && total < avail_w {
            if !auto.is_empty() {
                let auto_total: f32 = auto.iter().map(|&k| widths[k]).sum();
                if auto_total > 0.0 {
                    let extra = avail_w - total;
                    for &k in &auto {
                        widths[k] += extra * (widths[k] / auto_total);
                    }
                }
            } else {
                let factor = avail_w / total;
                for wid in widths.iter_mut() {
                    *wid *= factor;
                }
            }
        }
        widths
    }
}

/// 一个列表项的 gutter 标记:任务项 `✓`(强调色)/ `□`(次要色),无序 `•`,有序 `{n}.`。
/// 勾选符用内置字体确定覆盖的 `□`/`✓`(`☐`/`☑` 内置字体没有,裸环境会出 tofu)。
fn marker_inline(list: &List, idx: usize, check: Option<bool>, theme: &Theme) -> Inline {
    let (text, color) = match check {
        Some(true) => ("✓".to_string(), theme.accent),
        Some(false) => ("□".to_string(), theme.muted),
        None => match list.kind {
            ListKind::Unordered => ("•".to_string(), theme.accent),
            ListKind::Ordered => (format!("{}.", list.start as usize + idx), theme.accent),
        },
    };
    Inline::Text { text, style: TextStyle { color: Some(color), ..TextStyle::default() } }
}

/// 解码图片来源为 RGBA。`Named` 从 `images` 映射取字节,`Path` 读盘,`Bytes` 直接用。
fn decode_image(src: &ImageSource, images: &HashMap<String, Vec<u8>>) -> Option<image::RgbaImage> {
    let bytes: std::borrow::Cow<[u8]> = match src {
        ImageSource::Bytes(b) => std::borrow::Cow::Borrowed(b),
        ImageSource::Named(n) => std::borrow::Cow::Borrowed(images.get(n)?.as_slice()),
        ImageSource::Path(p) => std::borrow::Cow::Owned(read_image_file(p)?),
    };
    image::load_from_memory(&bytes).ok().map(|i| i.to_rgba8())
}

/// 安全读图片文件:只认普通文件,且至多读 [`MAX_IMAGE_BYTES`]。
/// `/dev/zero`、FIFO 等非普通或无 EOF 文件被拒,不会无界读爆内存。
fn read_image_file(p: &std::path::Path) -> Option<Vec<u8>> {
    use std::io::Read;
    if !std::fs::metadata(p).ok()?.is_file() {
        return None;
    }
    let mut buf = Vec::new();
    std::fs::File::open(p).ok()?.take(MAX_IMAGE_BYTES).read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// 一个 span 的装饰信息(按 `Attrs::metadata` 下标索引回查)。
struct SpanDeco {
    underline: bool,
    strike: bool,
    highlight: Option<Color>,
    code_bg: Option<Color>,
    ink: Color,
    /// 文字阴影(已折算成物理像素);字形定位时带走。
    shadow: Option<GlyphShadow>,
}

/// 用 cosmic-text 整形一段行内内容,产出定位好的字形、装饰矩形与该块高度(物理像素)。
#[allow(clippy::too_many_arguments)]
fn shape_text(
    fonts: &FontHandle,
    theme: &Theme,
    inlines: &[Inline],
    align: Align,
    base_logical: f32,
    base_bold: bool,
    sc: f32,
    width: f32,
    x_left: f32,
    y_top: f32,
) -> (Vec<PlacedGlyph>, Vec<DisplayItem>, f32) {
    let line_mult = theme.line_height;
    let default_px = safe_px(base_logical * sc);
    // metadata=usize::MAX ⇒ 无装饰(默认 / 换行用)。
    let default_attrs = Attrs::new()
        .family(Family::Name(&theme.font_sans))
        .color(to_cosmic(theme.text))
        .metrics(Metrics::new(default_px, default_px * line_mult))
        .metadata(usize::MAX);

    let (spans, decos) = build_spans(inlines, theme, base_logical, base_bold, sc, &default_attrs);

    fonts.with_system(|fs| {
        let mut buf = Buffer::new(fs, Metrics::new(default_px, default_px * line_mult));
        buf.set_size(Some(width), None);
        buf.set_rich_text(
            spans.iter().map(|(t, a)| (*t, a.clone())),
            &default_attrs,
            Shaping::Advanced,
            Some(align_to_cosmic(align)),
        );
        buf.shape_until_scroll(fs, false);

        let xi = x_left.round() as i32;
        let mut glyphs = Vec::new();
        let mut deco_rects = Vec::new();
        let mut height = 0.0f32;
        for run in buf.layout_runs() {
            for g in run.glyphs {
                let p = g.physical((0.0, 0.0), 1.0);
                let color = g.color_opt.map(from_cosmic).unwrap_or(theme.text);
                glyphs.push(PlacedGlyph {
                    cache_key: p.cache_key,
                    x: xi + p.x,
                    y: (y_top + run.line_y).round() as i32 + p.y,
                    color,
                    shadow: decos.get(g.metadata).and_then(|d| d.shadow),
                });
            }
            collect_decos(&run, &decos, x_left, y_top, &mut deco_rects);
            height = height.max(run.line_top + run.line_height);
        }
        (glyphs, deco_rects, height)
    })
}

/// 把行内序列构建成 cosmic-text 富文本跨段 + 各段装饰信息。shape_text 与自然宽测量共用。
#[allow(clippy::type_complexity)]
fn build_spans<'a>(
    inlines: &'a [Inline],
    theme: &'a Theme,
    base_logical: f32,
    base_bold: bool,
    sc: f32,
    default_attrs: &Attrs<'a>,
) -> (Vec<(&'a str, Attrs<'a>)>, Vec<SpanDeco>) {
    let line_mult = theme.line_height;
    let mut spans: Vec<(&str, Attrs)> = Vec::new();
    let mut decos: Vec<SpanDeco> = Vec::new();
    for inline in inlines {
        match inline {
            Inline::Text { text, style } => {
                let idx = decos.len();
                let px = safe_px(base_logical * style.size * sc);
                // 链接无显式色时用主题强调色。
                let fallback = if style.link { theme.accent } else { theme.text };
                let ink = style.color.unwrap_or(fallback);
                let mut a = Attrs::new()
                    .family(family_of(&style.font, theme))
                    .color(to_cosmic(ink))
                    .metrics(Metrics::new(px, px * line_mult))
                    .metadata(idx);
                let mut weight = match style.weight {
                    Some(w) => Weight(w),
                    None if base_bold => Weight::BOLD,
                    None => Weight::NORMAL,
                };
                // 默认楷体字族(霞鹜文楷)只有 300/400/500 三档,而 cosmic-text 对族内
                // 没有的字重会跨族借字(比如 700 借到黑体 Bold),楷体语境就丢了楷体——
                // 故夹到族内范围:细 → 300,粗 → 500(族内最重,当楷体的「粗」)。
                if matches!(style.font, FontRole::Kai) {
                    weight = Weight(weight.0.clamp(300, 500));
                }
                a = a.weight(weight);
                if style.italic {
                    a = a.style(Style::Italic);
                }
                spans.push((text, a));
                decos.push(SpanDeco {
                    underline: style.underline,
                    strike: style.strike,
                    highlight: resolve_highlight(style.highlight, theme),
                    code_bg: None,
                    ink,
                    shadow: style.shadow.map(|s| GlyphShadow {
                        dx: (s.dx * sc).round() as i32,
                        dy: (s.dy * sc).round() as i32,
                        blur: (s.blur * sc).max(0.0),
                        color: s.color,
                    }),
                });
            }
            Inline::Code(s) => {
                let idx = decos.len();
                let px = safe_px(base_logical * sc);
                let mut a = Attrs::new()
                    .family(Family::Name(&theme.font_mono))
                    .color(to_cosmic(theme.code_text))
                    .metrics(Metrics::new(px, px * line_mult))
                    .metadata(idx);
                if base_bold {
                    a = a.weight(Weight::BOLD);
                }
                spans.push((s, a));
                decos.push(SpanDeco {
                    underline: false,
                    strike: false,
                    highlight: None,
                    code_bg: Some(theme.code_bg),
                    ink: theme.code_text,
                    shadow: None,
                });
            }
            Inline::LineBreak => spans.push(("\n", default_attrs.clone())),
        }
    }
    (spans, decos)
}

/// 测一段行内内容的「自然宽」(不换行时的最长行宽,物理像素),给表格列宽求解用。
fn measure_natural(
    fonts: &FontHandle,
    theme: &Theme,
    inlines: &[Inline],
    base_logical: f32,
    base_bold: bool,
    sc: f32,
) -> f32 {
    let px = safe_px(base_logical * sc);
    let line_mult = theme.line_height;
    let default_attrs = Attrs::new()
        .family(Family::Name(&theme.font_sans))
        .metrics(Metrics::new(px, px * line_mult))
        .metadata(usize::MAX);
    let (spans, _) = build_spans(inlines, theme, base_logical, base_bold, sc, &default_attrs);
    fonts.with_system(|fs| {
        let mut buf = Buffer::new(fs, Metrics::new(px, px * line_mult));
        buf.set_size(None, None); // 不限宽 → 不换行
        buf.set_rich_text(
            spans.iter().map(|(t, a)| (*t, a.clone())),
            &default_attrs,
            Shaping::Advanced,
            None,
        );
        buf.shape_until_scroll(fs, false);
        buf.layout_runs().map(|r| r.line_w).fold(0.0, f32::max)
    })
}

/// 求覆盖件在图面内的停靠坐标:`frame` 是图面矩形 `(x, y, w, h)`,`size` 是覆盖件
/// `(宽, 高)`,`m` 是离边距。图面放不下时夹回左上,贴边呈现。
fn anchor_pos(
    a: crate::model::Anchor,
    frame: (f32, f32, f32, f32),
    size: (f32, f32),
    m: f32,
) -> (f32, f32) {
    use crate::model::Anchor;
    let (ix, iy, dw, dh) = frame;
    let (w, h) = size;
    let x = match a {
        Anchor::TopLeft | Anchor::BottomLeft => ix + m,
        Anchor::TopRight | Anchor::BottomRight => ix + dw - w - m,
        Anchor::Center => ix + (dw - w) / 2.0,
    };
    let y = match a {
        Anchor::TopLeft | Anchor::TopRight => iy + m,
        Anchor::BottomLeft | Anchor::BottomRight => iy + dh - h - m,
        Anchor::Center => iy + (dh - h) / 2.0,
    };
    (x.max(ix), y.max(iy))
}

/// 把主题高亮策略解析成具体色。
fn resolve_highlight(h: Option<crate::model::Highlight>, theme: &Theme) -> Option<Color> {
    use crate::model::Highlight;
    match h {
        Some(Highlight::Theme) => Some(theme.highlight),
        Some(Highlight::Custom(c)) => Some(c),
        None => None,
    }
}

/// 在一行内,把连续同 metadata 的字形归组,据其装饰信息产出装饰矩形。
fn collect_decos(
    run: &cosmic_text::LayoutRun,
    decos: &[SpanDeco],
    x_left: f32,
    y_top: f32,
    out: &mut Vec<DisplayItem>,
) {
    let glyphs = run.glyphs;
    let baseline = y_top + run.line_y;
    let line_top = y_top + run.line_top;
    let line_h = run.line_height;
    let mut i = 0;
    while i < glyphs.len() {
        let m = glyphs[i].metadata;
        let (mut x0, mut x1, mut fs) = (f32::MAX, f32::MIN, glyphs[i].font_size);
        let mut j = i;
        while j < glyphs.len() && glyphs[j].metadata == m {
            let g = &glyphs[j];
            x0 = x0.min(g.x);
            x1 = x1.max(g.x + g.w);
            fs = g.font_size;
            j += 1;
        }
        i = j;
        let Some(d) = decos.get(m) else { continue };
        let ax = x_left + x0;
        let aw = (x1 - x0).max(0.0);
        if aw <= 0.0 {
            continue;
        }
        if let Some(c) = d.highlight {
            out.push(DisplayItem::Rect {
                x: ax - fs * 0.06,
                y: line_top,
                w: aw + fs * 0.12,
                h: line_h,
                color: c,
                radius: fs * 0.12,
                layer: RectLayer::Under,
            });
        }
        if let Some(c) = d.code_bg {
            out.push(DisplayItem::Rect {
                x: ax - fs * 0.18,
                y: line_top + line_h * 0.08,
                w: aw + fs * 0.36,
                h: line_h * 0.84,
                color: c,
                radius: fs * 0.22,
                layer: RectLayer::Under,
            });
        }
        if d.underline {
            out.push(DisplayItem::Rect {
                x: ax,
                y: baseline + fs * 0.12,
                w: aw,
                h: (fs * 0.06).max(1.0),
                color: d.ink,
                radius: 0.0,
                layer: RectLayer::Over,
            });
        }
        if d.strike {
            out.push(DisplayItem::Rect {
                x: ax,
                y: baseline - fs * 0.28,
                w: aw,
                h: (fs * 0.06).max(1.0),
                color: d.ink,
                radius: 0.0,
                layer: RectLayer::Over,
            });
        }
    }
}

fn family_of<'a>(role: &'a FontRole, theme: &'a Theme) -> Family<'a> {
    match role {
        FontRole::Sans => Family::Name(&theme.font_sans),
        FontRole::Serif => Family::Name(&theme.font_serif),
        FontRole::Mono => Family::Name(&theme.font_mono),
        FontRole::Kai => Family::Name(&theme.font_kai),
        FontRole::Named(s) => Family::Name(s),
    }
}

fn align_to_cosmic(a: Align) -> cosmic_text::Align {
    match a {
        Align::Left => cosmic_text::Align::Left,
        Align::Center => cosmic_text::Align::Center,
        Align::Right => cosmic_text::Align::Right,
        Align::Justify => cosmic_text::Align::Justified,
    }
}

fn to_cosmic(c: Color) -> cosmic_text::Color {
    cosmic_text::Color::rgba(c.r, c.g, c.b, c.a)
}

fn from_cosmic(c: cosmic_text::Color) -> Color {
    Color::rgba(c.r(), c.g(), c.b(), c.a())
}

/// 水平规线(以 `y` 为中线)。
fn hrule(x: f32, y: f32, w: f32, line: f32, color: Color) -> DisplayItem {
    DisplayItem::Rect { x, y: y - line / 2.0, w, h: line, color, radius: 0.0, layer: RectLayer::Under }
}

/// 竖直规线(以 `vx` 为中线,从 `top` 到 `bottom`)。
fn vrule(vx: f32, top: f32, bottom: f32, line: f32, color: Color) -> DisplayItem {
    DisplayItem::Rect {
        x: vx - line / 2.0,
        y: top,
        w: line,
        h: bottom - top,
        color,
        radius: 0.0,
        layer: RectLayer::Under,
    }
}
