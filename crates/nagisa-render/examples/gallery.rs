//! 样张画廊 —— 渲染各特性样张到 `out/*.png`,人工看图核对(本项目无常驻单测)。
//! 跑:`cargo run -p nagisa-render --example gallery`。
//!
//! 衬线 / 楷体不随框架内置(包体上限),裸检出里这两类样张会回退黑体;设
//! `GALLERY_FONTS=<目录>` 指向存放 `.ttf`/`.ttf.zst` 的目录(比如 abot 的
//! `assets/fonts`)可补全。

use image::{ImageBuffer, Rgba};
use nagisa_render::{
    render_document, render_markup, Align, Doc, Document, FontHandle, Length, ListKind,
    RenderOptions, Theme,
};
use std::fs;
use std::sync::OnceLock;

/// 渲染选项:内置字体 + 系统字体,外加 `GALLERY_FONTS` 目录里的字体文件(支持 .zst)。
fn opts() -> RenderOptions {
    static FONTS: OnceLock<FontHandle> = OnceLock::new();
    let fonts = FONTS
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
        .clone();
    RenderOptions::default().with_fonts(fonts)
}

/// 综合 markup 样张(覆盖标题 / 行内样式 / 列表 / 引用 / 代码 / 围栏对齐 / 分割线)。
const SHOWCASE: &str = r#"# 排版引擎 · 综合样张 {align=center}

支持 **粗体**、*斜体*、~~删除~~、`行内代码`、==高亮==、[链接](https://example.com) 与 [自定义色]{color=#7c3aed,bold}。CJK 与 English 在同一行自动混排、按宽换行,标点也参与断行;user_id 这类标识符不会被 `_` 吞掉。

## 字体

正文默认是黑体(Noto Sans SC)。[这一句切成衬线字体,用的是内置的思源宋体,**粗体**也是真字重。]{font=serif}混在同一段里各排各的。

字重可调:[细体 300]{light} / 常规 400 / **粗体 700**,也能指定任意档位 [Medium 500]{weight=500}、[Black 900]{weight=900};[mono italic]{font=mono,italic} 有独立的斜体字面。

[这一句是楷体,内置的霞鹜文楷,同样细 / 常规 / 粗三档:]{font=kai}[细]{font=kai,light}[、]{font=kai}[常规]{font=kai}[、]{font=kai}[**粗**(它家最重是 500)]{font=kai}[。]{font=kai}

## 列表

- 第一项,带子列表
  - 子项 A
  - 子项 B
- [x] 任务列表:已完成
- [ ] 任务列表:待办

9. 有序起步可设
10. 多位数序号
11. 小数点右对齐不挤

## 引用与代码

> 这是一段引用,左侧有强调色竖条,内容整体内缩。

```rust
fn main() {
    println!("Hello, 世界");
}
```

## 表格

| 功能 | 说明 | 状态 |
|:--|:--|:-:|
| 多栏 | 显式并排栏,按权重分宽 | 完成 |
| 表格 | 自适应列宽,可手动限宽 | 完成 |

::: center
—— 居中的一段说明文字 ——
:::

---
最后一段普通正文,收尾。
"#;

fn main() {
    fs::create_dir_all("out").expect("建 out 目录");

    // 文本样张:标题 + CJK/拉丁混排 + 行内样式 + 居中。
    let doc = Doc::new()
        .heading(1, |h| {
            h.text("排版引擎样张");
        })
        .paragraph(|p| {
            p.text("这是一段中文与 English 混排的正文,用来检验 CJK 与拉丁字母在同一行里的整形、")
                .text("断行与基线对齐。文本超过一行会按内容宽自动换行,标点也参与断行。");
        })
        .paragraph(|p| {
            p.text("行内有 ")
                .bold("粗体")
                .text("、")
                .italic("斜体")
                .text("、")
                .styled("彩色", |s| {
                    s.color("#2563eb");
                })
                .text("、")
                .code("inline_code")
                .text(" 这些样式。");
        })
        .paragraph(|p| {
            p.align(Align::Center).text("—— 这一段居中 ——");
        })
        .build();

    write_png("out/hello.png", &doc);

    // 块样张:嵌套列表 + 引用 + 分割线 + 代码块。
    let blocks = Doc::new()
        .heading(2, |h| {
            h.text("块级元素");
        })
        .list(ListKind::Unordered, |l| {
            l.item(|i| {
                i.text("第一项,带一个子列表").list(ListKind::Unordered, |s| {
                    s.item(|i| {
                        i.text("子项 A");
                    })
                    .item(|i| {
                        i.text("子项 B");
                    });
                });
            })
            .item(|i| {
                i.text("第二项");
            })
            .task(true, |i| {
                i.text("构建器也能写任务项");
            });
        })
        .list(ListKind::Ordered, |l| {
            l.item(|i| {
                i.text("有序一");
            })
            .item(|i| {
                i.text("有序二");
            });
        })
        .quote(|q| {
            q.paragraph(|p| {
                p.text("引用块:左侧有一条强调色竖条,内容整体内缩。");
            });
        })
        .divider()
        .code(
            "rust",
            "fn main() {\n    println!(\"代码块:等宽字 + 圆角底色 + 软换行\");\n}",
        )
        .build();
    write_png("out/blocks.png", &blocks);

    // 行内装饰样张:高亮 / 行内代码 / 删除 / 下划 / 自定义底色。
    let inline = Doc::new()
        .heading(3, |h| {
            h.text("行内装饰");
        })
        .paragraph(|p| {
            p.text("这里有 ")
                .highlight("高亮")
                .text("、")
                .code("行内代码")
                .text("、")
                .strike("删除线")
                .text("、")
                .underline("下划线")
                .text(",还有自定义底色 ")
                .styled("黄底强调", |s| {
                    s.bg("#fde047");
                })
                .text(" 收尾。混在一行里也能各自定位。");
        })
        .paragraph(|p| {
            p.text("醒目标注:自适应椭圆圈 ")
                .styled("缺货", |s| {
                    s.ring_color("#dc2626");
                })
                .text(",定径正圆(单字双字同大)")
                .styled("1", |s| {
                    s.ring_radius(22.0);
                })
                .text(" 与 ")
                .styled("10", |s| {
                    s.ring_radius(22.0);
                })
                .text(",扁椭圆 ")
                .styled("年度目标", |s| {
                    s.ring_radii(58.0, 20.0).ring_stroke(2.0);
                })
                .text(";着重点 ")
                .styled("这几个字", |s| {
                    s.dot();
                })
                .text(" 与定径色点 ")
                .styled("重点", |s| {
                    s.dot_color("#0e9488").dot_radius(3.5);
                })
                .text(";圈与点都画进行距,不动布局。");
        })
        .paragraph(|p| {
            p.text("逐字模式:着重号正字法 ")
                .styled("字字有点", |s| {
                    s.dot_each().dot_color("#dc2626");
                })
                .text(",一字一圈 ")
                .styled("天天圈", |s| {
                    s.ring_each().ring_color("#4c63b6");
                })
                .text(",逐字定径 ")
                .styled("1 8 24", |s| {
                    s.ring_each().ring_radius(20.0);
                })
                .text("(空白不标)。");
        })
        .paragraph(|p| {
            p.align(Align::Center).text("边注:这行居中只按本句算").styled("当前", |s| {
                s.aside_right().color("#8a8f98").size(0.8);
            });
            p.styled("▶", |s| {
                s.aside_left().color("#0e9488");
            });
        })
        .build();
    write_png("out/inline.png", &inline);

    // 块级图样张:解码 + 缩放 + 居中 + 图注。
    let images = Doc::new()
        .heading(2, |h| {
            h.text("图片");
        })
        .paragraph(|p| {
            p.text("下面是一张块级图,宽 60%、居中,带图注:");
        })
        .image_bytes(gradient_png(480, 240), |i| {
            i.width_percent(60.0).align(Align::Center).caption("示例渐变图(480×240)");
        })
        .paragraph(|p| {
            p.text("解码、缩放、对齐与图注都在引擎里完成。");
        })
        .build();
    write_png("out/images.png", &images);

    // 并排栏样张:权重栏(图 + 富内容)+ 三等分数据块。
    let cols = Doc::new()
        .heading(2, |h| {
            h.text("并排栏");
        })
        .columns(|c| {
            c.gap(28.0)
                .col(|b| {
                    b.image_bytes(gradient_png(300, 300), |i| {
                        i.caption("头像");
                    });
                })
                .col_weighted(2.0, |b| {
                    b.heading(3, |h| {
                        h.text("张三");
                    });
                    b.paragraph(|p| {
                        p.text("简介:这一栏权重 2,比左栏宽。文字在本栏宽里自动换行,标题、段落、列表都能放。");
                    });
                    b.list(ListKind::Unordered, |l| {
                        l.item(|i| {
                            i.text("等级 12");
                        })
                        .item(|i| {
                            i.text("积分 3450");
                        });
                    });
                });
        })
        .divider()
        .columns(|c| {
            for (n, label) in [("128", "好友"), ("96", "群"), ("3.4k", "消息")] {
                c.col(|b| {
                    b.heading(2, |h| {
                        h.align(Align::Center).text(n);
                    });
                    b.paragraph(|p| {
                        p.align(Align::Center).text(label);
                    });
                });
            }
        })
        .build();
    write_png("out/columns.png", &cols);

    // 表格样张:自适应列宽 + 手动限宽 + 按列/格设样式与背景上色。
    let table = Doc::new()
        .heading(2, |h| {
            h.text("表格");
        })
        .table(|t| {
            t.head(["姓名", "积分", "状态", "备注"])
                .align([Align::Left, Align::Right, Align::Center, Align::Left])
                .width(3, Length::Px(170.0))
                .row(["张三", "3450", "正常", "活跃用户,本月发言很多"])
                .row(["李四", "985", "警告", "新人"])
                .row(["王五", "12048", "封禁", "管理员"]);
            // 积分列加粗;状态列按值上色(背景 + 文字色)。
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
            p.text("列宽自适应、「备注」限 170px;积分列加粗,状态列按格上色(背景 + 文字色)。");
        })
        .build();
    write_png("out/table.png", &table);

    // 紧凑度 / 网格控制对比。
    let compact = Doc::new()
        .heading(3, |h| {
            h.text("默认");
        })
        .table(|t| {
            t.head(["项目", "数值"])
                .align([Align::Left, Align::Right])
                .row(["第一项", "10"])
                .row(["第二项", "20"]);
        })
        .heading(3, |h| {
            h.text("行收紧 + 只留行横线");
        })
        .table(|t| {
            t.head(["项目", "数值"])
                .align([Align::Left, Align::Right])
                .row(["第一项", "10"])
                .row(["第二项", "20"])
                .pad_y(5.0)
                .grid_vertical(false)
                .grid_outer(false);
        })
        .heading(3, |h| {
            h.text("极简:无线 + 无表头底 + 列也收紧");
        })
        .table(|t| {
            t.head(["项目", "数值"])
                .align([Align::Left, Align::Right])
                .row(["第一项", "10"])
                .row(["第二项", "20"])
                .pad_x(8.0)
                .pad_y(5.0)
                .no_grid()
                .header_fill(false);
        })
        .build();
    write_png("out/table-compact.png", &compact);

    // 进度条样张:默认胶囊 / 自定义色与高度 / 直角细条 / 限宽对齐 / 0 与 1 两端。
    let progress = Doc::new()
        .heading(2, |h| {
            h.text("进度条");
        })
        .paragraph(|p| {
            p.text("默认:铺满内容宽,主题强调色,胶囊形。");
        })
        .progress(0.62, |_| {})
        .paragraph(|p| {
            p.text("自定义:高 16、靛蓝填充、浅灰底槽。");
        })
        .progress(0.43, |b| {
            b.height(16.0).fill("#4c63b6").track("#dbe2ec");
        })
        .paragraph(|p| {
            p.text("直角细条(radius 0,高 6):");
        })
        .progress(0.8, |b| {
            b.height(6.0).radius(0.0);
        })
        .paragraph(|p| {
            p.text("限宽 60% 居中:");
        })
        .progress(0.5, |b| {
            b.width_percent(60.0).align(Align::Center).fill("#0e9488");
        })
        .paragraph(|p| {
            p.text("两端:0(全槽)与 1(全满)。");
        })
        .progress(0.0, |_| {})
        .progress(1.0, |_| {})
        .build();
    write_png("out/progress.png", &progress);

    // 综合样张:同一段 markup,亮 / 暗两套主题。
    write_markup("out/showcase-light.png", SHOWCASE, Theme::light());
    write_markup("out/showcase-dark.png", SHOWCASE, Theme::dark());
}

fn write_markup(path: &str, src: &str, theme: Theme) {
    let opts = RenderOptions { theme, ..opts() };
    let png = render_markup(src, &opts).expect("渲染 markup");
    fs::write(path, &png).expect("写文件");
    println!("wrote {path} ({} bytes)", png.len());
}

/// 生成一张渐变 PNG(测试用)。
fn gradient_png(w: u32, h: u32) -> Vec<u8> {
    let img = ImageBuffer::from_fn(w, h, |x, y| {
        Rgba([(x * 255 / w) as u8, (y * 255 / h) as u8, 170, 255])
    });
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).expect("编码测试图");
    buf.into_inner()
}

fn write_png(path: &str, doc: &Document) {
    let png = render_document(doc, &opts()).expect("渲染");
    fs::write(path, &png).expect("写文件");
    println!("wrote {path} ({} bytes)", png.len());
}
