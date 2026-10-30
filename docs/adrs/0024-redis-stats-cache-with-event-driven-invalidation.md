# 0024. Cache Hero's `/stats` and `/predict` in Redis, invalidated by the CRUD event stream

## Status

Proposed

## Context

`GET /stats` and `GET /predict` read up to `MAX_HISTORY_RECORDS` rows and
aggregate them in the handler, so they are the most expensive reads and the
only ones whose answer is the same for every caller. `template-fastapi`
caches them in Redis (its `docs/adrs/0023`, `app/cache.py`) and this app has
to behave the same, with the same settings.

`craftainer-crud` 0.3.0 supplies the parts that are not Redis-specific (its
`stats_cache` module, crate ADR 0010): the `StatsCache` trait, `cache_key`,
`CachedStats::get_or_build` and an in-memory `MemoryStatsCache`. The crate
propagates a `CacheError` to the handler instead of treating it as a miss
(the same rule as `lib-crud-py`), so where a failure is turned into a miss is
the implementation's choice.

## Decision

- **`src/cache.rs`** holds `RedisStatsCache`, a `StatsCache` over the same
  `redis` crate and `ConnectionManager` the rate limiter uses (no new
  dependency). Keys are `stats-cache:{APP_VERSION|unknown}:{resource}:{key}`
  and entries are written with `SET ... EX`. The key itself comes from
  `craftainer_crud::stats_cache::cache_key` over the route kind, the response
  schema, the shared scope (reads are open to every read role, so there is no
  per-owner entry) and the normalised query (`include_archived` and `bucket`
  for `/stats`; `bucket`, `field` and `periods` for `/predict`). The version
  namespace means a canary release whose response shape differs never reads
  the other version's entries.
- **Off by default.** `STATS_CACHE_TTL_SECONDS` unset means no cache and the
  routes behave exactly as before; a set value must be a whole number >= 1
  (anything else exits 1 at startup). It is read by the standalone
  `config::stats_cache_ttl()`, like `GRACEFUL_SHUTDOWN_TIMEOUT_SECONDS`, not
  held in `Settings`: only the wiring reads it. `Mode::Mock` uses the crate's
  `MemoryStatsCache` with the same TTL; otherwise Redis (`REDIS_URL`).
- **A Redis failure never fails a request.** `RedisStatsCache` logs a warning
  and degrades: `get` returns a miss, `set` and `invalidate` do nothing (the
  TTL bounds staleness). This is where the crate's "errors propagate" rule
  and FastAPI's "degrade to a miss" meet: the crate propagates what the
  implementation returns, and this one returns `Ok`. Commands time out after
  500 ms so an unreachable Redis costs a request at most that much extra.
  A Redis that is down at *startup* still stops the process, as it does for
  the rate limiter.
- **Event-driven invalidation with a TTL net.** `lib::spawn_stats_cache_invalidation`
  starts `cache::run_invalidation` for the life of the process (only when the
  cache is on). It subscribes to the `heroes` event stream and drops every
  entry of the resource on each event; every replica runs its own, so a
  write handled by any replica reaches all caches. It also invalidates once
  on every (re)subscribe, and retries after 5 s when the subscription fails
  or the stream ends. It stops when `AppState.shutdown` is cancelled and
  `main` awaits it. The TTL bounds what this misses: a read racing a write, a
  replica that was disconnected, a Redis outage during invalidation.
- **Transient, not durable, subscription.** The task uses
  `EventBus::subscribe_transient`. FastAPI's task uses a durable MQTT session
  (`stats-cache-{hostname}`) to replay missed events; but since it also
  invalidates on every resubscribe, replay adds nothing, and a durable
  session per replica would leave one persistent session behind in the broker
  for every replica that ever existed. `EventBus::subscribe` also only
  resumes ids the bus minted itself, so a hostname-derived id would not be
  honoured. A clean session orphans nothing.
- **Resource name `heroes`.** The cache resource is `HERO_EVENT_RESOURCE`,
  the same name the events publish under. FastAPI uses `hero` there; it is
  an internal Redis key segment, never visible to a client, so the two
  implementations do not need to agree on it.
- **Handlers.** Only the v2 JSON `/stats` and `/predict` are cached (the v1
  compat, XML and web routers have no stats routes). When a cache is
  configured the response carries `X-Stats-Cache: hit|miss`, as FastAPI's
  does; with no cache there is no header. A `422` (too little history to
  forecast) is a build error and is never stored.

**Rejected:** a durable subscription per replica (above); a `OnceLock`
global instead of a field on `AppState` (hidden state tests cannot replace);
a second Redis client crate (`redis` is already a dependency).

## Consequences

- Hot `/stats` and `/predict` queries are served from Redis instead of
  re-aggregating up to `MAX_HISTORY_RECORDS` rows. Whether that is worth it
  on a given deployment is a measurement: run the load test with
  `STATS_CACHE_TTL_SECONDS` unset and then set (see `tests/perf/README.md`).
- A cached answer can be stale for up to the TTL when invalidation misses
  (see above); `X-Stats-Cache` makes that visible in a test or a trace.
- A Redis outage after startup makes the cached routes as slow as uncached
  ones plus up to 500 ms per Redis command, and logs a warning per failure.
- `AppState` gains a `hero_stats: CachedStats` field, which every test
  fixture that builds an `AppState` by hand must set (`CachedStats::uncached`).
- Tested in `tests/redis_stats_cache.rs` against the stack's Redis and MQTT:
  hit/miss, one entry per query, invalidation across app versions and past
  one SCAN batch, invalidation on a write event (in-memory bus) and through
  the broker, TTL expiry, and an outage simulated by cutting a TCP proxy in
  front of Redis.
