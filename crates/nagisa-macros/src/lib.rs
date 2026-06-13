//! Nagisa 的过程宏：QQ 机器人框架的「声明式注册面」。
//!
//! 一个 bot 作者只依赖门面 crate `nagisa`(它再导出本 crate 的宏)。这里的宏让作者
//! 用普通的 `async fn` + 属性写命令/事件 handler,展开后自动挂上 router 并登记进
//! 全局注册表,无需手写任何注册样板。
//!
//! # 路径解析
//!
//! 展开出的代码全部用**全限定路径**引用引擎 crate(`nagisa-core`)的项,其路径根在
//! 展开期由 `proc-macro-crate` 解析(见 `nagisa_core_root`):依赖门面 `nagisa` 时走
//! `::nagisa::nagisa_core`,直接依赖引擎时命名 `nagisa-core`。因此宏在任意消费 crate 内
//! 都可用,且不强制作者直接依赖 `nagisa-core`。
//!
//! # 提供的宏
//!
//! 属性宏(作用于 `async fn`):
//! - [`macro@command`] —— 把一个 `async fn` 注册成**消息命令触发器**(匹配器 + 参数解析)。
//! - [`macro@event`] —— 把一个 `async fn` 注册成**事件触发器**(按 `EventKind` 派发)。
//!
//! 函数式宏 / derive:
//! - [`macro@plugin`] —— 声明「当前模块即一个插件」,登记一个 `PluginSpec`。
//! - [`macro@Args`](derive) —— 为结构体生成 `ParseArgs`,在消息段流上解析命令参数。
//! - [`macro@ArgEnum`](derive) —— 为无字段枚举生成 `FromArg`(按变体名匹配受限选项)。
//! - [`macro@Slots`](derive) —— 为「命令头 + 命名类型化正则槽」结构体生成 `FromSlots`。
//! - [`macro@matcher`] —— `#[derive(Slots)]` 的内联函数式糖,直接求值出一个 `Matcher`。
//!
//! # 触发器宏的共同展开形状(概念级)
//!
//! `#[command]` 与 `#[event]` 都**原样保留**被标注的函数,并额外发出三样东西:
//! 1. `pub const <FN>_KEY: SwitchKey` —— 该触发器的强类型分层开关键句柄,供
//!    `EnabledSet::set(<FN>_KEY, ..)` 用编译期校验的键开关此触发器(拼错即编译报错)。
//! 2. `fn <FN>__nagisa_register(r: Router) -> Router` —— 同名兄弟注册函数,内部解析
//!    本触发器归属的插件(按 `module_path!()` 最长前缀),把 handler 挂上 router
//!    (`Router::trigger_command` / `Router::event_named`,带 `Option<Rule>` 门控槽)。
//! 3. `inventory::submit!(TriggerSpec { .. })` —— 把触发器元数据登记进全局注册表,
//!    供 `collect_into` / `registered_triggers` 链接期收集。
//!
//! `plugin!{}` 类似,只 `inventory::submit!(PluginSpec { .. })` 一个插件元数据。
//!
//! # 拼写约定
//!
//! - 可禁用开关:`can_disable`(与 `plugin!{}` 一致)。
//! - 命令匹配器:前导位置参数 `"a", "b"`(对齐 `Matcher::command`)。
use proc_macro::TokenStream;
use proc_macro_crate::{crate_name, FoundCrate};
use quote::{format_ident, quote};
use syn::{parse_macro_input, DeriveInput, ItemFn};

mod args_derive;
mod attrs;
mod plugin;
mod slots;
mod trigger;

use attrs::{CommandArgs, EventArgs};
use plugin::PluginArgs;
use slots::MatcherMacro;

/// 在宏展开期解析引擎 crate（`nagisa-core`）的路径根，使展开出的代码无论被哪个消费
/// crate 使用都能正确指名它——且不强制作者直接依赖 `nagisa-core`。
///
/// 各宏追加的全限定深尾（`::plugin::…`、`::registry::…`、`::args::…`、`::inventory`、
/// `::Matcher`、`::SwitchKey`、`::EventKind`、`::ArgToken`…）拼在返回的根之后。
///
/// 解析顺序：
/// - 若依赖了**门面** crate（`nagisa`），经它走。门面把引擎再导出为 `nagisa::nagisa_core`，
///   故 bot 作者只依赖 `nagisa`。注意：`nagisa` 自己的集成测试（`crates/nagisa/tests/`）也用
///   这些宏、且属于 `nagisa` 这个 PACKAGE，故 `crate_name("nagisa")` 在那里返回 `Itself`——但
///   集成测试是独立 crate，**无法**经 `crate::` 触及库本体。因此 `Itself` 分支发出 crate
///   名 `::nagisa::nagisa_core`（有效：`::nagisa` 解析到库本体、它再导出 `nagisa_core`），而非
///   `crate::…`。
/// - 否则门面不在作用域内；回落到直接指名 `nagisa-core`（依赖引擎但不经门面的 crate）。
pub(crate) fn nagisa_core_root() -> proc_macro2::TokenStream {
    match crate_name("nagisa") {
        Ok(FoundCrate::Itself) => quote! { ::nagisa::nagisa_core },
        Ok(FoundCrate::Name(n)) => {
            let id = format_ident!("{n}");
            quote! { ::#id::nagisa_core }
        }
        Err(_) => match crate_name("nagisa-core") {
            Ok(FoundCrate::Itself) => quote! { crate },
            Ok(FoundCrate::Name(n)) => {
                let id = format_ident!("{n}");
                quote! { ::#id }
            }
            Err(_) => quote! { ::nagisa_core },
        },
    }
}

/// 把一个 `async fn` 注册成**消息命令触发器**。
///
/// 保留原函数,并发出 `<FN>_KEY` 开关键、`<FN>__nagisa_register` 注册函数与一条
/// `TriggerSpec` 登记(详见 crate 级文档「触发器宏的共同展开形状」)。
///
/// # 属性键
///
/// 匹配器(**三选一,互斥,必给其一**):
/// - **前导位置参数** `"签到", "sign", ..`:字面量命令词(写在属性最前),编译成正则;至少一个。
/// - `regex = "^..$"`:原始正则字面量(展开期 `Matcher::regex(..).expect(..)`,
///   非法正则在该消费 crate 编译/启动时即暴露)。
/// - `slots = <Type>`:取一个 `#[derive(Slots)]` 类型的 `<Type as FromSlots>::matcher()`
///   作头匹配器。
///
/// 行为:
/// - `mention_me`(裸旗标):要求消息 @ 本机器人才触发。
/// - `top`(裸旗标):一级 top 观察者,永不被 waiter 拦截。
/// - `priority = N`:整数优先级(支持负数),越大越先。
/// - `usage = "..."`:parse-miss 时回贴给用户的用法串(命令专属;`#[event]` 拒绝此键)。
///
/// 门控(原样搬运的表达式,宏对其内容零知识):
/// - `gate = <Rule expr>`:任意 `Rule` 表达式,原样拼进门控槽。
/// - `cooldown = <expr>`:经 `Cooldown::from(<expr>).into_rule(..)` 物化,AND 进门控链最右
///   (权限/开关先判,冷却最后才盖戳)。
///
/// 元数据(→ `TriggerMeta`,缺省由展开期回填):
/// - `id = "..."`(缺省取函数名)、`name = "..."`(缺省取函数名)、`description = "..."`。
/// - `can_disable = <bool>`(缺省 `true`)、
///   `default_enable = <bool>`(缺省 `true`)、`hidden = <bool>`(缺省 `false`)。
///
/// 命令**体**(参数)始终声明在 handler 形参上(`args: Args<MyArgs>`),而非属性里:
/// 头写在属性、体写在形参,二者就近落在同一 handler。
///
/// # 触发语义(命令**不**互斥)
///
/// dispatch 对每条消息**逐个**试所有命令的匹配器、命中即跑,**不**首中即停、**不**做「最具体优先」——
/// 命令型 handler 之间互不阻断(只有 `top` 观察者与中断 `waiter` 的 `block` 能拦)。故两条匹配范围
/// **重叠**的命令会**都触发**。约定:**命令作者自行保证匹配不重叠**。
///
/// 字面量命令词天然不必担心「前缀吃后缀」:它们编成 `^(?:词…)(?:\s|$)`,末尾的 `(?:\s|$)` 边界让
/// `签到` 不会命中 `签到日历`(CJK 无词边界,靠这个显式边界)。重叠风险主要来自 `regex` / 多命令
/// 词人为撞车。
///
/// # 约束
///
/// handler 必须是非泛型、无 `self` 接收者的 `async fn`(否则编译报错)。
///
/// # 示例
///
/// ```rust,ignore
/// // 字面量命令词 + 结构化参数:
/// #[command("echo", "复读", description = "复读一句话")]
/// async fn echo(ctx: &Context, args: Args<EchoArgs>) -> Result<()> { /* .. */ }
///
/// // 正则头 + @机器人 + 优先级 + 声明式门控:
/// #[command(regex = "^ping$", mention_me, priority = 10, gate = is_admin())]
/// async fn ping(ctx: &Context) -> Result<()> { /* .. */ }
///
/// // 用 #[derive(Slots)] 类型作头匹配器 + 冷却:
/// #[command(slots = ViewBoard, cooldown = 30)]
/// async fn view(ctx: &Context, slots: Slots<ViewBoard>) -> Result<()> { /* .. */ }
/// ```
#[proc_macro_attribute]
pub fn command(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as CommandArgs);
    let func = parse_macro_input!(item as ItemFn);
    match trigger::expand(args, func) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// 把一个 `async fn` 注册成**事件触发器**(与 `#[command]` 对等,但按 `EventKind` 派发)。
///
/// 展开镜像 `#[command]`:保留原函数,发出 `<FN>_KEY`、`<FN>__nagisa_register` 与一条
/// `TriggerSpec`,但**无匹配器**——经 `Router::event_named` 以解析出的 `EventKind`
/// 挂载,登记时 `kind` 记为 `TriggerKind::Event(<kind>)`。
///
/// # 属性键
///
/// - **第一个位置参数(必给)**:`EventKind` 变体的裸标识符,如 `#[event(MemberJoin)]`
///   / `#[event(Nudge)]`。
/// - `top`(裸旗标):一级 top 观察者,永不被 waiter 拦截。
/// - `priority = N`:整数优先级(支持负数)。
/// - 门控:`gate = <Rule expr>`、`cooldown = <expr>`(语义同 `#[command]`)。
/// - 元数据:`id`(缺省取函数名)、`name`(缺省取函数名)、`description`、
///   `can_disable`(缺省 `true`)、`default_enable`(缺省 `true`)、
///   `hidden`(缺省 `false`)。
///
/// 事件触发器无匹配器/参数,故 `command` / `regex` / `slots` / `mention_me` 不适用;
/// `usage` 因无 parser 可 miss 而被显式拒绝(写了会编译报错)。
///
/// # 约束
///
/// handler 必须是非泛型、无 `self` 接收者的 `async fn`。
///
/// # 示例
///
/// ```rust,ignore
/// #[event(MemberJoin, description = "新人入群欢迎")]
/// async fn welcome(ctx: &Context) -> Result<()> { /* .. */ }
///
/// #[event(Nudge, priority = 5)]
/// async fn on_nudge(ctx: &Context) -> Result<()> { /* .. */ }
/// ```
#[proc_macro_attribute]
pub fn event(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as EventArgs);
    let func = parse_macro_input!(item as ItemFn);
    match trigger::expand_event(args, func) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// 声明「当前模块即一个插件」,登记一个 `PluginSpec`。
///
/// 展开为一条 `inventory::submit!(PluginSpec { .. })`,其 `module_path` 取展开点的
/// `module_path!()`;同模块(及子模块)下的触发器据此(最长前缀匹配)归属本插件。
/// 省略的字段继承 `PluginMeta::DEFAULT`:`key=""` 哨兵(链接期由 `module_path` 末段
/// 回填)、`category=User`、可禁用 + 默认启用。
///
/// # 字段
///
/// 全部为可选键值项,以 `,` 分隔:
/// - `name = "..."`、`key = "..."`、`version = "..."`、`description = "..."`、`usage = "..."`(字符串)。
/// - `category = Fun`:**裸标识符**糖,展开为 `<root>::plugin::Category::Fun`。
/// - `can_disable = <bool>`、`default_enable = <bool>`、`hidden = <bool>`、`maintain = <bool>`。
///
/// 注:此宏**不**收 `gate` / `cooldown` / `id`。
///
/// # 示例
///
/// ```rust,ignore
/// plugin! {
///     name = "复读机",
///     key = "echo",
///     category = Fun,
///     description = "把你说的话再说一遍",
/// }
/// ```
#[proc_macro]
pub fn plugin(input: TokenStream) -> TokenStream {
    let fields = parse_macro_input!(input as PluginArgs);
    plugin::expand_plugin(fields).into()
}

/// 为带具名字段的结构体生成 `ParseArgs`,配合 `Args<T>` 提取器在消息**段流**上
/// 解析命令参数。
///
/// # 字段属性 `#[arg(..)]`
///
/// 文本参数(类型须实现 `FromArg`):
/// - 默认(无 `#[arg]` 或 `#[arg(positional)]`):**文本位置参数**,按声明顺序消费文本词。
///   `Option<T>` = 可选;`#[arg(default = "..")]` = 缺省值。
/// - `#[arg(rest)] s: String`:收集剩余文本(按词重拼);加 `#[arg(rest, raw)]` 则保真
///   原文空白/换行,且旗标只认前导。
/// - `#[arg(long)]` / `#[arg(long = "name")]`(可加 `short = 'c'`):`--name value` 选项。
/// - `#[arg(flag)]`(可加 `long` / `short`):布尔旗标。
///
/// 元素参数(从消息的非文本段按类型取,顺序保留):
/// - `#[arg(image | record | video | at | reply | face)]`:一个该类型元素。必填缺失 ⇒
///   `ArgError::Missing`;`Option<T>` = 可选;加 `rest`(如 `#[arg(image, rest)] v: Vec<Media>`)
///   = 收集所有该类型元素。
/// - `#[arg(at_or_id)] u: Uin`:取一个 @ 提及元素,缺则取下一个文本词当 QQ 号
///   (群里 @、私聊里直接输号,一字段两种写法)。
///
/// # 类型约定
///
/// 文本字段类型需实现 `FromArg`(已为 String/整数/f64/bool/Uin 提供;受限选项枚举用
/// `#[derive(ArgEnum)]`)。元素字段类型固定:image/record/video → `Media`、at → `Uin`、
/// reply → `MessageId`、face → `String`。
///
/// # 示例
///
/// ```rust,ignore
/// #[derive(Args)]
/// struct GiveArgs {
///     #[arg(at_or_id)] target: Uin,       // @某人 或 裸 QQ 号
///     amount: u32,                         // 文本位置参数
///     #[arg(long, short = 'm')] memo: Option<String>, // --memo / -m
///     #[arg(rest)] note: String,           // 剩余文本
/// }
/// ```
#[proc_macro_derive(Args, attributes(arg))]
pub fn derive_args(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match args_derive::expand_args(input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// 为**无字段变体**的枚举生成 `FromArg`,按变体名小写匹配,用于命令里的 `on|off`、
/// `原曲|哼唱` 这类受限选项。
///
/// 匹配大小写不敏感(`enum Mode { On, Off }` 匹配 `"on"`/`"ON"`/`"off"`)。
///
/// # 变体属性 `#[arg(..)]`
///
/// - `#[arg(rename = "x")]`:改用 `"x"` 作主名(替换默认的变体名小写)。
/// - `#[arg(alias = "y")]`:在主名之外再加一个别名(可多个)。
///
/// 枚举须全为单元(无字段)变体,否则编译报错。
///
/// # 示例
///
/// ```rust,ignore
/// #[derive(ArgEnum)]
/// enum Mode {
///     #[arg(rename = "原曲", alias = "origin")] Original,
///     #[arg(rename = "哼唱")] Hum,
/// }
/// ```
#[proc_macro_derive(ArgEnum, attributes(arg))]
pub fn derive_arg_enum(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match args_derive::expand_arg_enum(input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// 为「命令头 + 命名类型化正则槽」结构体生成 `FromSlots`(`matcher()` 产出头匹配器,
/// `from_slots()` 从命名捕获投影出字段)。是 `#[derive(Args)]` 的兄弟,复用其 `option_inner`
/// 助手判断可选性。
///
/// 一个 `#[derive(Slots)]` 类型可同时用作 `#[command(slots = T)]` 的头匹配器与
/// `Slots<T>` 提取器的解析目标。
///
/// # 结构体级属性 `#[slots(..)]` —— 有序区块序列
///
/// 命令头是一串自由拼接的**区块**,顺序即声明序:
/// - `lit("查看")`:**固定块**(正则转义后原样匹配)。
/// - 裸标识符 `board`:**捕获块**,引用同名字段(其 `#[slot(..)]` 定来源/类型)。
/// - `sep = "…"`:块间分隔正则(默认 `\s*`,容忍可选空白);`usage = "…"`:parse-miss 回贴串。
///
/// 不写任何固定块/字段引用时,序列退化为「字段声明顺序」(简单结构体无需写序列)。
///
/// # 字段属性 `#[slot(..)]`(每字段三选一,必给其一)
///
/// - `#[slot(re = r"(\d+)")]`:原始正则片段。**单值字段须恰含一个捕获组**(无组则自动外包一层);
///   多选请用 `(a|b)` 而非 `(a)|(b)`——后者是两组,只会绑定第一组(另一支命中时取到空、误判缺失)。
///   tuple 字段(如 `Option<(u8, u8)>`)则须含与元素同数的内捕获组,各组分别经 `FromArg` 解析。
/// - `#[slot(union = ["原曲", "哼唱"])]`:字面量交替 `(原曲|哼唱)`,经 `SlotValue` 解回。
/// - `#[slot(tail)] q: Option<String>`:贪婪尾 `([\s\S]*)` 收剩余文本;也可收
///   `Tail<…>` 形态的原始多模态尾。tail 字段不能是 tuple。
///
/// 字段类型 `Option<T>` ⇒ 该捕获块外包为可选 ⇒ 缺省时得 `None`。重名槽 / 序列里漏引用某字段 /
/// 引用未声明字段 = 编译报错。`#[command(slots = T)]` 会据「固定块 × 各必填 union 块」的笛卡尔积
/// 生成具体命令词(`COMMAND_WORDS`)→ help 自动枚举。
///
/// # 示例
///
/// ```rust,ignore
/// #[derive(Slots)]
/// #[slots(lit("查看"), board, lit("榜"), scope)]
/// struct ViewRank {
///     #[slot(union = ["金币", "等级", "发言", "签到"])] board: String,
///     #[slot(union = ["全局", "全站"])] scope: Option<String>,
/// }
/// // 头匹配 查看(金币|等级|发言|签到)榜(全局|全站)?;help 枚举出 查看金币榜 / 查看等级榜 / …
/// ```
#[proc_macro_derive(Slots, attributes(slots, slot))]
pub fn derive_slots(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match slots::expand_slots(input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// `#[derive(Slots)]` 的内联函数式糖:就地求值出一个 `Matcher`,免去显式声明结构体。
///
/// 展开为一个块表达式:内部合成一个等价于 `#[derive(Slots)]` 的匿名结构体及其
/// `FromSlots`,并求值为 `<该结构体>::matcher()`。因此 `matcher!{}` 是**纯头匹配器**糖;
/// 若还需把捕获投影成字段(`Slots<T>` 提取器),仍要写具名的 `#[derive(Slots)]` 类型。
///
/// # 语法
///
/// `matcher! { lit("…"), <field>: <ty> = <src>, …, sep = "…", usage = "…" }`(同款区块序列),
/// 其中 `<src>` 为:
/// - `re("…")`:原始正则片段。
/// - `union("a", "b")`:字面量交替。
/// - `tail`:贪婪尾。
///
/// `lit("…")` 是固定块;字段块就地声明类型与来源;`sep` / `usage` 可选;`Option<ty>` 表示可选块。
///
/// # 示例
///
/// ```rust,ignore
/// let m = matcher! {
///     lit("查询"),
///     id: Option<u64> = re(r"(\d+)"),
///     mode: Option<String> = union("详细", "简略"),
/// };
/// ```
#[proc_macro]
pub fn matcher(input: TokenStream) -> TokenStream {
    let m = parse_macro_input!(input as MatcherMacro);
    slots::expand_matcher_macro(m).into()
}
