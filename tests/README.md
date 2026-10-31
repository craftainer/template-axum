# tests/

Rust's own test conventions split this differently than template-
fastapi's `tests/unit` + `tests/integration` + `tests/e2e` + `tests/perf`
directory tree, so this instance's four tiers live in different places:

- **unit** — a `#[cfg(test)] mod tests` block colocated at the bottom of
  the module it covers, inside `src/` itself (e.g. `src/views/hero.rs`'s
  validation rules are tested in `src/views/hero.rs`, not a separate
  file under here). This is the idiomatic Rust equivalent of "name a
  test file after the module it covers" — a colocated `mod tests` names
  itself by construction, and it's the only way to unit-test a private
  function/field (`crud::CrudService::repository`, `problem_details`'s
  `REDACT_INTERNAL_DETAIL` static) the way `tests/unit/test_config.py`
  reaches into `app.config`'s internals via Python's lack of real
  privacy. Run with `cargo test` (or `cargo test --lib` to skip doctest
  collection). No real external service, no network — see each module's
  own test block for the fakes/stubs it uses in place of one (an
  in-memory `Repository`, a stub `HealthCheck`, an unsigned mock-mode
  JWT). Following upstream Rust convention rather than departing from
  template-fastapi's own layout for a project-specific reason, so this
  particular choice doesn't need its own ADR.
- **integration** — this directory. Cargo's own "integration test"
  convention: a black-box binary per file, linked against the crate's
  public API only (`src/lib.rs` — the `[lib]`/`[[bin]]` split that makes
  this possible is `docs/adrs/0016`). Each file reaches the **real**
  devcontainer stack services (`.devcontainer/stack/`), never a
  container the suite starts itself:
  - `postgres_hero_repository.rs` — `repositories::hero_sea_orm` against
    live Postgres: what the operators actually *mean* (`__icontains`
    case folding, `__in` membership, NULL ordering, the archived-row
    visibility rule, bulk update/delete round trips), where the
    colocated unit tests stop at the SQL text SeaORM renders.
  - `postgres_hero_round_trips.rs` — how many statements Hero's writes send
    (`docs/adrs/0028`): `update`/`delete`/`update_many`/`delete_many` are one
    each, counted with sea-orm's metric callback, plus a concurrent
    update/delete race.
  - `postgres_stats_predict.rs` — `/stats` and `/predict` over HTTP
    against live Postgres, including the multi-bucket `/predict` happy
    path that needs backdated `created_at` values.
  - `postgres_db_pool.rs` — `db::connect` against live Postgres: the
    `DB_POOL_*` sizes and timeouts are on the real pool, and closing one
    handle closes every clone
    (`docs/adrs/0022`, `NFR-0031`).
  - `postgres_connect_database.rs` — `connect_database`'s
    `RUN_MIGRATIONS_ON_STARTUP` switch, on a throwaway database of its
    own (it sets `DATABASE_URL` for itself, so it is the only test in its
    binary).
  - `postgres_migration_down.rs` — the `heroes` migration's `down()`
    against real Postgres. `run_migrations` (`lib.rs`) only ever calls
    `up()`, so this is the only place `down()` runs at all.
  - `mqtt_event_bus.rs` — `lib::build_event_bus`'s real-broker branch
    against live Mosquitto: the app's wiring delivers a published event to
    a subscriber. The bus itself (replay, subscriber ids) is tested in
    `craftainer-crud` (`docs/adrs/0020`).
  - `otlp_log_export.rs` — OTLP log export through the real binary
    (`docs/adrs/0020`, `FR-0035`): an in-process OTLP receiver stands in
    for a collector (the stack has none), a startup record and the record
    logged at shutdown must reach it, and an exporter that cannot be built
    must fall back to JSON logs. Binds the fixed port 8000, like `e2e.rs`.
  - `health_info.rs` — `GET /health/info` over the mock-mode router:
    version and commit from `APP_VERSION`/`APP_COMMIT`, `null` when unset
    or empty (`docs/adrs/0026`). Needs no service; it sets the process
    environment, so it is the only test in its binary.
  - `analytics.rs` — product analytics (`docs/adrs/0029`, FR-0037,
    NFR-0033) over the mock-mode router and an in-process stand-in for
    Umami's `POST /api/batch` (never the real Umami): `hero.created`/
    `hero.deleted` after a successful write and none after a failed one,
    the GPC/DNT drop, `ANALYTICS_CONSENT=opt-in` with and without
    `Analytics-Consent: 1`, the `ANALYTICS_PROP_KEYS` allowlist. It sets
    the process environment, so it is the only test in its binary. `e2e.rs`
    spawns the binary with `ANALYTICS_BACKEND=none`, so that suite does
    not need Umami either.
  - `redis_rate_limiter.rs` — `rate_limit::RateLimiter`'s real-Redis
    backend against live Redis: `RateLimiter::connect`'s success path and
    a check/increment/reject round trip. The unreachable-server failure
    path and the in-memory `Backend::Mock` half stay in `src/rate_limit.
    rs`'s own colocated unit tests, which need no live service.
  - `redis_stats_cache.rs` — `cache::RedisStatsCache` and Hero's cached
    `/stats`/`/predict` against live Redis (and, for one test, MQTT):
    hit/miss, per-query entries, invalidation (across versions, past one
    SCAN batch, on a write event, through the broker), TTL expiry, and a
    simulated outage (a TCP proxy to Redis that the test cuts) that must
    degrade to a miss with no 5xx. Each test uses its own key prefix and
    removes its keys.
  - `keycloak_oidc.rs` — `oidc::OidcVerifier`'s real (non-`Mode::Mock`)
    verification path against live Keycloak: discovery + JWKS fetch (and
    its cache), a real signed token round trip, and the malformed-token/
    unrecognized-`kid`/wrong-audience rejection paths. `Mode::Mock`'s
    unsigned-token path stays in `src/oidc/mod.rs`'s own colocated unit
    tests. `e2e.rs`'s `MODE=dev` journey also exercises this path, but
    over a spawned child process that gets SIGKILLed at the end of the
    test (`AppProcess::drop`), so its coverage profile never flushes —
    this file, running in-process, is what actually counts toward the
    coverage gate.
  - `common/mod.rs` — shared fixtures. Not a test binary (Cargo treats a
    subdirectory `mod.rs` as a module, not a target).
- **e2e-equivalent** — `e2e.rs`, also in this directory (Cargo has no
  separate convention for this tier; it's still one black-box binary
  against the public API, just over a real socket instead of `tower::
  ServiceExt::oneshot`). Spawns the actual compiled `template-axum`
  binary (`env!("CARGO_BIN_EXE_template-axum")`) under `MODE=dev`, then
  again under `MODE=mock`, and drives each over real HTTP with
  `reqwest` — role-journey style, mirroring the reference's per-role
  `tests/e2e/` split (`viewer`/`editor`/`maintainer`/`detective`/
  `security` each get exactly the requests `FR-0015`/`FR-0033` grant
  them). `MODE=dev`'s tokens come from a real Keycloak Resource Owner
  Password Credentials grant against the devcontainer stack's test
  users; `MODE=mock`'s come from `POST /mock/token`. Both journeys run
  sequentially in one test function, in one process, since `main.rs`
  binds a fixed `0.0.0.0:8000` — see `e2e.rs`'s own module doc for why
  that rules out running them as two separate, Cargo-parallelizable
  test binaries.
- **perf** — `perf/` (not a Cargo target — see that directory's own
  `README.md`).

One file here doesn't belong to any of the four tiers above, because it
isn't testing the application at all: `check_layering.rs` exercises
`.github/scripts/check_layering.py` (the `check-layering` prek hook
enforcing `src/`'s module layering, `docs/adrs/0009`) against small
synthetic trees under the OS temp directory, never a real service and
never `src/` itself except for one regression-guard case. Follows
`.github/scripts/README.md`'s "Do: add a test in `../../tests/` for any
new logic here that isn't trivial."

### Isolation and test-only helpers

Every Postgres test runs in a schema of its own
(`common::IsolatedDb`, via a `search_path` connection option) with the
app's real migrations applied into it, so tests that assert on
whole-table aggregates run in parallel and share nothing with the dev
database. MQTT tests use a per-run topic/resource name for the same
reason.

`common::seed_hero_at` writes a Hero row with a caller-chosen
`created_at`. It is deliberately test-only and not reachable from any
production path: `HeroSeaOrmRepository::create` always stamps
`Utc::now()` (FR-0007), which is exactly why `/predict`'s multi-bucket
happy path had no end-to-end coverage before.

## Coverage gate

`cargo llvm-cov --fail-under-lines 99` measures `src/`'s line coverage
across the unit **and** integration tiers and fails below a 99% floor —
see `docs/nfrs/NFR-0023-test-coverage-gate.md` and
`docs/adrs/0010-80-percent-line-coverage-floor-via-cargo-llvm-cov.md`
for why this number rather than template-fastapi's 95%: Rust's type
system statically rules out a class of bug the Python floor is partly
there to catch at runtime, so the same number would be cargo-culted, not
justified. (The floor was 80% while the unit tier was the only one
collected, then 92% once the integration tier landed, then 97% once
that tier closed the Keycloak/Redis/Postgres/MQTT gaps; that ADR records
every raise and the small, individually-justified list of what stays
deliberately uncovered at 99%.)

Because the integration tier reaches real services, this command now
needs the devcontainer stack's Postgres, Redis, S3/RustFS, MQTT and
Keycloak running.

```bash
cargo llvm-cov --fail-under-lines 99
```

Wired into `.pre-commit-config.yaml` as `cargo-llvm-cov`, `pre-push`/
`manual` stage (same split as the existing `cargo-check`/`cargo-audit`
hooks) — so `prek run --all-files --hook-stage manual` (what CI runs)
enforces it, but a fast `pre-commit`-stage commit doesn't pay a full
instrumented test-suite run.

## All four tiers are built

`NFR-0024`: unit (colocated `mod tests`), integration (this directory's
`postgres_*.rs`, `otlp_log_export.rs`), e2e-equivalent (`e2e.rs`, real
Keycloak tokens included), and perf (`perf/`, against the built
`runner`-stage image). The integration tier's own tests still mint
`Mode::Mock` bearer tokens rather than reaching Keycloak (FR-0017,
unchanged) — real Keycloak tokens only enter this directory via `e2e.rs`
now, which is a deliberate split: the integration tier tests the
repository and HTTP handlers, not token validation (which `oidc`
unit-tests directly), while the e2e tier's whole point is exercising the
real auth path end to end.

## Do

- Put a test that needs a real backing service in this directory, as its
  own `tests/<name>.rs` binary, and give it an isolated schema/topic —
  never a shared one.
- Add a new module's unit tests as a `#[cfg(test)] mod tests` block at
  the bottom of that module's own file, matching every existing module
  under `src/`.
- Reach for a fake/stub (an in-memory `Repository`, a stub
  `HealthCheck`, `Mode::Mock`'s unsigned-JWT path) instead of a real
  external service — that's what keeps this tier fast and gate-worthy.
- Serialize any test that mutates process-wide state (env vars via
  `Settings::from_env`, the `REDACT_INTERNAL_DETAIL` static) behind a
  `static ... Mutex<()>` guard, matching `src/config.rs`'s `ENV_LOCK` --
  Rust's test harness runs tests in parallel by default.

## Don't

- Reach a real Postgres/Redis/S3/Keycloak from a `src/` unit test — that
  belongs in this directory's integration tier.
- Start a container from a test. Use the devcontainer stack's own
  already-running services (see the root `README.md`).
- Depend on test execution order or leak state between tests (a set env
  var, a mutated static) without a lock guard.
