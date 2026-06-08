//! 跨会话的 token/bind:用户在群里发 `绑定` 拿到一个一次性 token,再私聊把它发给 bot 完成绑定。
//! 两半是各自独立的分发事件,经一个共享的 `Rendezvous<String, Uin>` 关联(群里签发、私聊认领)——
//! 没有挂起的任务,能熬过长时间间隔。默认的 `Rendezvous<String, Uin>` 由 `App::new` 自动提供,所以
//! handler 的 `State<Rendezvous<..>>` 不用自己注册就能用。
use nagisa::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

nagisa::plugin! { name = "账号绑定", category = User, key = "bind" }

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// 群:签发一个映射到发送者的短 token,5 分钟内有效。
#[command("绑定", id = "issue")]
async fn issue(reply: Reply, s: Sender, pend: State<Rendezvous<String, Uin>>) -> HandlerResult {
    let token = format!("BIND{:04}", COUNTER.fetch_add(1, Ordering::Relaxed) % 10000);
    pend.issue_with_ttl(token.clone(), s.0, Duration::from_secs(300));
    reply.text(format!("私聊我发送 {token} 完成绑定(5 分钟内有效)")).await?;
    Ok(())
}

/// 私聊:发来一个 `BINDxxxx` token 认领待定的绑定。`PrivateMessage` 把它限定在私聊;`Command`
/// 携带匹配到的 token。
#[command(regex = r"^BIND\d{4}$", id = "claim")]
async fn claim(
    _pm: PrivateMessage,
    cmd: Command,
    pend: State<Rendezvous<String, Uin>>,
    reply: Reply,
) -> HandlerResult {
    match pend.claim(&cmd.0) {
        Some(issuer) => {
            // 真实 bot 里你会在这里持久化(issuer ↔ 这个私聊账号)的关联。
            reply.text(format!("绑定成功:已关联群账号 {}", issuer.0)).await?;
        }
        None => {
            reply.text("token 无效或已过期,请重新在群里发送「绑定」").await?;
        }
    }
    Ok(())
}

#[cfg(feature = "onebot")]
#[tokio::main]
async fn main() -> Result<()> {
    let shutdown = nagisa::ctrl_c_shutdown();
    // `Rendezvous<String, Uin>` 由 `App::new` 自动提供(默认 TTL 5 分钟),所以
    // token/bind handler 的 `State<Rendezvous<..>>` 无需手动 `.data(..)` 就能用。要覆盖就自己
    // 注册一个。
    App::new()
        .run_onebot(OneBotConfig::new("ws://127.0.0.1:8080/onebot/v11/ws"), shutdown)
        .await
}

#[cfg(not(feature = "onebot"))]
fn main() {
    eprintln!("enable the `onebot` feature to run this example");
}
