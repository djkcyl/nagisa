//! 全功能样张 —— 一图覆盖全部能力,人工看图核对(本项目无常驻单测)。
//! 跑:`cargo run -p nagisa-render --example gallery`,出 `out/full.png`(亮)、
//! `out/full-dark.png`(暗)与 `out/full.webp`(亮,README 样张由此而来)。
//!
//! 衬线 / 楷体不随框架内置(包体上限),裸检出里这两类内容会回退黑体;设
//! `GALLERY_FONTS=<目录>` 指向存放 `.ttf`/`.ttf.zst` 的目录(比如 abot 的
//! `assets/fonts`)可补全。

use image::{ImageBuffer, Rgba};
use nagisa_render::{
    parse_markup, render_document, Align, Anchor, Doc, Document, FontHandle, Length, PageChrome, RenderOptions, Theme,
};
use std::fs;

fn main() {
    fs::create_dir_all("out").expect("建 out 目录");
    let doc = full_doc();
    write("out/full.png", &doc, options(Theme::light()));
    write("out/full-dark.png", &doc, options(Theme::dark()));
    write("out/full.webp", &doc, options(Theme::light()).webp_or_png());
}

/// 渲一份、写盘、报字节数。
fn write(path: &str, doc: &Document, opts: RenderOptions) {
    let bytes = render_document(doc, &opts).expect("渲染全功能样张");
    fs::write(path, &bytes).expect("写文件");
    println!("wrote {path} ({} bytes)", bytes.len());
}

// ── 文档 ──────────────────────────────────────────────────────────────────────

/// 全功能样张:标记文本前半(标记语法能力)+ 构建器后半(构建器独有能力)+
/// 标记文本收尾,拼成同一份 `Document`。
fn full_doc() -> Document {
    let mut doc = parse_markup(MARKUP_HEAD).expect("解析样张标记文本");
    doc.blocks.extend(builder_half().blocks);
    doc.blocks.extend(parse_markup(MARKUP_TAIL).expect("解析样张标记文本").blocks);
    doc
}

/// 标记文本前半:行内 / 字族 / 圈点边注 / 列表 / 引用 / 多语言代码 / GFM 表格 /
/// 图属性 / 面板与多栏围栏。
const MARKUP_HEAD: &str = r#"# nagisa-render · 全功能样张 {align=center}

::: center
[标记文本与 Rust 构建器,同一份文档模型,排版渲染成图片]{font=kai}
:::

## 行内样式

一行混排:**粗体**、*斜体*、***粗斜***、~~删除~~、[下划线]{underline}、`行内代码`、==高亮==、[自定底色]{bg=#fde047,color=#713f12}、[彩色加粗]{color=#7c3aed,bold}、[字号 1.3×]{size=1.3}、[0.8×]{size=0.8} 与 [链接](https://github.com/djkcyl/nagisa);CJK、English 与 emoji 😄⛏️🤖 自动整形断行,user_id 不会被 `_` 吞掉,转义 \*照常星号\*。

字重任意档:[细 300]{light} · 常规 400 · [Medium 500]{weight=500} · **粗 700** · [Black 900]{weight=900};行尾反斜杠硬换行 \
这是换出来的第二行。

## 字族

正文黑体(Noto Sans SC);[这一句切衬线,思源宋体,**粗体**是真字重;]{font=serif}[这一句楷体,霞鹜文楷;]{font=kai}[mono italic 真斜体字面]{font=mono,italic},CJK 斜体为仿斜。

## 圈注 · 着重点 · 边注

库存 [缺货]{ring=#dc2626},定径正圆 [1]{ring-radius=22} 与 [10]{ring-radius=22} 同大,扁椭圆 [年度目标]{ring-rx=72,ring-ry=24,ring-stroke=2},逐字 [天天圈]{ring-each,ring=#4c63b6};着重点 [这几个字]{dot},正字法逐字 [字字有点]{dot-each,dot=#dc2626}。圈与点画进行距,不动布局。

::: center
[▶]{aside=left,color=#0e9488}这行居中只按正文算,边注挂在行外[当前]{aside,color=#8a8f98,size=0.8}
:::

## 列表

- 无序列表,可嵌套
  - 子项 A
  - 子项 B
- [x] 任务:已完成
- [ ] 任务:待办

9. 有序起步可设
10. 多位数序号右对齐

## 引用 · 代码

> 引用块:强调色竖条,衬淡色引号题饰,内层还能放块。

```rust
/// 题头栏带 `</>` 与语言标签,词按语言上色。
fn main() {
    let nums = vec![1, 2, 0xff];
    println!("sum = {}, ok = {}", nums.iter().sum::<i32>(), true);
}
```

```json
{ "name": "abot", "version": 0.7, "stable": true, "tags": ["bot", "qq"], "extra": null }
```

```python
# 阶乘,递归写法
def fact(n: int) -> int:
    return 1 if n <= 1 else n * fact(n - 1)
```

---

## 表格(标记文本 GFM)

| 前端 | 适合 | 行内能力 |
|:--|:-:|--:|
| 标记文本 | 一大段文字 | 全部 |
| 构建器 | 从数据生成卡片 | 全部 |

## 块级图属性(标记文本)

![40% 宽 · 居中 · 圆角 · 投影](@grad){width=40%, align=center, rounded=16, shadow}

## 面板与多栏(标记文本围栏)

::: panel {bg=#fff7ed, border=#f59e0b, rounded=14}
[卡片围栏:]{color=#9a5b13}`::: panel {bg=… border=… rounded=… pad=… shadow}`
:::

::: columns
::: col {bg=#eef2ff}
[左卡,]{color=#3730a3}`::: col {bg=…}`
:::
::: col 2 {border=#0e9488}
右卡权重 2,`::: col 权重 {属性}`,整栏卡片自动等高。
:::
:::
"#;

/// 标记文本收尾。
const MARKUP_TAIL: &str = r#"---

::: center
[—— 样张完,底部色带是页脚 PageChrome ——]{color=#8a8f98}
:::
"#;

/// 构建器后半:标记文本写不出的能力,按节一个函数。
fn builder_half() -> Document {
    let mut b = Doc::new();
    tables(&mut b);
    image_decor(&mut b);
    columns(&mut b);
    panels(&mut b);
    progress(&mut b);
    text_shadow(&mut b);
    b.build()
}

/// 表格进阶:铺满 / 限宽 / 按列按格上色、富文本格、窄表居中、紧凑度与网格。
fn tables(b: &mut Doc) {
    b.heading(2, |h| {
        h.text("表格(构建器进阶)");
    })
    .table(|t| {
        t.head(["姓名", "积分", "状态", "备注"])
            .align([Align::Left, Align::Right, Align::Center, Align::Left])
            .width(3, Length::Px(180.0))
            .row(["张三", "3450", "正常", "活跃用户,本月发言很多"])
            .row(["李四", "985", "警告", "新人"])
            .row(["王五", "12048", "封禁", "管理员"])
            .expand();
        t.col_style(1, |s| {
            s.bold();
        });
        t.cell_fill(0, 2, "#dcfce7").cell_style(0, 2, |s| {
            s.color("#166534");
        });
        t.cell_fill(1, 2, "#fef9c3").cell_style(1, 2, |s| {
            s.color("#854d0e");
        });
        t.cell_fill(2, 2, "#fee2e2").cell_style(2, 2, |s| {
            s.color("#991b1b");
        });
    })
    .paragraph(|p| {
        p.text("铺满可用宽(expand)、备注列限宽;积分列加粗,状态列按格上色。富文本格与窄表居中:");
    })
    .table(|t| {
        t.head_rich(|r| {
            r.cell(|p| {
                p.bold("项目");
            })
            .cell(|p| {
                p.styled("状态", |st| {
                    st.color("#7c3aed");
                });
            });
        })
        .row_rich(|r| {
            r.text("构建").cell(|p| {
                p.code("cargo build").text(" ").styled("通过", |st| {
                    st.color("#166534").bold();
                });
            });
        })
        .row_rich(|r| {
            r.text("装饰垫底").cell(|p| {
                p.highlight("高亮").text(" 与格底色不打架");
            });
        })
        .cell_fill(1, 1, "#fef9c3")
        .table_align(Align::Center);
    })
    .paragraph(|p| {
        p.text("紧凑度与网格可调:");
    })
    .columns(|c| {
        c.col(|d| {
            d.table(|t| {
                t.head(["默认", "数值"]).row(["第一项", "10"]).row(["第二项", "20"]);
            });
        })
        .col(|d| {
            d.table(|t| {
                t.head(["极简", "数值"])
                    .row(["第一项", "10"])
                    .row(["第二项", "20"])
                    .pad_y(5.0)
                    .no_grid()
                    .header_fill(false);
            });
        });
    });
}

/// 图片装饰层:圆角 / 边框 / 投影 / 角标 / 水印 / 富文本图注。
fn image_decor(b: &mut Doc) {
    b.heading(2, |h| {
        h.text("图片与装饰层");
    })
    .image_bytes(gradient_png(640, 280), |i| {
        i.width_percent(62.0)
            .align(Align::Center)
            .rounded(18.0)
            .border(3.0, "#4c63b6")
            .shadow()
            .badge("角标", |bb| {
                bb.bg("#dc2626e0");
            })
            .watermark("nagisa-render", |w| {
                w.anchor(Anchor::Center).size(1.4);
            })
            .caption_with(|p| {
                p.text("圆角 · 边框 · 投影 · 角标 · 水印,").bold("富文本图注");
            });
    });
}

/// 并排栏:权重分宽,栏内块级元素齐全(图 / 段落 / 进度条)。
fn columns(b: &mut Doc) {
    b.heading(2, |h| {
        h.text("并排栏");
    })
    .columns(|c| {
        c.gap(28.0)
            .col(|d| {
                d.image_bytes(gradient_png(300, 300), |i| {
                    i.rounded(150.0).caption("头像");
                });
            })
            .col_weighted(2.0, |d| {
                d.heading(3, |h| {
                    h.text("张三");
                });
                d.paragraph(|p| {
                    p.text("等级 12 · 入群 480 天。这一栏权重 2,按权重分宽,栏内块级元素齐全。");
                });
                d.paragraph(|p| {
                    p.text("经验 7200 / 10000");
                });
                d.progress(0.72, |pb| {
                    pb.height(12.0).fill("#4c63b6");
                });
            });
    });
}

/// 面板卡片:并排等高数据卡 + 自定装饰卡。
fn panels(b: &mut Doc) {
    b.heading(2, |h| {
        h.text("面板卡片");
    })
    .columns(|c| {
        for (n, label) in [("128", "好友"), ("96", "群"), ("3.4k", "消息")] {
            c.panel(|p| {
                p.heading(2, |h| {
                    h.align(Align::Center).text(n);
                });
                p.paragraph(|d| {
                    d.align(Align::Center).text(label);
                });
            });
        }
    })
    .panel(|p| {
        p.bg("#eef2ff").border(2.0, "#4c63b6").rounded(18.0).shadow();
        p.paragraph(|d| {
            d.styled(
                "面板:底色 / 边框 / 圆角 / 内边距 / 投影的卡片容器;并排栏里整栏一个面板时自动拉齐行高。",
                |s| {
                    s.color("#312e81");
                },
            );
        });
    });
}

/// 进度条:默认胶囊 / 自定色与高度 / 限宽居中直角细条。
fn progress(b: &mut Doc) {
    b.heading(2, |h| {
        h.text("进度条");
    })
    .progress(0.62, |_| {})
    .progress(0.43, |pb| {
        pb.height(16.0).fill("#0e9488").track("#dbe2ec");
    })
    .progress(0.5, |pb| {
        pb.width_percent(60.0).align(Align::Center).radius(0.0).height(6.0);
    });
}

/// 文字阴影:默认投影与自定彩影。
fn text_shadow(b: &mut Doc) {
    b.heading(2, |h| {
        h.text("文字阴影");
    })
    .paragraph(|p| {
        p.styled("默认投影", |s| {
            s.bold().size(1.3).shadow();
        })
        .text("  与  ")
        .styled("自定彩影", |s| {
            s.bold().size(1.3).color("#4c63b6").shadow_with(2.0, 3.0, 6.0, "#93b4f8");
        });
    });
}

// ── 选项 ──────────────────────────────────────────────────────────────────────

/// 渲染选项:字体栈 + 页眉页脚 + 具名图 `@grad`。
fn options(theme: Theme) -> RenderOptions {
    let mut opts = RenderOptions { theme, ..RenderOptions::default() }
        .with_fonts(fonts())
        .with_header_chrome(
            PageChrome::rich(|p| {
                p.styled("nagisa-render", |s| {
                    s.weight(600);
                })
                .text(" · 排版引擎");
            })
            .trailing(|p| {
                p.text("全功能样张");
            }),
        )
        .with_footer_chrome(
            PageChrome::new("github.com/djkcyl/nagisa · 文档 → 图片")
                .align(Align::Center)
                .band("#1f2937")
                .color("#d1d5db"),
        );
    opts.images.insert("grad".into(), gradient_png(480, 240));
    opts
}

/// 字体栈:内置 + 系统,外加 `GALLERY_FONTS` 目录里的字体文件(支持 .zst)。
fn fonts() -> FontHandle {
    static FONTS: std::sync::OnceLock<FontHandle> = std::sync::OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut b = FontHandle::builder().bundled().system();
            if let Ok(dir) = std::env::var("GALLERY_FONTS") {
                for entry in fs::read_dir(&dir).expect("读 GALLERY_FONTS 目录").flatten() {
                    let p = entry.path();
                    let name = p.to_string_lossy();
                    if name.ends_with(".ttf") || name.ends_with(".otf") || name.ends_with(".zst") {
                        b = b.data(fs::read(&p).expect("读字体文件"));
                    }
                }
            }
            b.build().expect("构建字体栈")
        })
        .clone()
}

/// 生成一张渐变 PNG(样张里的占位图)。
fn gradient_png(w: u32, h: u32) -> Vec<u8> {
    let img = ImageBuffer::from_fn(w, h, |x, y| Rgba([(x * 255 / w) as u8, (y * 255 / h) as u8, 170, 255]));
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).expect("编码占位图");
    buf.into_inner()
}
