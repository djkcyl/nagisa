//! 内置的、按需启用的限流器。一个令牌桶 `Middleware`：当某 peer——或全局桶——
//! 超过配置速率时返回 `Flow::Stop`（静默丢弃）。纯内存，无外部依赖。
//!
//! 经 `App::layer(RateLimit::per_peer(max, per))` 启用。
use crate::ctx::Ctx;
use crate::middleware::{Flow, Middleware, Next};
use async_trait::async_trait;
use nagisa_types::id::Peer;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// `RateLimit` 以什么为桶的键。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RateLimitScope {
    /// 每个可寻址 peer（群/好友/临时）一个桶。无 peer 事件放行。
    PerPeer,
    /// 所有事件共享一个桶。
    Global,
}

/// 可当 `Middleware` 用的令牌桶限流器。
#[derive(Clone)]
pub struct RateLimit {
    scope: RateLimitScope,
    /// 最大令牌数（突发容量）。
    max: f64,
    /// 补满 `max` 个令牌的窗口。
    per: Duration,
    /// 按 peer 的桶（PerPeer）或单个以 `None` 为键的桶（Global）。
    buckets: Arc<Mutex<HashMap<Option<Peer>, Bucket>>>,
}

#[derive(Clone, Copy)]
struct Bucket {
    tokens: f64,
    last: Instant,
}

impl RateLimit {
    /// 每个 peer 一个桶：每 `per` 窗口、每 peer 最多 `max` 个事件。
    pub fn per_peer(max: u32, per: Duration) -> Self {
        RateLimit {
            scope: RateLimitScope::PerPeer,
            max: max as f64,
            per,
            buckets: Arc::new(Mutex::new(HashMap::new())),
        }
    }
    /// 单个共享桶：每 `per` 窗口全局最多 `max` 个事件。
    pub fn global(max: u32, per: Duration) -> Self {
        RateLimit {
            scope: RateLimitScope::Global,
            max: max as f64,
            per,
            buckets: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 为 `peer` 消耗一个令牌。放行返回 `true`，被限流返回 `false`。
    /// `PerPeer` 下无 peer 的事件恒放行（没有可作键的东西）。
    pub fn check(&self, peer: Option<Peer>) -> bool {
        let key = match self.scope {
            RateLimitScope::PerPeer => match peer {
                Some(p) => Some(p),
                None => return true, // 无可寻址 peer → 不限流
            },
            RateLimitScope::Global => None,
        };
        let refill_per_sec = self.max / self.per.as_secs_f64();
        let now = Instant::now();
        let mut map = self.buckets.lock().expect("ratelimit buckets poisoned");
        let bucket = map.entry(key).or_insert(Bucket { tokens: self.max, last: now });
        // 按流逝时间补充令牌，封顶到 `max`。
        let elapsed = now.saturating_duration_since(bucket.last).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * refill_per_sec).min(self.max);
        bucket.last = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[async_trait]
impl Middleware for RateLimit {
    async fn handle(&self, ctx: Arc<Ctx>, next: Next<'_>) -> Flow {
        let peer = ctx.event().peer();
        if self.check(peer) {
            next.run(ctx).await
        } else {
            tracing::debug!(?peer, "rate limited; dropping event");
            Flow::Stop
        }
    }
}
