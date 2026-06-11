# nagisa-render

[![crates.io](https://img.shields.io/crates/v/nagisa-render?style=flat-square&logo=rust&color=e37933)](https://crates.io/crates/nagisa-render)
[![docs.rs](https://img.shields.io/docsrs/nagisa-render?style=flat-square&logo=docsdotrs)](https://docs.rs/nagisa-render)
[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue?style=flat-square)](#license)

把一段类 Markdown 的标记文本,或一份 Rust 构建器拼出的文档,排版渲染成图片字节(PNG / WebP)。纯 CPU 管线:cosmic-text 整形(断行 / 字体回退 / CJK + 拉丁 + emoji 混排)+ tiny-skia 光栅,不依赖浏览器、GPU 或系统字体,内置中文字体开箱出图。是 [nagisa](https://github.com/djkcyl/nagisa) 框架的排版引擎,也可单独使用。

<div align="center">
<img src="https://raw.githubusercontent.com/djkcyl/nagisa/master/render-showcase.webp" alt="全功能样张" width="640">

*全功能样张,由 `examples/gallery.rs` 渲出*
</div>

## 安装

bot 场景经 nagisa 门面(挂在 `render` feature 后面):

```toml
nagisa = { version = "0.5", features = ["render"] }
```

```rust,ignore
use nagisa::render::*;
```

单独使用:`cargo add nagisa-render`。

## 两种写法

标记文本(类 Markdown,适合「一大段文字」):

```rust,ignore
use nagisa_render::{render_markup, RenderOptions};

let png = render_markup("# 标题\n\n正文 **加粗**、[彩色]{color=#e00}、==高亮==。", &RenderOptions::default())?;
```

Rust 构建器(类型安全,适合从数据生成卡片):

```rust,ignore
use nagisa_render::{render_document, Doc, RenderOptions};

let doc = Doc::new()
    .heading(1, |h| h.text("月度报告"))
    .paragraph(|p| { p.text("环比 ").bold("+12%").text("。"); })
    .table(|t| { t.head(["项", "值"]).row(["发言", "3450"]); })
    .build();
let png = render_document(&doc, &RenderOptions::default())?;
```

两种写法产出同一个 `Document`;`parse_markup` 拿到 `Document` 后还能往 `blocks` 里接着拼构建器块(上面的样张就是这么混出来的)。构建器闭包写花括号块(`|p| { p.text(..); }`),单表达式链式返回 `&mut` 会撞闭包生命周期。

## 能排什么

- **块级**:标题(1–6 级)、段落、有序 / 无序列表(可嵌套、起始序号可设)、任务列表(`✓` / `□`)、引用(可嵌套)、行内与块级代码(语言标签渲在盒角)、分割线、表格、多栏并排(按权重分宽)、面板(底色 / 边框 / 圆角 / 内边距 / 投影的卡片容器,作并排栏整栏时自动拉齐行高)、块级图、进度条。
- **行内**:粗 / 细 / 任意字重(100–900 真字形)、斜体、下划线、删除线、文字色、高亮 / 自定底色、字号倍率、字族切换(黑 / 宋 / 楷 / 等宽 / 具名)、链接(取文字按强调色渲染)、圈注与着重点(整段或逐字,可定径定色)、边注(挂行外,不挤布局)、文字阴影、硬换行、反斜杠转义。
- **表格**:自适应列宽、按列限宽、铺满可用宽(`expand`)、各列对齐、按列 / 行 / 格设文字样式与背景色、紧凑度(`pad_x` / `pad_y`)与网格线(外框 / 横 / 竖)可调、表头浅底开关。
- **图片**:PNG / JPEG / WebP / GIF 解码,缩放(像素 / 百分比)、对齐、富文本图注,外加装饰层——圆角裁切、边框、投影、角标、水印(画在图面上,不改布局)。

## 标记语法速查

Markdown 基底加少量扩展。解析宽容:认不出的写法退化成普通文字。

| 写法 | 效果 |
|:--|:--|
| `# …`–`###### …` | 标题 1–6 级;块尾 `{align=center}` 设对齐 |
| `**粗**` `*斜*` `***粗斜***` `~~删~~` `` `码` `` `==高亮==` | 行内基础样式,可嵌套 |
| `[文字](URL)` | 链接(取文字按强调色渲染,URL 不展示) |
| `[文字]{属性}` | 属性 span,见下表 |
| `- ` / `9. ` / `- [x]` | 无序 / 有序(起始序号取首项)/ 任务列表,缩进嵌套 |
| `> ` | 引用,`>>` 嵌套 |
| ```` ```lang ```` | 代码块 |
| `---` | 分割线(`***` / `___` 同,3 个起步) |
| GFM 表格 | 分隔行 `\|:--\|:-:\|--:\|` 定列对齐 |
| `![图注](路径)` / `![图注](@名字)` | 块级图;`@名字` 从 `RenderOptions::images` 取字节 |
| `::: center` … `:::` | 对齐围栏(left / right / center / justify),可嵌套 |
| `::: columns` 内嵌 `::: col 权重` | 多栏围栏;`::: col 权重 {bg=…}` = 卡片栏(自动等高) |
| `::: panel {bg=… border=… rounded=… pad=… shadow}` | 面板(卡片);属性全缺省即主题默认卡片样 |
| 行尾 `\` | 硬换行 |

属性 span 的键(逗号或空白分隔):

| 属性 | 含义 |
|:--|:--|
| `color=#hex` / `bg=#hex` | 文字色 / 底色 |
| `bold` / `light` / `weight=500` | 字重 |
| `italic` / `underline` / `strike` | 斜 / 下划 / 删除 |
| `size=1.2` | 字号倍率 |
| `font=sans\|serif\|kai\|mono\|字族名` | 字族 |
| `ring` / `ring=#hex` / `ring-radius` / `ring-rx` / `ring-ry` / `ring-stroke` / `ring-each` | 圈注:自适应或定径,整段一圈或逐字 |
| `dot` / `dot=#hex` / `dot-radius` / `dot-each` | 着重点:整段一点或正字法逐字 |
| `aside` / `aside=left` | 边注:挂行外,不参与布局 |

## 输出与配置

都在 `RenderOptions`(默认:逻辑宽 960、亮色主题、scale 2、PNG):

- **格式**:`.png()`(默认,通用)/ `.png_fast()`(约 8 倍快、体积大 ~40%)/ `.webp()`(无损,文字图体积最小且快;单边 > 16383px 报错)/ `.webp_or_png()`(WebP 优先,超限自动落 PNG)。文字图建议 WebP。
- **清晰度**:`.fast()`(1×)/ `.standard()`(1.5×)/ `.sharp()`(2×,默认)/ `.ultra()`(3×),或 `.with_scale(f)`。所有尺寸都是逻辑值,输出 = 逻辑尺寸 × scale。
- **主题** `Theme`:亮 / 暗预设(`Theme::light()` / `Theme::dark()`)+ 全部配色 / 字族名 / 字号 / 行高 / 标题阶梯可改。
- **页眉 / 页脚** `PageChrome`:与文档无关的固定标识(品牌 / 署名 / 出处),配在选项上所有出图统一带;富文本、左右分栏(`trailing`)、页脚满幅色带(`band`)皆可。
- **具名图** `images`:标记文本里 `@名字` 的字节来源。

入口四个:

| 函数 | 产出 |
|:--|:--|
| `render_markup` | 标记文本 → 图片字节 |
| `render_document` | `Document` → 图片字节 |
| `render_to_rgba` | `Document` → RGBA 图,供进一步合成 |
| `measure_document` | 只排版不绘制,返回物理像素尺寸——按高度上限切多图时先量再渲 |

## 字体

内置兜底 Noto Sans SC + JetBrains Mono 正斜(zstd 压缩内嵌,首次渲染时解压),保证开箱出中文、粗细是真字形。衬线 / 楷体两个角色不随包内置(crates.io 包体上限),字族名默认指 Noto Serif SC 与 LXGW WenKai GB,自备对应字体即生效,缺则回退黑体:

```rust,ignore
let fonts = FontHandle::builder()
    .bundled()                                   // 内置兜底(默认开)
    .data(include_bytes!("NotoSerifSC.ttf.zst")) // 自备数据,裸字节或 zstd 压缩皆可
    .dir("assets/fonts")                         // 字体目录
    .system()                                    // 系统字体
    .build()?;
let opts = RenderOptions::default().with_fonts(fonts);
```

`FontHandle` 构建一次很贵,`Arc` 共享,建好复用。CJK 没有斜体字面,`italic` 由仿斜(错切)合成;等宽拉丁有真斜体,优先命中。

emoji 表现序列(含 VS16 升级、肤色修饰、ZWJ 合字、键帽、旗帜)统一切到彩色 emoji 字族(`Theme::font_emoji`,默认 Noto Color Emoji)——黑体自带的单色 emoji 字面不会抢跑;该字体不内置,系统装有即用,缺则回退单色。

## 样张

`examples/gallery.rs` 按特性渲样张到 `out/*.png`:

```sh
cargo run -p nagisa-render --example gallery
# 衬线 / 楷体样张要真字形的话,指一个放字体的目录:
GALLERY_FONTS=path/to/fonts cargo run -p nagisa-render --example gallery
```

## License

MIT OR Apache-2.0
