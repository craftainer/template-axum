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
  - `postgres_stats_predict.rs` — `/stats` and `/predict` over HTTP
    against live Postgres, including the multi-bucket `/predict` happy
    path that needs backdated `created_at` values.
  - `postgres_health_check.rs` — `health::checks::DatabaseHealthCheck`'s
    `SELECT 1` happy path against live Postgres. The `S3`/`Redis`/`Oidc`
    checks' own happy paths stay deliberately uncovered (`docs/adrs/
    0010`) since they'd need `lib::build_health_registry`'s real-backend
    wiring, not just a live service already in this tier.
  - `postgres_migration_down.rs` — the `heroes` migration's `down()`
    against real Postgres. `run_migrations` (`lib.rs`) only ever calls
    `up()`, so this is the only place `down()` runs at all.
  - `mqtt_events.rs` — the CRUD event stream against live Mosquitto.
    The only place NFR-0029's persistent-session replay guarantee can be
    verified at all (`Mode::Mock`'s bus has no broker). Also covers
    `lib::build_event_bus`'s real-broker branch directly (its
    `Mode::Mock` branch is unit-tested in `src/lib.rs`) and a subscriber
    skipping an undecodable payload on the wire -- something only a raw
    client bypassing `EventBus::publish` can put on the topic at all.
  - `redis_rate_limiter.rs` — `rate_limit::RateLimiter`'s real-Redis
    backend against live Redis: `RateLimiter::connect`'s success path and
    a check/increment/reject round trip. The unreachable-server failure
    path and the in-memory `Backend::Mock` half stay in `src/rate_limit.
    rs`'s own colocated unit tests, which need no live service.
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
`postgres_*.rs`/`mqtt_events.rs`), e2e-equivalent (`e2e.rs`, real
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
