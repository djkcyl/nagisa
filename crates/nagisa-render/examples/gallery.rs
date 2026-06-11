//! 样张画廊 —— 渲染各特性样张到 `out/*.png`,人工看图核对(本项目无常驻单测)。
//! 跑:`cargo run -p nagisa-render --example gallery`。
//!
//! 衬线 / 楷体不随框架内置(包体上限),裸检出里这两类样张会回退黑体;设
//! `GALLERY_FONTS=<目录>` 指向存放 `.ttf`/`.ttf.zst` 的目录(比如 abot 的
//! `assets/fonts`)可补全。

use image::{ImageBuffer, Rgba};
use nagisa_render::{
    parse_markup, render_document, render_markup, Align, Anchor, Doc, Document, FontHandle,
    Length, ListKind, PageChrome, RenderOptions, Theme,
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
        .heading(3, |h| {
            h.text("富文本格 + 窄表居中");
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
        .build();
    write_png("out/table.png", &table);

    // 标记文本块级图属性:尾部 {width/align/rounded/shadow/border},命名来源 @grad。
    let img_attrs = parse_markup(
        "## 标记文本图属性\n\n![40% 宽 · 居中 · 圆角 · 投影](@grad){width=40%, align=center, rounded=16, shadow}\n",
    )
    .expect("解析图属性样张");
    let mut img_opts = opts();
    img_opts.images.insert("grad".into(), gradient_png(480, 240));
    let png = render_document(&img_attrs, &img_opts).expect("渲染图属性样张");
    fs::write("out/image-attrs.png", &png).expect("写文件");
    println!("wrote out/image-attrs.png ({} bytes)", png.len());

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

    // 代码上色样张:四门语言,亮暗双主题各一张。
    const CODE_MD: &str = r#"## 代码上色

```rust
/// 求和并打印。注释是注释色。
fn main() {
    let nums = vec![1, 2, 3_000, 0xff];
    let s: i64 = nums.iter().sum();
    println!("sum = {s}, ok = {}", true); // 行尾注释
}
```

```json
{ "name": "abot", "version": 0.6, "stable": true, "tags": ["bot", "qq"], "extra": null }
```

```python
# 阶乘,递归写法
def fact(n: int) -> int:
    return 1 if n <= 1 else n * fact(n - 1)

print(f"5! = {fact(5)}", True, None)
```

```shell
# 部署脚本片段
if [ -f .env ]; then
  export $(cat .env | xargs)  # 读环境
fi
echo "deployed at $(date)"
```
"#;
    write_markup("out/code-light.png", CODE_MD, Theme::light());
    write_markup("out/code-dark.png", CODE_MD, Theme::dark());

    // 面板样张:默认卡片 / 自定装饰 / 并排等高卡片 / markup 围栏。
    let panel = Doc::new()
        .heading(2, |h| {
            h.text("面板");
        })
        .panel(|p| {
            p.text("默认卡片:主题浅底 + 细边 + 圆角,内边距 0.6 倍基准字号。");
        })
        .panel(|p| {
            p.bg("#eef2ff").border(2.0, "#4c63b6").rounded(18.0).shadow();
            p.heading(3, |h| {
                h.text("自定装饰");
            });
            p.paragraph(|d| {
                d.text("底色、边框、圆角、投影都可调;内层是块容器,").bold("什么块都能放").text("。");
            });
            p.progress(0.7, |b| {
                b.height(10.0).fill("#4c63b6");
            });
        })
        .columns(|c| {
            c.panel(|p| {
                p.heading(2, |h| {
                    h.align(Align::Center).text("128");
                });
                p.paragraph(|d| {
                    d.align(Align::Center).text("好友");
                });
            })
            .panel(|p| {
                p.bg("#ecfdf5");
                p.heading(2, |h| {
                    h.align(Align::Center).text("96");
                });
                p.paragraph(|d| {
                    d.align(Align::Center).text("群");
                });
                p.paragraph(|d| {
                    d.align(Align::Center).styled("这栏内容更高,左右卡片自动拉齐", |st| {
                        st.color("#0e9488").size(0.8);
                    });
                });
            })
            .panel(|p| {
                p.heading(2, |h| {
                    h.align(Align::Center).text("3.4k");
                });
                p.paragraph(|d| {
                    d.align(Align::Center).text("消息");
                });
            });
        })
        .build();
    write_png("out/panel.png", &panel);

    // markup 围栏版:::: panel 与带装饰属性的 ::: col。
    write_markup(
        "out/panel-markup.png",
        "::: panel {bg=#fff7ed, border=#f59e0b, rounded=14}\n标记文本同样写得出卡片:`::: panel {bg=… border=… rounded=… pad=… shadow}`。\n:::\n\n::: columns\n::: col {bg=#eef2ff}\n左卡\n:::\n::: col 2 {border=#0e9488}\n右卡权重 2,`::: col 权重 {属性}`。\n:::\n:::\n",
        Theme::light(),
    );

    // 综合样张:同一段 markup,亮 / 暗两套主题。
    write_markup("out/showcase-light.png", SHOWCASE, Theme::light());
    write_markup("out/showcase-dark.png", SHOWCASE, Theme::dark());

    // 全功能样张:标记文本块 + 构建器块拼成同一份文档,页眉 / 页脚配在选项上。
    write_full();
}

/// 全功能样张的标记文本前半(行内 / 字族 / 圈点边注 / 列表 / 引用 / 代码 / GFM 表格)。
const FULL_HEAD: &str = r#"# nagisa-render · 全功能样张 {align=center}

::: center
[标记文本与 Rust 构建器,同一份文档模型,排版渲染成图片]{font=kai}
:::

## 行内样式

一行混排:**粗体**、*斜体*、***粗斜***、~~删除~~、[下划线]{underline}、`行内代码`、==高亮==、[自定底色]{bg=#fde047}、[彩色加粗]{color=#7c3aed,bold}、[字号 1.3×]{size=1.3}、[0.8×]{size=0.8} 与 [链接](https://github.com/djkcyl/nagisa);CJK、English 与 emoji 😄⛏️🤖 自动整形断行,user_id 不会被 `_` 吞掉,转义 \*照常星号\*。

字重任意档:[细 300]{light} · 常规 400 · [Medium 500]{weight=500} · **粗 700** · [Black 900]{weight=900};行尾反斜杠硬换行 \
这是换出来的第二行。

## 字族

正文黑体(Noto Sans SC);[这一句切衬线,思源宋体,**粗体**是真字重;]{font=serif}[这一句楷体,霞鹜文楷;]{font=kai}[mono italic 真斜体字面]{font=mono,italic},CJK 斜体为仿斜。

## 圈注 · 着重点 · 边注

库存 [缺货]{ring=#dc2626},定径正圆 [1]{ring-radius=22} 与 [10]{ring-radius=22} 同大,扁椭圆 [年度目标]{ring-rx=72,ring-ry=24,ring-stroke=2},逐字 [天天圈]{ring-each,ring=#4c63b6};着重点 [这几个字]{dot},正字法逐字 [字字有点]{dot-each,dot=#dc2626}。圈与点画进行距,不动布局。

::: center
这行居中只按正文算,边注挂在行外[当前]{aside,color=#8a8f98,size=0.8}
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

> 引用块:强调色竖条,内容整体内缩,内层还能放块。

```rust
/// 语言标签在盒角,词按语言上色。
fn main() {
    let nums = vec![1, 2, 0xff];
    println!("sum = {}, ok = {}", nums.iter().sum::<i32>(), true);
}
```

---

## 表格(标记文本 GFM)

| 前端 | 适合 | 行内能力 |
|:--|:-:|--:|
| 标记文本 | 一大段文字 | 全部 |
| 构建器 | 从数据生成卡片 | 全部 |

## 块级图属性(标记文本)

![40% 宽 · 居中 · 圆角 · 投影](@grad){width=40%, align=center, rounded=16, shadow}
"#;

/// 全功能样张的标记文本收尾。
const FULL_TAIL: &str = r#"---

::: center
[—— 样张完,底部色带是页脚 PageChrome ——]{color=#8a8f98}
:::
"#;

/// 全功能样张:FULL_HEAD(标记文本)+ 构建器接力(表格进阶 / 图片装饰 / 并排栏 / 进度条 /
/// 文字阴影,标记文本没有的能力)+ FULL_TAIL,一图出全。README 例图由此而来。
fn write_full() {
    let mut doc = parse_markup(FULL_HEAD).expect("解析样张标记文本");

    let mut b = Doc::new();
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
    })
    .heading(2, |h| {
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
    })
    .heading(2, |h| {
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
    })
    .heading(2, |h| {
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
            d.text("面板:底色 / 边框 / 圆角 / 内边距 / 投影的卡片容器;并排栏里整栏一个面板时自动拉齐行高。标记文本写 ")
                .code("::: panel {bg=… border=…}")
                .text("。");
        });
    })
    .heading(2, |h| {
        h.text("进度条");
    })
    .progress(0.62, |_| {})
    .progress(0.43, |pb| {
        pb.height(16.0).fill("#0e9488").track("#dbe2ec");
    })
    .progress(0.5, |pb| {
        pb.width_percent(60.0).align(Align::Center).radius(0.0).height(6.0);
    })
    .heading(2, |h| {
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
    doc.blocks.extend(b.build().blocks);
    doc.blocks.extend(parse_markup(FULL_TAIL).expect("解析样张标记文本").blocks);

    let mut opts = opts()
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

    let png = render_document(&doc, &opts).expect("渲染全功能样张");
    fs::write("out/full.png", &png).expect("写文件");
    println!("wrote out/full.png ({} bytes)", png.len());
    let webp = render_document(&doc, &opts.webp_or_png()).expect("渲染全功能样张");
    fs::write("out/full.webp", &webp).expect("写文件");
    println!("wrote out/full.webp ({} bytes)", webp.len());
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
