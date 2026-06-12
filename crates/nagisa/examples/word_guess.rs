//! 群内猜词游戏:群里任何人都能作答;答错让本轮继续(一个 waiter、多个事件),"取消"/超时
//! 结束。演示中断引擎 —— `session.waiter().scope(Scope::peer(g))` + 一个 `recv::<GroupMessage>`
//! 循环 —— 外加一个 `#[event(Message, top)]` top 观察者(发言计数器):因为 top 层在 waiter 之前
//! 运行,所以即便会话进行中它也持续看到每条消息。
use nagisa::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

nagisa::plugin! { name = "猜词", category = Fun, key = "word_guess" }

static SPEECH_COUNT: AtomicU64 = AtomicU64::new(0);

#[event(Message, top, id = "speech_count")]
async fn speech_count(_m: MessageEvent) -> HandlerResult {
    SPEECH_COUNT.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

#[command("猜词", id = "start")]
async fn start(reply: Reply, ep: EventPeer, session: Session) -> HandlerResult {
    let group = ep.0;
    let answer = "答案";
    let waiter = session.waiter().scope(Scope::peer(group)).build();
    reply.text("开始猜词,在群里直接发答案吧(发\"取消\"结束)").await?;
    loop {
        match waiter.recv::<GroupMessage>(Duration::from_secs(60)).await {
            Some(m) => {
                let text = m.0.content.first().and_then(|s| s.as_text()).unwrap_or("");
                if text == "取消" {
                    reply.text("已结束").await?;
                    break;
                } else if text == answer {
                    // 链式构建器对标 `Msg`:先 @ 赢家,再道贺。
                    reply.msg().at(m.0.sender).text(" 答对了!").send().await?;
                    break;
                } else {
                    reply.text("再猜猜").await?;
                }
            }
            None => {
                reply.text(format!("超时,答案是 {answer}")).await?;
                break;
            }
        }
    }
    Ok(())
}

#[cfg(feature = "onebot")]
#[tokio::main]
async fn main() -> Result<()> {
    let shutdown = nagisa::ctrl_c_shutdown();
    App::new().run_onebot(OneBotConfig::new("ws://127.0.0.1:8080/onebot/v11/ws"), shutdown).await
}

#[cfg(not(feature = "onebot"))]
fn main() {
    eprintln!("enable the `onebot` feature to run this example");
}
