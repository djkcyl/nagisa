//! 字体栈 —— 把内置兜底 / 自定义目录 / 系统字体注册进一个 fontique `Collection`(经 parley
//! `FontContext` 暴露),配一个 parley `LayoutContext`(整形)与 swash `ScaleContext` + 字形
//! 位图缓存(栅格化)。构建一次很贵,故用 `FontHandle`(`Arc` 共享)复用。
//!
//! 内置一套兜底:Noto Sans SC + JetBrains Mono 正斜两份(等宽拉丁;CJK 在等宽语境靠回退
//! 到 Noto)——都是可变字体,含全字重命名实例(粗 / 细体即由此而来)。保证「开箱出中文 +
//! 真字重」。衬线 / 楷体两个角色**不带字体**(crates.io 包体上限装不下),字族名默认指
//! Noto Serif SC(思源宋体)与 LXGW WenKai GB(霞鹜文楷),由使用方经 [`FontStackBuilder::data`] /
//! [`FontStackBuilder::dir`] / 系统字体提供;缺字体时这两个角色回退黑体。
//! CJK 没有斜体字面(思源系列不发行斜体),`italic` 时由 fontique 合成仿斜(错切),
//! 栅格时按 [`Synthesis`](parley::fontique::Synthesis) 的角度施加;等宽拉丁有真斜体字面,优先命中。
//!
//! 内置字体以 zstd 压缩内嵌(`include_bytes!` 的是 `.ttf.zst`),构建字体栈时解压——
//! `shared_default` 懒加载,解压只在首次渲染前发生一次。[`FontStackBuilder::data`] 同样
//! 接受 zstd 压缩字节(按魔数识别),使用方可以用同一招内嵌自己的字体。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use parley::fontique::{Blob, Collection, CollectionOptions};
use parley::{FontContext, LayoutContext};
use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::zeno::{Angle, Format, Transform};

use crate::error::{Error, Result};
use crate::layout::{GlyphFont, Ink};

/// 内置兜底字体数据(zstd 压缩)。
const BUNDLED: &[&[u8]] = &[
    include_bytes!("../assets/fonts/NotoSansSC.ttf.zst"),
    include_bytes!("../assets/fonts/JetBrainsMono.ttf.zst"),
    include_bytes!("../assets/fonts/JetBrainsMono-Italic.ttf.zst"),
];

/// zstd 帧魔数(RFC 8878),`data()` 靠它识别压缩字节。
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

/// 可克隆的共享字体句柄:内部持有注册好字体的 parley `FontContext`、复用的 `LayoutContext`
/// 与字形栅格(`ScaleContext` + 位图缓存),各用 `Mutex` 包(整形 / 栅格都要 `&mut`)。
/// 克隆只增引用计数。
#[derive(Clone)]
pub struct FontHandle(Arc<Inner>);

struct Inner {
    fonts: Mutex<FontContext>,
    layouts: Mutex<LayoutContext<Ink>>,
    raster: Mutex<RasterState>,
}

/// 栅格态:swash 缩放上下文 + 已栅格字形位图缓存(键含字体 / 字号 / 变量轴 / 合成参数)。
struct RasterState {
    scale: ScaleContext,
    cache: HashMap<GlyphKey, Option<GlyphImage>>,
}

/// 字形位图缓存键。`coords` 是可变字体归一化轴值(字重由此体现),`skew` / `embolden`
/// 是 fontique 给的合成参数(仿斜 / 假粗),都影响像素,都进键。
#[derive(Clone, PartialEq, Eq, Hash)]
struct GlyphKey {
    blob: u64,
    index: u32,
    glyph: u16,
    size: u32,
    coords: Vec<i16>,
    skew: u32,
    embolden: bool,
}

/// 栅格化后的字形位图(`left`/`top` 为相对笔位的摆放偏移,swash 口径)。
pub(crate) struct GlyphImage {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    /// true = RGBA 彩色位图(emoji),false = 单通道覆盖率蒙版。
    pub color: bool,
    pub data: Vec<u8>,
}

/// 借给 `paint` 的字形栅格器:按需栅格并缓存。
pub(crate) struct GlyphRaster<'a>(&'a mut RasterState);

impl GlyphRaster<'_> {
    /// 取一个字形的位图(无字形 / 栅格失败返回 `None`,结果含失败也缓存)。
    pub(crate) fn image(&mut self, gf: &GlyphFont, glyph: u16) -> Option<&GlyphImage> {
        let key = GlyphKey {
            blob: gf.font.data.id(),
            index: gf.font.index,
            glyph,
            size: gf.size.to_bits(),
            coords: gf.coords.clone(),
            skew: gf.skew.map_or(0, f32::to_bits),
            embolden: gf.embolden,
        };
        if !self.0.cache.contains_key(&key) {
            let img = raster(&mut self.0.scale, gf, glyph);
            self.0.cache.insert(key.clone(), img);
        }
        self.0.cache.get(&key).and_then(|o| o.as_ref())
    }
}

/// 栅格化一个字形:彩色轮廓 / 彩色位图(emoji)优先,退普通轮廓;不开 hinting(本引擎
/// 默认 2× 超采样,hinting 无益,且 swash 0.2 的 hint 实例缓存不按字号失效,开了会在
/// 共享句柄连续渲染 / 同文档多字号时按「上一次的字号」栅格化——别打开)。
fn raster(cx: &mut ScaleContext, gf: &GlyphFont, glyph: u16) -> Option<GlyphImage> {
    let font_ref = swash::FontRef::from_index(gf.font.data.as_ref(), gf.font.index as usize)?;
    let mut scaler = cx.builder(font_ref).size(gf.size).hint(false).normalized_coords(&gf.coords).build();
    let mut render = Render::new(&[Source::ColorOutline(0), Source::ColorBitmap(StrikeWith::BestFit), Source::Outline]);
    render.format(Format::Alpha);
    if let Some(deg) = gf.skew {
        render.transform(Some(Transform::skew(Angle::from_degrees(deg), Angle::ZERO)));
    }
    if gf.embolden {
        render.embolden(gf.size * 0.02);
    }
    let img = render.render(&mut scaler, glyph)?;
    let (width, height) = (img.placement.width, img.placement.height);
    let (color, data) = match img.content {
        Content::Color => (true, img.data),
        Content::Mask => (false, img.data),
        // Format::Alpha 不会产出子像素蒙版;防御性平均成普通蒙版。
        Content::SubpixelMask => {
            (false, img.data.chunks_exact(3).map(|c| ((c[0] as u16 + c[1] as u16 + c[2] as u16) / 3) as u8).collect())
        }
    };
    Some(GlyphImage { left: img.placement.left, top: img.placement.top, width, height, color, data })
}

impl FontHandle {
    /// 起一个字体栈构建器(默认含内置兜底)。
    pub fn builder() -> FontStackBuilder {
        FontStackBuilder::new()
    }

    /// 全局懒加载默认句柄(内置兜底 + 系统字体),`RenderOptions::default()` 用它。
    pub fn shared_default() -> FontHandle {
        static DEFAULT: OnceLock<FontHandle> = OnceLock::new();
        DEFAULT
            .get_or_init(|| {
                FontHandle::builder().bundled().system().build().unwrap_or_else(|_| {
                    FontHandle::from_collection(Collection::new(CollectionOptions {
                        shared: false,
                        system_fonts: true,
                    }))
                })
            })
            .clone()
    }

    fn from_collection(collection: Collection) -> FontHandle {
        let fonts = FontContext { collection, source_cache: Default::default() };
        FontHandle(Arc::new(Inner {
            fonts: Mutex::new(fonts),
            layouts: Mutex::new(LayoutContext::new()),
            raster: Mutex::new(RasterState { scale: ScaleContext::new(), cache: HashMap::new() }),
        }))
    }

    /// 借出整形所需的一对上下文(parley 的 `ranged_builder` 同时要二者)。锁序固定:
    /// 先 fonts 后 layouts。
    ///
    /// 锁中毒(某次渲染在持锁时 panic)后仍照常借出内层数据——上下文没有跨调用不变量,
    /// 一次坏输入不应永久毒死整条渲染链(长驻 bot 致命)。
    pub(crate) fn with_layout<R>(&self, f: impl FnOnce(&mut FontContext, &mut LayoutContext<Ink>) -> R) -> R {
        let mut fonts = self.0.fonts.lock().unwrap_or_else(|e| e.into_inner());
        let mut layouts = self.0.layouts.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut fonts, &mut layouts)
    }

    /// 借出字形栅格器(取字形位图)。同样容忍中毒锁(理由见 [`Self::with_layout`])。
    pub(crate) fn with_raster<R>(&self, f: impl FnOnce(&mut GlyphRaster) -> R) -> R {
        let mut raster = self.0.raster.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut GlyphRaster(&mut raster))
    }
}

/// 字体栈构建器:`builder().bundled().data(BYTES).dir("fonts").system().build()`。
pub struct FontStackBuilder {
    bundled: bool,
    datas: Vec<Vec<u8>>,
    dirs: Vec<PathBuf>,
    system: bool,
}

impl FontStackBuilder {
    fn new() -> Self {
        Self { bundled: true, datas: Vec::new(), dirs: Vec::new(), system: false }
    }

    /// 加入内置兜底字体(默认开)。
    pub fn bundled(mut self) -> Self {
        self.bundled = true;
        self
    }

    /// 不加入内置兜底字体。
    pub fn no_bundled(mut self) -> Self {
        self.bundled = false;
        self
    }

    /// 加入一份字体数据(可多次)。接受裸字体字节,也接受 zstd 压缩字节(按魔数识别,
    /// 构建时解压)——使用方可以像内置字体一样 `include_bytes!` 压缩资产再喂进来。
    pub fn data(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.datas.push(bytes.into());
        self
    }

    /// 加入一个字体目录(可多次)。
    pub fn dir(mut self, p: impl Into<PathBuf>) -> Self {
        self.dirs.push(p.into());
        self
    }

    /// 加入系统字体。
    pub fn system(mut self) -> Self {
        self.system = true;
        self
    }

    /// 构建字体句柄。字体栈为空则报 [`Error::FontLoad`]。
    pub fn build(self) -> Result<FontHandle> {
        let mut collection = Collection::new(CollectionOptions { shared: false, system_fonts: self.system });
        let mut registered = false;
        let mut register = |collection: &mut Collection, raw: Vec<u8>| {
            registered |= !collection.register_fonts(Blob::from(raw), None).is_empty();
        };
        if self.bundled {
            for z in BUNDLED {
                register(&mut collection, unzstd(z)?);
            }
        }
        for d in self.datas {
            if d.starts_with(&ZSTD_MAGIC) {
                register(&mut collection, unzstd(&d)?);
            } else {
                register(&mut collection, d);
            }
        }
        // 目录递归遍历(与 fontdb 的 load_fonts_dir 口径一致);坏文件 / 不可读项静默跳过。
        let mut stack: Vec<PathBuf> = self.dirs.clone();
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let is_font = p
                    .extension()
                    .and_then(|x| x.to_str())
                    .is_some_and(|x| matches!(x.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc" | "otc"));
                if is_font {
                    if let Ok(raw) = std::fs::read(&p) {
                        register(&mut collection, raw);
                    }
                }
            }
        }
        if !registered && !self.system {
            return Err(Error::FontLoad("字体栈为空(未启用任何字体来源)".into()));
        }
        Ok(FontHandle::from_collection(collection))
    }
}

/// 解压一只 zstd 压缩的内置字体。
fn unzstd(data: &[u8]) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut dec =
        ruzstd::decoding::StreamingDecoder::new(data).map_err(|e| Error::FontLoad(format!("内置字体解压失败:{e}")))?;
    let mut out = Vec::new();
    dec.read_to_end(&mut out).map_err(|e| Error::FontLoad(format!("内置字体解压失败:{e}")))?;
    Ok(out)
}
