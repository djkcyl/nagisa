//! bot 作者怎么接事件触发器:一个插件,其 handler 在*事件*(而非命令文本)上触发,受与
//! `#[command]` 触发器相同的分层开关门控。
//!
//! 声明一个插件(`group_events`),带两个事件触发器:
//!   * `#[event(MemberJoin, id = "welcome")]` —— 欢迎新成员;
//!   * `#[event(Nudge, id = "petpet")]` —— 被戳时戳回去。
//!
//! 开关 key 遵循 `plugin!{}` + `#[event(id=…)]` 的层级:
//!   * 插件 master 就是它的 `key` —— 这里是 `group_events`;
//!   * 每个触发器是 `"<plugin_key>.<id>"` —— `group_events.welcome` /
//!     `group_events.petpet`。
//!
//! 在某个 peer 禁用 `group_events` 会同时静默那里的两个事件 handler —— 开关对事件一视同仁,
//! 无需内联判断。`Nudge` 提取器还会丢掉 bot 自己发起的戳(无反馈环),所以 bot 绝不戳自己。
//!
//! 运行(需要一个在线的 OneBot 端点):
//!   cargo run --example event_plugin --features onebot

use nagisa::prelude::*;

// 一个插件。它的 master 开关 key 是 "group_events";下面两个事件触发器都挂在它下面。
nagisa::plugin! { name = "群事件", category = Fun, key = "group_events" }

// 在任意已启用群的每条入群通知上触发。`MemberJoin` 暴露入群的群 + 用户;我们 @ 新人欢迎他。
#[event(MemberJoin, id = "welcome")]
async fn welcome(j: MemberJoin, bot: Bot) -> HandlerResult {
    bot.send(
        &Peer::group(j.group.0),
        &[Segment::at(j.user), Segment::text(" 欢迎入群！")],
    )
    .await?;
    Ok(())
}

// 有人戳一戳 bot 时触发。`Nudge` 把好友/群的戳统一成 `{ peer, sender, receiver }`,并自我
// 过滤:bot 自己发的戳永远到不了这里,所以回复不会成环。
#[event(Nudge, id = "petpet")]
async fn petpet(n: Nudge, bot: Bot) -> HandlerResult {
    bot.send(&n.peer, &[Segment::text("戳回去！")]).await?;
    Ok(())
}

#[cfg(feature = "onebot")]
#[tokio::main]
async fn main() -> Result<()> {
    let shutdown = ctrl_c_shutdown();
    App::new()
        .run_onebot(
            OneBotConfig::new("ws://127.0.0.1:8080/onebot/v11/ws"),
            shutdown,
        )
        .await
}

// 没有适配器 feature 时无可运行;用一个空操作的 `main` 让示例仍能编译(上面的 handler 也仍被
// 类型检查)。
#[cfg(not(feature = "onebot"))]
fn main() {
    println!("build with --features onebot to run this example against a OneBot endpoint");
}
