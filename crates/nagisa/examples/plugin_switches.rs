//! 端到端的分层插件开关,bot 作者怎么接它。
//!
//! 声明一个插件(`demo`),带两个触发器,外加一对管理命令(`/disable <key>` /
//! `/enable <key>`),它们经 `State<EnabledSet>` 句柄在当前 peer 翻开关。管理命令设了
//! `can_disable=false`,所以它们永远不会把*自己*关掉(你总能把东西再开回来)。
//!
//! 开关 key 遵循 `plugin!{}` + `#[command(id=…)]` 的层级:
//!   * 插件 master 就是它的 `key` —— 这里是 `demo`;
//!   * 每个触发器是 `"<plugin_key>.<id>"` —— 这里是 `demo.hello` / `demo.world`。
//!
//! 禁用 master 会压住其下每个触发器;禁用某个触发器不影响它的同级。
//!
//! 运行(需要一个在线的 OneBot 端点):
//!   cargo run --example plugin_switches --features onebot

use nagisa::prelude::*;

// 一个插件。它的 master 开关 key 是 "demo";下面两个触发器都挂在它下面。
nagisa::plugin! { name = "演示插件", category = Tool, key = "demo" }

#[command("hello", id = "hello")]
async fn hello(reply: Reply) -> HandlerResult {
    reply.text("hello!").await?;
    Ok(())
}

#[command("world", id = "world")]
async fn world(reply: Reply) -> HandlerResult {
    reply.text("world!").await?;
    Ok(())
}

// ── 管理命令 ────────────────────────────────────────────────────────────────
//
// `/disable <key>` 和 `/enable <key>` 在*当前* peer 翻一个开关。`can_disable=false` 保证管理
// 命令自身永远在线,这样 bot 永远不会被锁死、再也开不回东西。
//
// `<key>` 是任意开关 key:插件 master("demo")或某个触发器("demo.hello")。

#[command("/disable", id = "disable", can_disable = false)]
async fn disable(reply: Reply, es: State<EnabledSet>, peer: EventPeer, args: ArgText) -> HandlerResult {
    let key = args.0.trim();
    if key.is_empty() {
        reply.text("用法：/disable <key>").await?;
        return Ok(());
    }
    // `State<EnabledSet>` deref 到 `EnabledSet`,所以 `es.set(..)` 直接调通。
    es.set(key, Some(peer.0), false);
    // 引用触发命令,让确认消息挂在它下面。
    reply.reply(format!("已在本会话禁用：{key}")).await?;
    Ok(())
}

#[command("/enable", id = "enable", can_disable = false)]
async fn enable(reply: Reply, es: State<EnabledSet>, peer: EventPeer, args: ArgText) -> HandlerResult {
    let key = args.0.trim();
    if key.is_empty() {
        reply.text("用法：/enable <key>").await?;
        return Ok(());
    }
    es.set(key, Some(peer.0), true);
    reply.reply(format!("已在本会话启用：{key}")).await?;
    Ok(())
}

#[cfg(feature = "onebot")]
#[tokio::main]
async fn main() -> Result<()> {
    let shutdown = ctrl_c_shutdown();
    App::new()
        // 观察每一次开关改动 —— 在这里把快照持久化到你的存储。
        .on_switch_change(|key, peer, value| {
            println!("switch {key} {peer:?} -> {value}");
        })
        .run_onebot(OneBotConfig::new("ws://127.0.0.1:8080/onebot/v11/ws"), shutdown)
        .await
}

// 没有适配器 feature 时无可运行;用一个空操作的 `main` 让示例仍能编译(上面的 handler 也仍被
// 类型检查)。
#[cfg(not(feature = "onebot"))]
fn main() {
    println!("build with --features onebot to run this example against a OneBot endpoint");
}
