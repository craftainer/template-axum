//! Redis-backed per-route rate limiting -- port of `app/rate_limit.py`.
//!
//! Hand-rolled rather than via a `tower`-compatible crate: the natural
//! `tower::Layer` choice, `tower_governor`, wraps the `governor` crate's own
//! local, in-process token bucket with no Redis/shared-backend option (see
//! its own docs -- "storage size" is an in-memory `governor::state::keyed::
//! DefaultKeyedStateStore`, not a shared store), so a counter under it would
//! be per-process rather than shared the way `slowapi`'s
//! `storage_uri=settings.redis_url` is across every worker. `docs/adrs/
//! 0011-hand-rolled-redis-rate-limiter-over-tower-governor.md` records this.
//!
//! Checked inline, at the top of a handler body (`RateLimiter::check`),
//! never as `tower`/`axum::middleware` -- axum's `Router::route` mounts
//! every HTTP method for a path as one service (see `controllers::heroes`'s
//! single `"/"` route with `get(...).post(...).patch(...).delete(...)`), so
//! a `tower::Layer` applied to the route would rate-limit `GET` too;
//! checking inline, only in the handlers that need it, avoids restructuring
//! routing just to give reads and writes separate middleware stacks.
//!
//! `Mode::Mock` swaps in an in-memory counter instead of a real Redis
//! connection (`NFR-0022`: mock mode needs zero containers) -- the same
//! per-key fixed-window algorithm, just backed by a `Mutex<HashMap>`
//! instead of Lua+`INCR`+`EXPIRE`.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use redis::aio::{ConnectionManager, ConnectionManagerConfig};

use crate::problem_details::AppError;

/// Bounds how long a Redis connect/command attempt can take -- the
/// `ConnectionManager`'s own defaults are unbounded (`None`) for both
/// connect and per-command timeouts and retry up to 6 times with
/// exponential backoff, which turns "Redis is unreachable" into a
/// multi-minute hang instead of a fast `AppError::TooManyRequests`-adjacent
/// failure. A rate-limit check failing fast (as `AppError::Internal`) is
/// far better than a mutating route hanging the whole request.
const REDIS_TIMEOUT: Duration = Duration::from_secs(2);

/// Atomically increments `KEYS[1]`'s counter and sets its expiry only on the
/// first hit in a window -- the standard Redis fixed-window rate-limit
/// pattern (what the `limits` library's default strategy, which `slowapi`
/// builds on, does under the hood). `ARGV[1]` is the window length in
/// seconds.
const INCR_WITH_EXPIRY: &str = r"
local count = redis.call('INCR', KEYS[1])
if count == 1 then
    redis.call('EXPIRE', KEYS[1], ARGV[1])
end
return count
";

enum Backend {
    Redis(ConnectionManager),
    Mock(Mutex<HashMap<String, (u32, Instant)>>),
}

/// Shared rate limiter -- one instance built at startup (`main.rs`), held in
/// `AppState` behind an `Arc` and reused across every request.
pub struct RateLimiter {
    backend: Backend,
}

impl RateLimiter {
    /// Connect to Redis via a `ConnectionManager` (auto-reconnecting,
    /// cheaply cloned per-check rather than pooled) -- used outside
    /// `Mode::Mock`. Bounded per `REDIS_TIMEOUT` (see its own doc comment).
    pub async fn connect(redis_url: &str) -> Result<Self, redis::RedisError> {
        let client = redis::Client::open(redis_url)?;
        let config = ConnectionManagerConfig::new()
            .set_connection_timeout(REDIS_TIMEOUT)
            .set_response_timeout(REDIS_TIMEOUT)
            .set_number_of_retries(1);
        let manager = ConnectionManager::new_with_config(client, config).await?;
        Ok(Self {
            backend: Backend::Redis(manager),
        })
    }

    /// An in-memory stand-in for `Mode::Mock` -- see the module doc.
    pub fn mock() -> Self {
        Self {
            backend: Backend::Mock(Mutex::new(HashMap::new())),
        }
    }

    /// Check-and-increment `scope`'s counter for `client_ip`, erroring with
    /// `AppError::TooManyRequests` once more than `limit` requests land
    /// within a `window_secs`-second fixed window. `scope` namespaces
    /// independent limits (e.g. Hero writes vs. `POST /mock/token`) sharing
    /// the same backend.
    pub async fn check(
        &self,
        scope: &str,
        client_ip: IpAddr,
        limit: u32,
        window_secs: u64,
    ) -> Result<(), AppError> {
        let key = format!("ratelimit:{scope}:{client_ip}");
        let count = match &self.backend {
            Backend::Redis(manager) => {
                let mut conn = manager.clone();
                redis::Script::new(INCR_WITH_EXPIRY)
                    .key(&key)
                    .arg(window_secs)
                    .invoke_async::<u32>(&mut conn)
                    .await
                    .map_err(|err| AppError::Internal(format!("rate limiter: {err}")))?
            }
            Backend::Mock(store) => {
                let mut store = store.lock().unwrap();
                let now = Instant::now();
                let entry = store.entry(key).or_insert((0, now));
                if now.duration_since(entry.1) >= Duration::from_secs(window_secs) {
                    *entry = (0, now);
                }
                entry.0 += 1;
                entry.0
            }
        };

        if count > limit {
            return Err(AppError::TooManyRequests(format!(
                "rate limit exceeded: more than {limit} requests per {window_secs}s"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip() -> IpAddr {
        "127.0.0.1".parse().unwrap()
    }

    #[tokio::test]
    async fn mock_backend_allows_up_to_the_limit_then_rejects() {
        let limiter = RateLimiter::mock();
        for _ in 0..3 {
            limiter.check("test-scope", ip(), 3, 60).await.unwrap();
        }
        let err = limiter.check("test-scope", ip(), 3, 60).await.unwrap_err();
        assert!(matches!(err, AppError::TooManyRequests(_)));
    }

    #[tokio::test]
    async fn mock_backend_scopes_counters_independently_per_scope_and_ip() {
        let limiter = RateLimiter::mock();
        limiter.check("scope-a", ip(), 1, 60).await.unwrap();
        // A different scope for the same IP has its own budget.
        limiter.check("scope-b", ip(), 1, 60).await.unwrap();
        // A different IP under the same scope also has its own budget.
        let other_ip: IpAddr = "127.0.0.2".parse().unwrap();
        limiter.check("scope-a", other_ip, 1, 60).await.unwrap();
    }

    #[tokio::test]
    async fn mock_backend_resets_the_counter_once_the_window_elapses() {
        let limiter = RateLimiter::mock();
        limiter.check("reset-scope", ip(), 1, 0).await.unwrap();
        // window_secs=0 means "already elapsed" on the very next check.
        limiter.check("reset-scope", ip(), 1, 0).await.unwrap();
    }

    #[tokio::test]
    async fn connect_fails_fast_for_an_unreachable_server() {
        // Bounded by REDIS_TIMEOUT (see its own doc comment) -- without
        // that, connection refused on an unrouted port can otherwise take
        // several minutes to surface via the ConnectionManager's default
        // (unbounded, 6-retry) backoff. tests/README.md's "Don't" section
        // covers why this doesn't reach a real Redis instead: only the
        // failure path (no live dependency needed) belongs at this tier.
        let result = RateLimiter::connect("redis://127.0.0.1:1/0").await;
        assert!(result.is_err());
    }
}
