# 0020. Use `craftainer-crud` and `craftainer-observability` instead of the app's own copies

## Status

Accepted

## Context

The generic CRUD layer (`src/generic/{models,views,repositories}`,
`src/generic/controllers/crud_*.rs`, `src/crud/`, `src/events.rs`), the
health checks and router (`src/health/`,
`src/generic/controllers/health.rs`) and the logging bridge
(`src/telemetry.rs`) were extracted from this app into two published crates
(`lib-crud-rs` ADR 0002 and `lib-observability-rs` ADR 0002, work items AX1
and AX2 of the templates, SDKs and UI plan). Keeping the copies would mean
two implementations that drift, and the crates are what `template-fastapi`'s
Rust twin is meant to share.

The crates were extracted with history, then corrected toward `lib-crud-py`
and `lib-observability-py` before their first release (AX2b, CR1-CR6, AX5,
AX7). So switching is not a pure move: some behaviour changed on purpose,
and this ADR records each difference with the reason it is accepted.

The first release pair could not be used together: every dependency is
pinned `=x.y.z`, and `craftainer-observability` 0.2.0 pinned `thiserror` and
`tokio` versions that `craftainer-crud` 0.2.0 did not. `craftainer-observability`
0.2.1 aligned them; this app pins `craftainer-crud =0.2.0` and
`craftainer-observability =0.2.1`, and its own `tokio`, `thiserror` and
`serde_json` pins follow the crates (`tokio =1.54.1`, `serde_json =1.0.154`;
`thiserror` is no longer a direct dependency). `sea-orm`'s `sqlx` and the
crates' `sqlx` resolve to one copy, 0.9.0.

## Decision

We use the published crates and delete the code that moved.

**What moved where**

| Was | Now |
| --- | --- |
| `src/generic/{models,views,repositories}` (`HasId`, `FieldError`, bulk/stats views, `Repository`, `ListOptions`, filtering, `dyn_repository!`) | `craftainer_crud::{models,views,repositories}` |
| `src/crud/` (`CrudService`) | `craftainer_crud::crud` |
| `src/generic/controllers/crud_{query,actions,stats,events}.rs` | `craftainer_crud::controllers::*` |
| `src/events.rs` (event bus, MQTT) | `craftainer_crud::events`, feature `mqtt` |
| `src/health/mod.rs`, `src/health/checks.rs` | `craftainer_observability::health`, features `postgres`, `redis`, `s3`, `http`, `testing` |
| `src/generic/controllers/health.rs` (`/live`, `/ready`, `HasHealthRegistry`) | `craftainer_observability::health::router` |
| `src/telemetry.rs` (`configure_logging`, OTLP log layer) | `craftainer_observability::logs`, feature `otel` |
| `tests/mqtt_events.rs`, `tests/postgres_health_check.rs`, `tests/s3_health_check.rs`, `tests/telemetry_configure_logging.rs` | the crates' own test suites |

Stays in the app: `AppError` (`src/problem_details.rs`), `config`, `oidc`,
`rate_limit`, `http_headers`, `db`, `migration`, `src/generic/controllers/
{audit,mock}.rs` with `HasSettings`/`HasRateLimiter` (they need the app's
configuration and authentication), and everything under `src/hero/`.

**Feature selection** is exactly what the app uses. `craftainer-crud`:
`mqtt` (`EventBus::connect`). `craftainer-observability`: `postgres` (the
database check, over the pool inside the SeaORM connection), `redis`, `s3`,
`http` (the OIDC check is `HttpHealthCheck` on the issuer's
`/.well-known/openid-configuration`), `otel` (OTLP log export) and `testing`.
`testing` gives `MockHealthCheck`, which `Mode::Mock` registers in non-test
code. The feature name is the crate's choice; `MockHealthCheck` is only ever
registered under `Mode::Mock`, as before.

**Wiring changes.** `AppError` gains `From<CrudError>` and a
`PreconditionFailed` (412) variant for the conversion to be total; no route
returns it yet (the conditional-request work wires it). `CrudError::Internal`
and `AppError::Internal` share the crate's redaction switch, which
`problem_details::configure_detail_redaction(Mode)` sets. `register` on the
health registry returns a `Result` (duplicate names), so
`build_health_registry` uses `expect` on its fixed, distinct names. Handlers
extract the raw query as `Vec<(String, String)>` (the crate's parsers take
ordered pairs) and ask the crate for the archived scope. `main` keeps the
`LoggingGuard` alive to the end and shuts it down after the server stops, so
the last OTLP batch is flushed.

**Logging when the exporter cannot be built.** The crate treats that as an
error (`Error::ExporterBuild`). The app keeps its old behaviour of not
failing to start: `main` falls back to `configure_logging_without_export()`
and logs a warning.

**Behaviour differences, each accepted.** These are the crates' corrections
toward the Python libraries; none changes the JSON shape or status of a
successful route response.

1. `?id=` combined with a filter is `422` on read, update and delete. Before,
   the filter was silently ignored, so `DELETE ?id=5&status=draft` deleted
   record 5. Refusing is safer for a mutation.
2. The bulk-action cap counts only the caller's own matching records. Before,
   other owners' rows counted toward `BULK_ACTION_MAX_MATCHED`, so a caller
   could be refused because of data they cannot touch.
3. `only_archived=true` lists soft-deleted rows only; it implies
   `include_archived`, and is `422` next to `include_archived=false`, on a
   single-record read, on a write and on `/stats`. A malformed
   `include_archived` is now `422` (it was `400` from the query extractor).
   `skip`/`limit` still fail as `400` when not numbers.
4. Query parameters are an ordered list: a repeated key is a repeated clause
   (`power_level=5&power_level=9` matches nothing), where a map kept only the
   last value. Errors come back in request order.
5. `/predict` regresses over bucket time, not bucket index: an empty bucket
   leaves a gap instead of shifting later points, matching `lib-crud-py`.
   Predictions change numerically for a series with gaps. `/stats` category
   counts come back in first-seen order instead of hash order, and float
   fields (none in Hero) are numeric stat fields.
6. Event-stream subscriber ids are minted by the server (128 random bits and
   an HMAC tag) and a client can only hand one back. A made-up id gets a new
   id and session instead of being adopted, because an id is a capability to
   resume someone's queued events. The number of simultaneous streams that
   needed a fresh id is capped (default 256); over it the route answers `429`
   with `Retry-After: 1`. A broker failure on subscribe is `503` with the
   broker's text hidden outside `MODE=dev` (before, it was shown).
7. Health: each check has a 5 second timeout and reports unhealthy
   (`"health check timed out"`) instead of stalling `/health/ready`; the Redis
   check keeps one connection; failure logs redact credentials. `/health/live`
   and `/health/ready` keep their bodies and status codes.
8. Logs: an empty `OTEL_EXPORTER_OTLP_ENDPOINT` counts as unset;
   `OTEL_SDK_DISABLED=true` and `OTEL_LOGS_EXPORTER=none` switch export off;
   the batch thread uses `reqwest`'s blocking client. The app's old exporter
   used the async client on the batch processor's own thread and exported
   nothing, silently. `tests/otlp_log_export.rs` runs the real binary against
   an in-process OTLP receiver and asserts a startup record and the final
   record (logged as the signal arrives, so only the flush at shutdown can
   carry it) arrive.

## Consequences

- About 5,000 lines of app code and tests are gone; the layer is maintained
  once and fixed for every dependent. The price is an exact-pin coupling:
  every dependency `=x.y.z` in a library forces dependents onto the same
  version, so the crates and this app must move together.
- `AppError` is a second error type beside `CrudError`. A handler that
  matched on an `AppError` variant for a repository failure now sees
  `CrudError` first and converts with `?`.
- **Event-stream resume across restarts and replicas.** The default subscriber
  secret is random per process, so an id issued by one instance does not
  verify on another or after a restart: the client gets a fresh session and no
  replay. With one replica that only shows after a restart. Running several
  behind a load balancer needs `EventBus::with_subscriber_secret` with one
  shared secret (at least 32 bytes), which needs an app setting (name,
  whether production requires it) that is not designed here; it is left as
  a follow-up.
- The crates add nothing to the data-model contract, so the Hero repositories
  only gained `only_archived` and the `Float` filter value.
- `.github/scripts/check_layering.py` lost its `crud`, `events`, `health`,
  `telemetry` and `generic::{models,views,repositories}` categories with the
  code; imports from the crates are `craftainer_*::` paths it does not check.
- Features still not wired (conditional requests, request IDs, stats cache,
  `/health/info`, OTel traces and metrics) are later items; the crates ship
  them, the app does not use them yet.
