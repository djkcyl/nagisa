//! 命令触发匹配器 + MentionMe 预处理。
//!
//! **一个**触发匹配器:正则核心。`command([..])` 是字面量糖(编译成
//! `^(?:a|b)(?:\s|$)`),`regex(..)` 是原始正则。给定 `&Ctx` 决定消息是否命中并产出
//! [`ParsedCommand`]:
//! - **呼叫姿势预处理(所有命令)**:前导 `@self`(以运行期 `bot.self_id()` 为准)在匹配前
//!   统一剥掉、不进参数区——否则 `at`/`at_or_id` 元素参会把它当目标吃掉;**MentionMe** 额外
//!   要求 to_me,并在文本层再剥前导 `/`。
//! - **匹配**:正则跑在(剥离后的)首个文本段上,`command`=整段匹配(group0,trim),
//!   `captures`=捕获组。
//! - 命中后 `args` = 去掉命令文本前缀后的消息段(**保留非文本段**,如图片/回复),
//!   供 `Args<T>` 在段流上做有序 + 元素解析。
//! - **剩余内容要求**:无参命令**默认**([`Matcher::no_args`],`#[command]` 自动)只认
//!   「命令词」与「回复 / @bot / 空白 + 命令词」,再有别的内容整体不算命中,防「我的 xxx」
//!   这类日常说话误触发;显式严格([`Matcher::exact`],`#[command(.., exact)]`)更狠,
//!   整条消息只能是命令词本身。带参命令不受影响。
use crate::ctx::Ctx;
use nagisa_types::prelude::*;
use nagisa_types::segment::Segment;
use std::borrow::Cow;
use std::collections::HashMap;

/// 解析后的命令:命令字面量 + 剩余段 + 剩余纯文本 + 正则捕获组。
#[derive(Clone, Debug)]
pub struct ParsedCommand {
    /// 命中的命令文本(`command([..])` 为命中的词;`regex` 为整段匹配)。
    pub command: String,
    /// 命令之后剩余的消息段(**保留非文本段**,如图片/回复)。
    pub args: Vec<Segment>,
    /// `args` 的纯文本(对 `args` 跑 `MessageExt::extract_text`)。
    pub args_text: String,
    /// 正则捕获组(`command([..])` 糖无捕获组,故为空)。
    pub captures: Vec<String>,
    /// 命名正则槽:名 → `Some(text)`(命中) | `None`(缺省的可选槽)。
    /// **仅** `Matcher::slots` 构建时填充;`command`/`regex` 永远为空——故 `Captures`/`Args`
    /// 既有提取器字节级不受影响。键为 `Cow<'static, str>`,使命令式
    /// 运行时槽名也是一等。
    pub named_captures: HashMap<Cow<'static, str>, Option<String>>,
}

/// 一个 slot 在字面量头相对位置。`Pre`+字面量+`Post` 程序编成
/// `(?:..)?lit(?:..)?`,使字面量头两侧的可选修饰可表达。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flank {
    /// 槽即整个程序(无字面量头侧)。
    Whole,
    /// 槽在字面量头**之前**。
    Pre,
    /// 槽在字面量头**之后**。
    Post,
}

/// 一个有序 slot 规格。`Matcher::slots` 把一组 `SlotSpec` 编成**一条**锚定正则,
/// 命名槽成编号组,并保留 名→组下标 的映射。
///
/// `src` 内部已含 `names.len()` 个捕获组(单捕获槽 1 个;tuple 槽如 `(u8,u8)` 是 **2** 个,
/// 由 `#[derive(Slots)]` 在它**知道**两个内组的那一层产出——tuple 是多组关注,不是单捕获)。
/// `names[i]` 是第 i 个内组的注入键;`""` 表示字面量头
/// (无捕获组、不进 `named_captures`)。`name` 为 `Cow` 使运行时槽名无需泄漏 `'static`。
/// `src` 为 `String`(非 `&'static`),故 config 派生的字面量/union 经命令式路径可用。
#[derive(Clone, Debug)]
pub struct SlotSpec {
    /// `src` 内每个捕获组的注入键(按组顺序)。单捕获槽 1 个;tuple 槽 N 个;字面量头空。
    pub names: Vec<Cow<'static, str>>,
    /// 正则片段(FullMatch 转义;union 拼 `a|b`;tuple 含两内组)。命令式运行时构造亦可填入。
    pub src: String,
    /// `true` ⇒ 整段外包 `(?:..)?` ⇒ 结构体里的 `Option<T>`(缺省时其各内组皆为 `None`)。
    pub optional: bool,
    /// 槽相对字面量头的位置。
    pub flank: Flank,
}

impl SlotSpec {
    /// 命名的单捕获必填槽(`Flank::Whole`)。`src` 须恰含一个捕获组(或由 `Matcher::slots`
    /// 在无捕获组时自动外包一层)。
    pub fn named(name: impl Into<Cow<'static, str>>, src: impl Into<String>) -> Self {
        SlotSpec { names: vec![name.into()], src: src.into(), optional: false, flank: Flank::Whole }
    }
    /// 命名的单捕获可选槽(`(?:..)?` ⇒ `Option<T>`)。
    pub fn optional(name: impl Into<Cow<'static, str>>, src: impl Into<String>) -> Self {
        SlotSpec { names: vec![name.into()], src: src.into(), optional: true, flank: Flank::Whole }
    }
    /// 字面量头(无捕获组、不进 `named_captures`);`src` 应已转义。
    pub fn literal(src: impl Into<String>) -> Self {
        SlotSpec { names: Vec::new(), src: src.into(), optional: false, flank: Flank::Whole }
    }
}

/// 命中命令时随 `Ctx` 携带的显式用法串(`#[command(usage="…")]`)。
///
/// 刻意是 `ParsedCommand` 之外的**独立** ext——故 `ParsedCommand` 字段不变(既有测试零改动)；
/// 由 [`Matcher::match_event`] 在命中且 `usage` 存在时插入,与 `ParsedCommand` 同生命周期
/// (router 跑完 handler 后一并清除)。`Args<T>` 的 parse-miss 路径优先用它而非 dev 自动 hint。
#[derive(Clone, Debug)]
pub struct CommandUsage(pub String);

/// 命令触发匹配器:一组正则(任一命中即整体命中)+ MentionMe / to_me 开关。
#[derive(Clone, Debug)]
pub struct Matcher {
    /// 触发正则;按序尝试,第一个命中者产出结果。
    patterns: Vec<regex::Regex>,
    /// 是否启用 MentionMe 预处理(剥前导 @self + 前导 `/`)。
    pub mention_me: bool,
    /// 是否要求消息「to me」(私聊或 @self)才匹配。
    pub to_me_only: bool,
    /// 显式用法串(`#[command(usage="…")]`)；命中后作为独立 [`CommandUsage`] ext 随事件携带,
    /// 供 parse-miss 路径优先于 dev 自动 hint 回贴。`None` ⇒ 无。
    usage: Option<String>,
    /// `Matcher::slots` 的 命名槽→组下标 映射。空 ⇒ `command`/`regex`(无命名槽),
    /// 命中后 `named_captures` 也为空(既有提取器零影响)。
    slot_names: Vec<(Cow<'static, str>, usize)>,
    /// 命中后对剩余内容的要求(见 [`Matcher::no_args`] / [`Matcher::exact`])。
    strictness: Strictness,
}

/// 命中后对剩余内容的要求。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Strictness {
    /// 缺省(带参命令):剩余内容不影响命中,交给参数解析。
    #[default]
    Any,
    /// 无参命令的默认:除呼叫姿势(回复 / @bot / 空白)外不得有剩余内容。
    NoArgs,
    /// 显式严格(`exact`):整条消息只能是命令词本身,呼叫姿势也不算。
    Exact,
}

impl Matcher {
    /// 字面量命令词触发器(最常用)。`["echo","签到"]` 编译成 `^(?:echo|签到)(?:\s|$)`
    /// ——首词精确命中其一即触发,命中词为 `command`,其余进 `args`。
    ///
    /// 多词命令(如 `"git add"`)里的字面空格放宽成 `\s+`,容忍多空格——故 `"a b"` 也命中
    /// `"a  b"`。要匹配字面空格请改用 [`Matcher::regex`]。空 `alts` 永不命中。
    pub fn command<I, S>(alts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let alts: Vec<String> = alts.into_iter().map(Into::into).collect();
        let body = if alts.is_empty() {
            // 空集合:永不匹配(用一个不可能命中的模式)。
            r"\z\A".to_string()
        } else {
            // 转义字面量;多词命令(如 "git add")里的空白放宽成 `\s+`,容忍多空格。
            // (regex::escape 不转义空格,故替换的是字面空格。)
            alts.iter().map(|a| regex::escape(a).replace(' ', r"\s+")).collect::<Vec<_>>().join("|")
        };
        let pat = format!(r"^(?:{body})(?:\s|$)");
        let re = regex::Regex::new(&pat).expect("escaped command literals form a valid regex");
        Self {
            patterns: vec![re],
            mention_me: false,
            to_me_only: false,
            usage: None,
            slot_names: Vec::new(),
            strictness: Strictness::Any,
        }
    }

    /// 原始正则触发器;编译失败返回 `regex::Error`。捕获组进 `ParsedCommand.captures`。
    ///
    /// **切分语义(留意未锚定的写法)**:命中后 `ParsedCommand.command` = 整段匹配文本
    /// (捕获组 0,trim 后——含被捕获的"参数"部分),`args`/`args_text` = 匹配区间**之外**的
    /// 文本(匹配前缀 + 匹配后缀按字节区间拼接)。对 `^` 锚定的模式这就是「命令头 + 其后参数」
    /// 的直觉切分;**未锚定**的模式(如 `r"weather (\w+)"` 匹配 `"today weather beijing now"`)
    /// 则会得到 `command = "weather beijing"`、`args_text = "today  now"`(前缀保留、中间留缝)。
    /// 想要参数,优先从 `captures` 取捕获组,或给模式加 `^` 锚定。
    pub fn regex(re: &str) -> std::result::Result<Self, regex::Error> {
        Ok(Self {
            patterns: vec![regex::Regex::new(re)?],
            mention_me: false,
            to_me_only: false,
            usage: None,
            slot_names: Vec::new(),
            strictness: Strictness::Any,
        })
    }

    /// 第三种构造器(增量):一组有序 [`SlotSpec`] 编成**一条**锚定正则,
    /// 命名槽成编号组,保留 名→组下标 映射。
    ///
    /// 编译规则:
    /// - 整条正则 `^` 锚定(在首个文本段上跑),保证头部确定性匹配。
    /// - `Flank::Pre` 的槽排在最前、`Whole` 居中、`Post` 排在最后——故 `pre`+字面量头+`post`
    ///   程序编成 `^(?:pre)?lit(?:post)?`(字面量头两侧的可选修饰)。
    /// - 命名槽(`name != ""`)外包一层捕获组 `(...)`,其 1-based 组下标进 `slot_names`;
    ///   `optional` 再外包 `(?:...)?`。字面量头(`name == ""`)不额外包捕获组(不进映射)。
    ///
    /// 命中后 `match_event` 据 `slot_names` 把每个命名组(`Some(text)` | 缺省 `None`)填入
    /// `ParsedCommand.named_captures`,供 `Slots<T>` 类型化投影。
    pub fn slots(specs: Vec<SlotSpec>) -> std::result::Result<Self, regex::Error> {
        // Pre 槽在前、Whole 居中、Post 在后(稳定排序保留同 flank 内的声明顺序)。
        let mut ordered: Vec<&SlotSpec> = specs.iter().collect();
        ordered.sort_by_key(|s| match s.flank {
            Flank::Pre => 0u8,
            Flank::Whole => 1,
            Flank::Post => 2,
        });

        let mut pat = String::from("^");
        let mut slot_names: Vec<(Cow<'static, str>, usize)> = Vec::new();
        let mut group_idx = 0usize; // 已开的捕获组计数(1-based 命名时用)。
        for spec in ordered {
            if spec.names.is_empty() {
                // 字面量头:不命名,但其 src 内若含捕获组仍推进计数,保证下标对齐。
                group_idx += count_capturing_groups(&spec.src);
                if spec.optional {
                    pat.push_str("(?:");
                    pat.push_str(&spec.src);
                    pat.push_str(")?");
                } else {
                    pat.push_str(&spec.src);
                }
                continue;
            }
            // 命名槽:`src` 内的捕获组数须与 `names` 数一致;若 `src` 无捕获组而只命名一个,
            // 自动外包一层(便于 `SlotSpec::named("x", "\\d+")` 这种无组写法)。
            let inner = count_capturing_groups(&spec.src);
            let src = if inner == 0 && spec.names.len() == 1 {
                std::borrow::Cow::Owned(format!("({})", spec.src))
            } else {
                std::borrow::Cow::Borrowed(spec.src.as_str())
            };
            // 整段可选 ⇒ 外包 `(?:..)?`(不新增捕获组;其内各组缺省时为 None)。
            if spec.optional {
                pat.push_str("(?:");
                pat.push_str(&src);
                pat.push_str(")?");
            } else {
                pat.push_str(&src);
            }
            // 把整段里的每个捕获组依次绑到 names(按组顺序),记录其 1-based 全局下标。
            for name in &spec.names {
                group_idx += 1;
                slot_names.push((name.clone(), group_idx));
            }
            // 若 src 的捕获组多于 names(防御),把多出的也计入偏移。
            let bound = spec.names.len();
            if inner > bound {
                group_idx += inner - bound;
            }
        }

        let re = regex::Regex::new(&pat)?;
        Ok(Self {
            patterns: vec![re],
            mention_me: false,
            to_me_only: false,
            usage: None,
            slot_names,
            strictness: Strictness::Any,
        })
    }

    /// 附带显式用法串(`#[command(usage="…")]`)：命中后随 `ParsedCommand.usage` 携带,
    /// parse-miss 时优先于 dev 自动 hint 回贴。增量构造器,不影响既有匹配语义。
    pub fn with_usage(mut self, usage: impl Into<String>) -> Self {
        self.usage = Some(usage.into());
        self
    }

    /// 要求 @bot 呼叫(置 `to_me_only`),文本层再剥前导 `/`。前导 @self 段的剥离
    /// 对所有命令统一做,不属本开关。
    pub fn mention_me(mut self) -> Self {
        self.mention_me = true;
        self.to_me_only = true;
        self
    }

    /// 仅要求 to_me(不剥 @;私聊或 @self 时通过)。
    pub fn to_me(mut self) -> Self {
        self.to_me_only = true;
        self
    }

    /// 无参命令的**默认**剩余内容要求(`#[command]` 对没有 `Args<T>` 形参或参数规格为空的
    /// 命令自动调用,命令式注册无参命令请手动加):只认「命令词」与「回复 / @bot / 空白 +
    /// 命令词」(呼叫姿势的任意子集组合);剩余文本非空,或剩余段里有 回复 / @bot 以外的段
    /// (表情 / 图片 / @别人 / 语音…)都算内容,整体不算命中(静默放行,不进 parse-miss)
    /// ——「我的」是查数据,「我的 xxx」只是日常说话,不该触发。
    pub fn no_args(mut self) -> Self {
        self.strictness = Strictness::NoArgs;
        self
    }

    /// 严格模式(`#[command(.., exact)]` 显式开启):整条消息**只能是命令词本身**,
    /// 连 回复 / @bot 这些呼叫姿势都不算——比 [`no_args`](Self::no_args) 更狠。
    /// (与 `mention_me` 同用时 @self 在匹配前已被剥掉,不受此限。)
    pub fn exact(mut self) -> Self {
        self.strictness = Strictness::Exact;
        self
    }

    /// 判断某段是否是「@本 bot」。`self_id` 唯一真值来源是运行期 `bot.self_id()`。
    fn is_mention_self(seg: &Segment, self_id: Uin) -> bool {
        matches!(seg, Segment::Mention { user, .. } if *user == self_id)
    }

    /// 针对事件运行匹配。`None` = 不匹配(或非消息事件);`Some` = 命中并解析。
    pub fn match_event(&self, ctx: &Ctx) -> Option<ParsedCommand> {
        let msg = ctx.message()?;
        let self_id = ctx.bot().self_id();

        // —— 呼叫姿势预处理(**所有**命令):剥前导 @self(≤2 个),记录 to_me。——
        // 前导 @bot 是在叫机器人,不是参数——不剥的话 `at`/`at_or_id` 元素参会把它当目标
        // 吃掉(「@bot 转账 @张三 100」收款人解析成 bot)。一个前导 Reply 段(「回复 +
        // @bot + 命令」很常见)保留在 args 里,但不能挡住 @self 检测——故 @self 从 Reply
        // 之后开始看。mention_me 仅额外要求 to_me,并在文本层再剥前导 `/`。
        let mut to_me = msg.peer.scene != Scene::Group; // 私聊天然 to_me
        let reply_prefix = usize::from(matches!(msg.content.first(), Some(Segment::Reply { .. })));
        let mut stripped = 0usize;
        while stripped < 2 {
            match msg.content.get(reply_prefix + stripped) {
                Some(s) if Self::is_mention_self(s, self_id) => {
                    to_me = true;
                    stripped += 1;
                }
                _ => break,
            }
        }
        let segs: std::borrow::Cow<[Segment]> = if stripped == 0 {
            std::borrow::Cow::Borrowed(&msg.content[..])
        } else if reply_prefix == 0 {
            std::borrow::Cow::Borrowed(&msg.content[stripped..])
        } else {
            // 中间挖掉 @self、保留前导 Reply,需新建 Vec。
            let mut v = Vec::with_capacity(msg.content.len() - stripped);
            v.extend_from_slice(&msg.content[..reply_prefix]);
            v.extend_from_slice(&msg.content[reply_prefix + stripped..]);
            std::borrow::Cow::Owned(v)
        };
        let segs: &[Segment] = &segs;

        if self.to_me_only && !to_me {
            return None;
        }

        // 找到第一个 Text 段作为命令匹配目标(只操作首个文本段)。
        // 若 mention_me 启用则额外剥前导 `/`。
        let first_text_idx = segs.iter().position(|s| matches!(s, Segment::Text(_)));

        // 取出首个文本段的内容(trimmed-start,并视情况去掉前导 '/')。
        let (match_text_owned, first_text_idx) = match first_text_idx {
            Some(idx) => {
                let raw = match &segs[idx] {
                    Segment::Text(t) => t.as_str(),
                    _ => unreachable!(),
                };
                let trimmed = if self.mention_me {
                    raw.trim_start().trim_start_matches('/').trim_start()
                } else {
                    raw.trim_start()
                };
                (trimmed.to_string(), idx)
            }
            // 无文本段:只有非文本段(正则跑空串,如纯图片消息)。
            None => (String::new(), segs.len()),
        };
        let match_text: &str = &match_text_owned;

        for re in &self.patterns {
            if let Some(caps) = re.captures(match_text) {
                let m0 = caps.get(0);
                let whole = m0.map(|m| m.as_str().trim().to_string()).unwrap_or_default();
                // 剩余文本 = 匹配区间之外(匹配前 + 匹配后)——**无损**(按字节区间切,
                // 不靠 strip_prefix);命令型(`^` 锚定)时匹配前为空,即命令之后的部分。
                let remainder = match m0 {
                    Some(m) => {
                        let mut r = String::new();
                        r.push_str(&match_text[..m.start()]);
                        r.push_str(&match_text[m.end()..]);
                        r.trim().to_string()
                    }
                    None => match_text.to_string(),
                };
                let groups: Vec<String> =
                    caps.iter().skip(1).map(|g| g.map(|m| m.as_str().to_string()).unwrap_or_default()).collect();
                // 命名槽(`Matcher::slots`):据 slot_names 把每个命名组(Some(text) | 缺省 None)
                // 收进 named_captures。`command`/`regex` 的 slot_names 为空 ⇒ 此 map 为空。
                let mut named_captures: HashMap<Cow<'static, str>, Option<String>> =
                    HashMap::with_capacity(self.slot_names.len());
                for (name, gi) in &self.slot_names {
                    let val = caps.get(*gi).map(|m| m.as_str().to_string());
                    named_captures.insert(name.clone(), val);
                }
                let args = splice_args(segs, first_text_idx, &remainder);
                // 剩余内容要求:超出允许范围 ⇒ 不算命中(静默放行,不插任何 ext)。
                // NoArgs(无参命令默认)容忍呼叫姿势:回复 / @bot / 空白;Exact(显式严格)
                // 连呼叫姿势都不容忍——整条消息只能是命令词本身。
                let blocked = match self.strictness {
                    Strictness::Any => false,
                    Strictness::NoArgs => {
                        !remainder.is_empty()
                            || args.iter().any(|s| match s {
                                Segment::Reply { .. } => false,
                                Segment::Mention { user, .. } => *user != self_id,
                                Segment::Text(t) => !t.trim().is_empty(),
                                _ => true,
                            })
                    }
                    Strictness::Exact => {
                        !remainder.is_empty()
                            || args.iter().any(|s| match s {
                                Segment::Text(t) => !t.trim().is_empty(),
                                _ => true,
                            })
                    }
                };
                if blocked {
                    return None;
                }
                // 命中且有显式用法串 → 随事件携带(与 ParsedCommand 同生命周期),供 parse-miss 回贴。
                if let Some(u) = &self.usage {
                    ctx.insert_ext(CommandUsage(u.clone()));
                }
                return Some(self.make_parsed(whole, args, groups, named_captures));
            }
        }
        None
    }

    fn make_parsed(
        &self,
        command: String,
        args: Vec<Segment>,
        captures: Vec<String>,
        named_captures: HashMap<Cow<'static, str>, Option<String>>,
    ) -> ParsedCommand {
        let args_text = args.extract_text();
        ParsedCommand { command, args, args_text, captures, named_captures }
    }
}

/// 转义一段字面量为正则片段(`#[derive(Slots)]` 的 `full=`/`union=` codegen 用,
/// 免得业务/宏直接依赖 `regex` crate)。等价 `regex::escape`。
pub fn regex_escape(s: &str) -> String {
    regex::escape(s)
}

/// 数一个正则片段里**捕获组**的数量(忽略 `(?:..)`/`(?P<..)` 等非捕获 / 已转义括号)。
/// 用于 `Matcher::slots` 计算组下标偏移。不是完整正则解析器,但对槽 src(简单分组)足够,
/// 且即便误判也只影响下标对齐的诊断——实际匹配仍由 regex crate 负责。
fn count_capturing_groups(src: &str) -> usize {
    let bytes = src.as_bytes();
    let mut n = 0usize;
    let mut i = 0usize;
    let mut in_class = false; // 处于 [..] 字符类内(其中的 ( 不是分组)。
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2, // 转义:跳过下一个字符。
            b'[' if !in_class => {
                in_class = true;
                i += 1;
            }
            b']' if in_class => {
                in_class = false;
                i += 1;
            }
            b'(' if !in_class => {
                // `(?...` 为非捕获 / 断言 / 命名组开头——只有裸 `(` 是捕获组。
                if bytes.get(i + 1) == Some(&b'?') {
                    // 命名捕获 `(?P<..>` / `(?<..>` 仍是捕获组;其余 `(?:` `(?=` 等不是。
                    let is_named = matches!(bytes.get(i + 2), Some(&b'P') | Some(&b'<'));
                    if is_named {
                        n += 1;
                    }
                } else {
                    n += 1;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    n
}

/// 把命令之后的 `remainder`(已算好的剩余文本)拼回剩余段:
/// 输出 = `segs[..first_text_idx]`(命令前的非文本段,如 Reply)
///   + (若 `remainder` 非空)`Segment::Text(remainder)`
///   + `segs[first_text_idx+1..]`(命令后的所有其余段,如 Image、后续 Text)。
fn splice_args(segs: &[Segment], first_text_idx: usize, remainder: &str) -> Vec<Segment> {
    let mut args = Vec::new();
    // 前置非文本段(命令段之前,如 Reply)。
    args.extend_from_slice(&segs[..first_text_idx.min(segs.len())]);
    // 命令之后的剩余文本。
    if !remainder.is_empty() {
        args.push(Segment::text(remainder));
    }
    // 首文本段之后的所有段(保留,如 Image、后续 Text)。
    let after_start = (first_text_idx + 1).min(segs.len());
    args.extend_from_slice(&segs[after_start..]);
    args
}
