//! Shared server-side cache for Hero's `/stats` and `/predict` routes --
//! port of template-fastapi's `app/cache.py` (`docs/adrs/0024`).
//!
//! [`RedisStatsCache`] implements `craftainer_crud::stats_cache::StatsCache`
//! over the app's Redis (`REDIS_URL`, the same `redis` crate and
//! `ConnectionManager` the rate limiter uses); under `Mode::Mock` the crate's
//! `MemoryStatsCache` stands in. The cache is off unless
//! `STATS_CACHE_TTL_SECONDS` is set (`config::stats_cache_ttl`).
//!
//! Keys are `stats-cache:{version}:{resource}:{key}`, where `{version}` is
//! `APP_VERSION` (else `"unknown"`), so a canary release whose response shape
//! differs never reads the other version's entries. Invalidation ignores the
//! version: a write by either version drops both versions' entries.
//!
//! A Redis failure never fails a request: every method logs a warning and
//! degrades -- `get` to a miss, `set` and `invalidate` to a no-op (the TTL
//! bounds the staleness). The crate propagates whatever a `StatsCache`
//! returns, so this impl is where the degrade rule lives.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use craftainer_crud::events::EventBus;
use craftainer_crud::stats_cache::{CacheError, CachedStats, MemoryStatsCache, StatsCache};
use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::config::Mode;

const KEY_PREFIX: &str = "stats-cache";
const SCAN_COUNT: usize = 500;

/// Bounds a cache command so an unreachable Redis costs a request at most
/// this long before it degrades to a miss (shorter than the rate limiter's
/// 2 s: the cache is an optimisation, not a gate).
const REDIS_TIMEOUT: Duration = Duration::from_millis(500);

/// How long [`run_invalidation`] waits before resubscribing after a failure.
pub const INVALIDATION_RETRY: Duration = Duration::from_secs(5);

/// `APP_VERSION` if set and non-empty, else `"unknown"`.
pub fn app_version() -> String {
    std::env::var("APP_VERSION")
        .ok()
        .filter(|version| !version.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Escape Redis glob metacharacters in `value` for a `SCAN MATCH` pattern.
fn glob_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if matches!(ch, '*' | '?' | '[' | ']' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// `StatsCache` over Redis: entries expire after `ttl`.
pub struct RedisStatsCache {
    manager: ConnectionManager,
    ttl: Duration,
    version: String,
    prefix: String,
}

impl RedisStatsCache {
    /// Connect (auto-reconnecting `ConnectionManager`) to `redis_url`.
    pub async fn connect(
        redis_url: &str,
        ttl: Duration,
        version: impl Into<String>,
    ) -> Result<Self, redis::RedisError> {
        Self::connect_with_prefix(redis_url, ttl, version, KEY_PREFIX).await
    }

    /// As [`connect`](Self::connect) under a custom key prefix, so tests can
    /// keep their keys apart from real entries.
    pub async fn connect_with_prefix(
        redis_url: &str,
        ttl: Duration,
        version: impl Into<String>,
        prefix: &str,
    ) -> Result<Self, redis::RedisError> {
        let client = redis::Client::open(redis_url)?;
        let config = ConnectionManagerConfig::new()
            .set_connection_timeout(Some(REDIS_TIMEOUT))
            .set_response_timeout(Some(REDIS_TIMEOUT))
            .set_number_of_retries(1);
        let manager = ConnectionManager::new_with_config(client, config).await?;
        Ok(Self {
            manager,
            ttl,
            version: version.into(),
            prefix: prefix.to_string(),
        })
    }

    fn redis_key(&self, resource: &str, key: &str) -> String {
        format!("{}:{}:{resource}:{key}", self.prefix, self.version)
    }

    async fn try_invalidate(&self, resource: &str) -> Result<(), redis::RedisError> {
        let mut conn = self.manager.clone();
        let pattern = format!("{}:*:{}:*", self.prefix, glob_escape(resource));
        let mut cursor = 0u64;
        loop {
            let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(SCAN_COUNT)
                .query_async(&mut conn)
                .await?;
            if !keys.is_empty() {
                redis::cmd("UNLINK")
                    .arg(&keys)
                    .query_async::<usize>(&mut conn)
                    .await?;
            }
            if next == 0 {
                return Ok(());
            }
            cursor = next;
        }
    }
}

#[async_trait]
impl StatsCache for RedisStatsCache {
    async fn get(&self, resource: &str, key: &str) -> Result<Option<Value>, CacheError> {
        let mut conn = self.manager.clone();
        let raw: Result<Option<Vec<u8>>, _> = redis::cmd("GET")
            .arg(self.redis_key(resource, key))
            .query_async(&mut conn)
            .await;
        match raw {
            Ok(Some(bytes)) => match serde_json::from_slice(&bytes) {
                Ok(value) => Ok(Some(value)),
                Err(err) => {
                    tracing::warn!(
                        "stats cache entry is not valid JSON, treating as a miss: {err}"
                    );
                    Ok(None)
                }
            },
            Ok(None) => Ok(None),
            Err(err) => {
                tracing::warn!("stats cache get failed, treating as a miss: {err}");
                Ok(None)
            }
        }
    }

    async fn set(&self, resource: &str, key: &str, value: Value) -> Result<(), CacheError> {
        let mut conn = self.manager.clone();
        let result = redis::cmd("SET")
            .arg(self.redis_key(resource, key))
            .arg(value.to_string())
            .arg("EX")
            .arg(self.ttl.as_secs().max(1))
            .query_async::<()>(&mut conn)
            .await;
        if let Err(err) = result {
            tracing::warn!("stats cache set failed, entry not stored: {err}");
        }
        Ok(())
    }

    async fn invalidate(&self, resource: &str) -> Result<(), CacheError> {
        if let Err(err) = self.try_invalidate(resource).await {
            tracing::warn!("stats cache invalidate failed, falling back to the TTL: {err}");
        }
        Ok(())
    }
}

/// The cache `ttl` asks for: `None` when the cache is off, the crate's
/// in-process cache under `Mode::Mock`, Redis otherwise.
pub async fn build_stats_cache(
    mode: Mode,
    redis_url: &str,
    ttl: Option<Duration>,
) -> Result<Option<Arc<dyn StatsCache>>, redis::RedisError> {
    let Some(ttl) = ttl else {
        return Ok(None);
    };
    if mode == Mode::Mock {
        return Ok(Some(Arc::new(MemoryStatsCache::with_ttl(ttl))));
    }
    let cache = RedisStatsCache::connect(redis_url, ttl, app_version()).await?;
    Ok(Some(Arc::new(cache)))
}

/// Invalidate `stats`' resource on every CRUD event from `events`, until
/// `shutdown` is cancelled.
///
/// Every replica runs its own, so a write handled by any replica reaches
/// every replica's cache through the event stream. The subscription is
/// transient (clean MQTT session, nothing to orphan), so instead of replay
/// the resource is invalidated once on every (re)subscribe, which covers
/// whatever could not have been delivered while disconnected. A failing
/// subscription or a stream that ends is logged and retried after `retry`.
pub async fn run_invalidation(
    stats: CachedStats,
    events: Arc<EventBus>,
    event_resource: &str,
    retry: Duration,
    shutdown: CancellationToken,
) {
    loop {
        let subscribed = tokio::select! {
            () = shutdown.cancelled() => return,
            result = events.subscribe_transient(event_resource, None) => result,
        };
        match subscribed {
            Ok((_subscriber_id, mut stream)) => {
                invalidate_logged(&stats).await;
                loop {
                    let event = tokio::select! {
                        () = shutdown.cancelled() => return,
                        event = stream.next_event() => event,
                    };
                    if event.is_none() {
                        tracing::warn!("stats cache invalidation stream ended, resubscribing");
                        break;
                    }
                    invalidate_logged(&stats).await;
                }
            }
            Err(err) => {
                tracing::warn!("stats cache invalidation could not subscribe: {err}");
            }
        }
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(retry) => {}
        }
    }
}

async fn invalidate_logged(stats: &CachedStats) {
    if let Err(err) = stats.invalidate().await {
        tracing::warn!("stats cache invalidate failed: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_escape_escapes_redis_pattern_metacharacters() {
        assert_eq!(glob_escape("heroes"), "heroes");
        assert_eq!(glob_escape("a*b?c[d]e\\"), "a\\*b\\?c\\[d\\]e\\\\");
    }
}
