# AGENTS.md

给在**本仓库内改框架代码**的 coding agent 的约定。用 nagisa 写 bot 的 API 上下文见 llms.txt,人类向文档见 README.md。

## 仓库形态

- Rust workspace,8 crates:`nagisa-types`(跨协议通用的域模型)/ `nagisa-core`(运行时引擎 + 适配器共享基建)/ `nagisa-onebot`(OneBot v11 适配器,6 种传输)/ `nagisa-milky`(Milky 适配器)/ `nagisa-macros`(7 个宏入口)/ `nagisa-log`(可选日志,feature "log")/ `nagisa-render`(可选排版引擎:文档 → 图片,feature "render",与协议解耦)/ `nagisa`(门面,**唯一对外入口**)。
- 提交:正常按改动分次提交,写清楚 commit message。

## 门禁(改完必须全跑、真跑、看真实输出)

```sh
cargo fmt                    # rustfmt.toml:行宽 120 + Max 启发式,护紧凑链式风格
cargo build
cargo clippy --all-targets -- -D warnings
cargo doc --no-deps          # 必须零警告
```

- **刻意零测试**:不要添加任何 `#[test]` / 集成测试,验证靠以上四件套 + 消费者构建。
- 消费者联动:若本地有依赖本仓的下游消费者(path 依赖),公开面改动后对其 manifest 跑一次 `cargo build`,验证未破坏对接。

## 文档与注释约定

- rustdoc **全中文**(技术 token 保留英文);不留施工编号/考古叙事/TODO 残留。
- 注释高门槛:复读签名的 gloss 不写,只写非显然契约(必填无默认、惰性与否、判别字段、刻意行为)。
- 带「刻意」「不要」字样的红线注释**必须保留**(如:反向 WS 不套 pump 的原因、SSE 不发 Disconnect、Tick 不重置 idle、wire 日志 target 名不可改、协议字符串字面量是代码)。
- 动作的 OFFICIAL / ENDPOINT 溯源注释只在 adapter impl 侧一处,trait 侧不重复。

## 设计红线

- 功能面**不按消费者用不用来裁剪**;只删真重复/真死/真不合理的东西。
- bot 作者只依赖 `nagisa` 门面:新增公开 API 必须经门面/prelude 导出;不让消费者直接碰 nagisa-core / nagisa-types / inventory / async-trait。
- 错误口径自有 `Result` / `Error` / `Context`,无 anyhow;动作错误按 `ActionErrorKind` 粗分类,绝不比对 retcode 数字。
- 宏属性键、协议字符串字面量、`Vendor` 判定子串都是公开契约,改名即破坏消费者/协议对接。
- `Vendor` 是 OneBot 专用轴(存适配器内部,公开口唯一 `bot.vendor()`);Milky 端识别走 `ImplInfo`,`ImplInfo` 不带 vendor 字段。

## 同步义务

改公开 API(签名/属性键/枚举变体/默认值)后同步 README.md。llms.txt 是**能力地图 + 指路 + 坑清单**(刻意不抄签名,签名以源码为准):新增/移除能力、移动文件、或踩出新的行为坑时才更新它。
