# 0011. Hand-roll a Redis-backed rate limiter instead of adopting `tower_governor`

## Status

Accepted

## Context

Tier B item 1 of `docs/plans/2026-09-tier-b-c-app-features.md` ports
template-fastapi's `app/rate_limit.py`: Redis-backed rate limiting on
Hero's mutating routes (create/update/delete) and `POST /mock/token`,
so the limit is shared across every process/replica rather than
counted per-worker. `rate_limit.py` builds this on `slowapi`
(`Limiter(key_func=get_remote_address, storage_uri=settings.redis_url)`),
applied per-route (`limiter.limit(...)`) and checked inline before the
handler body runs -- explicitly not global ASGI middleware, because
`rate_limit.py`'s own docstring notes `SlowAPIMiddleware` is a
`BaseHTTPMiddleware` subclass with the same response-body-splitting
problem `http_headers.py`'s `_SecurityHeadersMiddleware` was written to
avoid.

The natural axum/`tower` equivalent of `slowapi` is `tower_governor` (a
`tower::Layer` built on the `governor` crate). Its own docs describe
`governor`'s state store as an in-process `governor::state::keyed::
DefaultKeyedStateStore` -- a local `HashMap`-like structure whose "state
size" is logged from the process's own memory, with no Redis or other
shared-backend option. Adopting it as-is would mean each replica
enforces its own independent budget, which is a materially different
guarantee than `storage_uri=settings.redis_url` gives the reference
app, and is the open question this plan flagged before starting: check
whether a suitable `tower`-compatible crate has a *Redis-backed* (not
just in-memory) limiter before committing to `tower_governor` vs.
hand-rolling. It doesn't.

Separately, `tower_governor` (like any `tower::Layer`) attaches to a
whole `axum::Router::route`, and `controllers::heroes`'s `"/"` route
mounts every HTTP method (`get(...).post(...).patch(...).delete(...)`)
as one service -- a layer applied there would also rate-limit the read
path (`GET`), which neither `rate_limit.py` nor this plan wants.
Splitting the route into separate per-method sub-routers just to attach
a layer to a subset of them would be a bigger structural change than
the feature warrants.

## Decision

We will hand-roll a small `RateLimiter` (`src/rate_limit.rs`) rather
than adopt `tower_governor`:

- **Redis-backed via a fixed-window counter**, not `governor`'s
  in-process token bucket: `RateLimiter::check(scope, client_ip, limit,
  window_secs)` runs a small atomic Lua script (`INCR` the
  `ratelimit:{scope}:{client_ip}` key; `EXPIRE` it only on the first hit
  in a window) via `redis::Script` over a `redis::aio::
  ConnectionManager` -- the standard Redis fixed-window rate-limit
  pattern (what the `limits` library's default strategy, which
  `slowapi` builds on, does under the hood), atomic enough that
  concurrent requests from the same caller can't race past the limit.
- **Checked inline, at the top of each mutating handler body**
  (`create`/`update`/`delete_hero` in `controllers::heroes`,
  `mint_token` in `controllers::mock`), not as `tower` middleware --
  this sidesteps both the per-route-not-global concern `rate_limit.py`
  raised and the single-multi-method-route structural issue above,
  with no router restructuring needed.
- **Caller identity via `axum::extract::ConnectInfo<SocketAddr>`**
  (`main.rs` now serves via
  `into_make_service_with_connect_info::<SocketAddr>()`), matching
  `get_remote_address`'s role in the reference.
- **`Mode::Mock` gets an in-memory fallback** (`Backend::Mock`, a
  `Mutex<HashMap<String, (u32, Instant)>>` behind the same `check()`
  API) rather than requiring a real Redis connection --
  `NFR-0022-mock-mode-zero-infrastructure.md` already commits this app
  to booting with zero containers under `Mode::Mock`, and
  `rate_limit.py` itself has no equivalent concern since template-
  fastapi doesn't have a from-scratch mock mode of this kind.
- **Bounded Redis timeouts.** `redis::aio::ConnectionManager`'s own
  defaults are unbounded per-attempt (`connection_timeout`/
  `response_timeout` both `None`) with up to 6 retries -- discovered
  during this item's own test run, where an unreachable-Redis unit test
  hung for minutes instead of failing fast. `RateLimiter::connect` sets
  an explicit 2s connection/response timeout and 1 retry via
  `ConnectionManagerConfig`, so a Redis outage degrades to a fast
  `AppError::Internal` on the affected route rather than a hung
  request.
- **429s render as RFC 9457 problem-details**, via a new
  `AppError::TooManyRequests` variant (`src/problem_details.rs`) --
  mirrors `RateLimitExceeded` being a `starlette.exceptions.
  HTTPException` subclass the existing FastAPI exception handler
  already renders uniformly; no separate response-building code needed
  here either.

Default limits (`Settings::rate_limit_mock_token_per_minute` = 10,
`rate_limit_hero_write_per_minute` = 20, both env-overridable) mirror
`config.py`'s `rate_limit_mock_token`/`rate_limit_bulk_action` defaults
("10/minute"/"20/minute"), as bare per-minute counts rather than a
`"<count>/<period>"` expression -- this app's limiter has no expression
parser to reuse, and a single fixed 60s window is the only period this
phase needs.

## Consequences

The limiter is ~140 lines with no new runtime dependency beyond
features already latent in the pinned `redis` crate
(`connection-manager`, `script`) -- cheaper than adopting and then
partially fighting a `tower::Layer`'s per-route/per-method attachment
model. The trade-off: this is template-axum's own small piece of
rate-limiting logic to maintain (the Lua script, the fixed-window
semantics, the mock fallback) rather than a maintained upstream crate's
responsibility, and it implements exactly one algorithm (fixed window)
rather than `governor`'s more general token-bucket/GCRA. If a future
need calls for a smoother rate (bursty-traffic smoothing, not just a
hard per-minute cutoff), that's a reason to revisit, not evidence this
choice was wrong for what Tier B item 1 actually asks for.

Per `tests/README.md`'s "Don't reach a real Redis from a `src/` unit
test," the unit tier covers the mock backend's full behavior (limit
enforcement, per-scope/per-IP isolation, window reset) and the Redis
backend's fail-fast-on-unreachable path only -- the Redis success path
(`INCR`/`EXPIRE` actually executing against a live server) is left for
the not-yet-built integration tier, same gap `tests/README.md` already
documents for `RedisHealthCheck`.
