//! 最小但覆盖面广的入门示例:bot 作者经 `nagisa::prelude::*` 接触到的公开 API。涵盖一个
//! `command` + `mention_me` 命令、一个带插件元数据(`name` / `description`)的命令、一个普通的
//! `GroupMessage` handler、一个 `State<T>` 共享状态 handler、带类型的 `#[derive(Args)]` 解析
//! (文本位置参数 + 一个 `#[arg(at)]` 元素 + 一个 `-f` 标志),以及一个带可选 `#[arg(image)]`
//! 元素的 `#[derive(ArgEnum)]` 选项。用
//! `App::new().data(..).on(..).run_onebot(cfg, ctrl_c_shutdown())` 接起来。
//!
//! 运行(需要一个在线的 OneBot 端点):
//!   cargo run --example echo_bot

use nagisa::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};

// 1. 一个带 mention_me 的命令,把收到的文本回显回去。
#[command("echo", mention_me)]
async fn echo(reply: Reply, args: ArgText) -> HandlerResult {
    reply.text(format!("echo: {}", args.0)).await?;
    Ok(())
}

// 2. 一个携带插件元数据(name/description)的命令。
#[command("ping", name = "ping", description = "health check")]
async fn ping(reply: Reply) -> HandlerResult {
    reply.text("pong").await?;
    Ok(())
}

// 3. 只处理群消息的 handler(普通 `on`,靠提取器自我过滤)。
async fn group_only(msg: GroupMessage, bot: Bot) -> HandlerResult {
    let _ = (msg, bot);
    Ok(())
}

// 4. 一个 State<T> 共享状态 handler。
struct Counter {
    hits: AtomicU64,
}

#[command("count")]
async fn count(reply: Reply, state: State<Counter>) -> HandlerResult {
    // `State<Counter>` deref 到 `Counter`,所以字段可直接访问。
    let n = state.hits.fetch_add(1, Ordering::Relaxed) + 1;
    reply.text(format!("count = {n}")).await?;
    Ok(())
}

// 5. 在 segment 流上做带类型的 Args:有序位置参数 + 一个消息元素(@提及)+ 一个标志。
//    `转账 @张三 100 -f` → target=张三, amount=100, force=true。必填的 `#[arg(at)]` 意味着
//    只有出现 @ 时命令才触发。
#[derive(Args)]
struct Transfer {
    #[arg(at)]
    target: Uin, // 一个 @提及元素(必须是真的 @ 段;裸号/「@123」文本要用 #[arg(at_or_id)])
    amount: u64, // 有序文本位置参数
    #[arg(flag, short = 'f')]
    force: bool, // -f / --force
}

#[command("转账", "transfer", mention_me)]
async fn transfer(reply: Reply, args: Args<Transfer>) -> HandlerResult {
    let Transfer { target, amount, force } = args.0;
    reply.text(format!("transfer {amount} to {} (force={force})", target.0)).await?;
    Ok(())
}

// 6. 一个经 `#[derive(ArgEnum)]` 的选项枚举,外加一个可选的图片元素。
#[derive(ArgEnum, Debug)]
enum Switch {
    On,
    Off,
}

#[derive(Args)]
struct RepeatCfg {
    mode: Switch, // on|off(大小写不敏感)
    #[arg(image)]
    sample: Option<Media>, // 可选图片;缺省则为 None
}

#[command("repeat", mention_me)]
async fn repeat(reply: Reply, args: Args<RepeatCfg>) -> HandlerResult {
    let RepeatCfg { mode, sample } = args.0;
    reply.text(format!("repeat {mode:?}, has_image={}", sample.is_some())).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let shutdown = ctrl_c_shutdown();
    App::new()
        .data(Counter { hits: AtomicU64::new(0) })
        .on(group_only)
        .run_onebot(OneBotConfig::new("ws://127.0.0.1:8080/onebot/v11/ws"), shutdown)
        .await
}
