//! 字体栈 —— 把内置兜底 / 自定义目录 / 系统字体合进一个 cosmic-text `FontSystem`,并缓存
//! `SwashCache`(字形栅格化)。构建一次很贵,故用 `FontHandle`(`Arc` 共享)复用。
//!
//! 内置一套兜底:Noto Sans SC + JetBrains Mono 正斜两份(等宽拉丁;CJK 在等宽语境靠回退
//! 到 Noto)——都是可变字体,含全字重命名实例(粗 / 细体即由此而来)。保证「开箱出中文 +
//! 真字重」。衬线 / 楷体两个角色**不带字体**(crates.io 包体上限装不下),字族名默认指
//! Noto Serif SC(思源宋体)与 LXGW WenKai GB(霞鹜文楷),由使用方经 [`FontStackBuilder::data`] /
//! [`FontStackBuilder::dir`] / 系统字体提供;缺字体时这两个角色回退黑体。
//! CJK 没有斜体字面(思源系列不发行斜体),`italic` 时由 cosmic-text 仿斜(错切)合成;
//! 等宽拉丁有真斜体字面,优先命中。
//!
//! 内置字体以 zstd 压缩内嵌(`include_bytes!` 的是 `.ttf.zst`),构建字体栈时解压——
//! `shared_default` 懒加载,解压只在首次渲染前发生一次。[`FontStackBuilder::data`] 同样
//! 接受 zstd 压缩字节(按魔数识别),使用方可以用同一招内嵌自己的字体。

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use cosmic_text::{fontdb, FontSystem, SwashCache};

use crate::error::{Error, Result};

/// 内置兜底字体数据(zstd 压缩)。
const BUNDLED: &[&[u8]] = &[
    include_bytes!("../assets/fonts/NotoSansSC.ttf.zst"),
    include_bytes!("../assets/fonts/JetBrainsMono.ttf.zst"),
    include_bytes!("../assets/fonts/JetBrainsMono-Italic.ttf.zst"),
];

/// zstd 帧魔数(RFC 8878),`data()` 靠它识别压缩字节。
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

/// 可克隆的共享字体句柄:内部持有加好字体的 `FontSystem` 与一个 `SwashCache`,各用 `Mutex`
/// 包(cosmic-text 整形 / 取字形位图都要 `&mut`)。克隆只增引用计数。
#[derive(Clone)]
pub struct FontHandle(Arc<Inner>);

struct Inner {
    system: Mutex<FontSystem>,
    cache: Mutex<SwashCache>,
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
                FontHandle::builder()
                    .bundled()
                    .system()
                    .build()
                    .unwrap_or_else(|_| FontHandle::from_db(fontdb::Database::new()))
            })
            .clone()
    }

    fn from_db(db: fontdb::Database) -> FontHandle {
        let system = FontSystem::new_with_locale_and_db("zh-CN".to_string(), db);
        FontHandle(Arc::new(Inner {
            system: Mutex::new(system),
            cache: Mutex::new(SwashCache::new()),
        }))
    }

    /// 借出 `FontSystem`(整形需 `&mut`)。
    ///
    /// 锁中毒(某次渲染在持锁时 panic)后仍照常借出内层数据——`FontSystem` 没有跨调用不变量,
    /// 一次坏输入不应永久毒死整条渲染链(长驻 bot 致命)。
    pub(crate) fn with_system<R>(&self, f: impl FnOnce(&mut FontSystem) -> R) -> R {
        let mut sys = self.0.system.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut sys)
    }

    /// 借出 `SwashCache` + `FontSystem`(取字形位图需二者)。锁序固定:先 cache 后 system。
    /// 同样容忍中毒锁(理由见 [`Self::with_system`])。
    pub(crate) fn with_cache<R>(&self, f: impl FnOnce(&mut SwashCache, &mut FontSystem) -> R) -> R {
        let mut cache = self.0.cache.lock().unwrap_or_else(|e| e.into_inner());
        let mut sys = self.0.system.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut cache, &mut sys)
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
        let mut db = fontdb::Database::new();
        if self.bundled {
            for z in BUNDLED {
                db.load_font_data(unzstd(z)?);
            }
        }
        for d in self.datas {
            if d.starts_with(&ZSTD_MAGIC) {
                db.load_font_data(unzstd(&d)?);
            } else {
                db.load_font_data(d);
            }
        }
        for d in &self.dirs {
            db.load_fonts_dir(d);
        }
        if self.system {
            db.load_system_fonts();
        }
        if db.is_empty() {
            return Err(Error::FontLoad("字体栈为空(未启用任何字体来源)".into()));
        }
        Ok(FontHandle::from_db(db))
    }
}

/// 解压一只 zstd 压缩的内置字体。
fn unzstd(data: &[u8]) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut dec = ruzstd::decoding::StreamingDecoder::new(data)
        .map_err(|e| Error::FontLoad(format!("内置字体解压失败:{e}")))?;
    let mut out = Vec::new();
    dec.read_to_end(&mut out).map_err(|e| Error::FontLoad(format!("内置字体解压失败:{e}")))?;
    Ok(out)
}

