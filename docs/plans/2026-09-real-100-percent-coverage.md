# Close every remaining src/ coverage gap, reach literal 100%

## Status

Draft

## Goal

`cargo llvm-cov` currently gates at 97% (`docs/adrs/0010-...md`), with an
itemized list of what's deliberately left uncovered because it looked
unreachable, process-lifecycle-bound, or not worth the risk to force.
Re-examining each item against what this devcontainer stack actually
provides (Postgres, Redis, MQTT, Keycloak, and S3/RustFS are *all* live,
not just the three the gate currently depends on) shows most of that
list is closable without weakening any safety net or writing a flaky
test. This plan closes each one, then raises the gate to
`--fail-under-lines 100` and rewrites ADR 0010/NFR-0023/NFR-0024 to
match. A couple of items are genuinely higher-risk refactors; those are
called out so the decision to do them (vs. leave a documented, much
shorter uncovered list) is explicit rather than assumed.

## Approach

Work in phases; re-run `cargo llvm-cov --fail-under-lines 97` (still
`CARGO_BUILD_JOBS=2` in this sandbox — see "Open questions") after each
phase and adjust the next one if a line count doesn't land where
expected. Every phase's tests go through this repo's existing
mock-vs-real-service split (`tests/README.md`): direct calls against the
real stack in `tests/`, no new containers started.

### Phase 1 — mechanical, no behavior change

1. **`telemetry::configure_logging`** (currently 0%, can only run once
   per process): add `tests/telemetry_configure_logging.rs`, a new
   integration-tier file. Cargo builds every file under `tests/` as its
   own process, so calling `template_axum::telemetry::configure_logging()`
   once there never collides with any other test. Assert it doesn't
   panic and that a `tracing::info!` call afterward doesn't either.

2. **`events.rs::publish`'s and `crud_events::event_frame`'s
   JSON-encode-failure branches**: both are a deliberate safety net
   (log and degrade, never crash a live publish/stream), not dead code
   to delete. Extract each branch's body into its own small function
   (`fn log_and_skip_publish(err: serde_json::Error)` /
   `fn error_frame(err: serde_json::Error) -> SseEvent`) and unit-test
   it directly by constructing a `serde_json::Error` the cheap way
   (`serde_json::from_str::<i32>("bad").unwrap_err()`) rather than
   trying to make `CrudEvent` itself unserializable (it can't be — it's
   owned scalars — which is exactly why this was marked unreachable
   before).

3. **Test-helper `Ok(_) => panic!(...)` arms** (`crud_actions.rs` and
   similar `expect_err`-shaped tests): replace each
   `match result { Ok(_) => panic!("..."), Err(e) => e }` with
   `result.expect_err("...")`. Same assertion, one stdlib line instead
   of a branch with an arm that (by construction) must never execute
   during a passing run.

4. **`crud_stats::forecast`'s zero-denominator branch**: structurally
   unreachable from `forecast` itself (`xs` is always `0..n` for
   `n >= 2`, whose variance is never zero) — not from the underlying
   math. Extract `fn slope_intercept(xs: &[f64], ys: &[f64]) -> (f64,
   f64)` as its own pure function and unit-test it directly, including
   a degenerate all-equal-`xs` case (e.g. `[5.0, 5.0, 5.0]`) that
   `forecast`'s own caller never produces but the guard still has to
   handle correctly (returns `0.0` slope, not `NaN`).

5. **`controllers::heroes`'s categorical-stats branch** (dead for Hero
   specifically — no boolean/enum field): move the counting loop
   (`for record in &records { if let Some(value) = boolean_value(...)
   { ... } }`) out of `get_stats` into `crud_stats.rs` as a generic
   `categorical_counts<T>(records: &[T], fields: &[&'static str],
   value_of: impl Fn(&T, &str) -> Option<bool>) -> Vec<CategoricalValueCount>`,
   mirroring how `numeric_stat`/`numeric_fields` are already
   resource-agnostic and separately tested there. Unit-test it in
   `crud_stats.rs` with a small local struct that *does* have a boolean
   field, decoupled from Hero. `heroes.rs`'s own call site (still zero
   iterations for Hero) and `boolean_value`'s existing direct unit test
   are unaffected.

6. **`heroes_web::update`'s `unreachable!()`** (an `?id=` update can
   never produce `UpdateOutcome::Bulk`, but the match still has to
   handle the variant since `resolve_update`'s return type is shared
   with callers that *do* support bulk): add
   `crud_actions::resolve_update_by_id(crud, id, owner_id, data) ->
   Result<R::Model, AppError>` — the existing `Some(id)` branch of
   `resolve_update`, factored out so it returns the model directly
   instead of wrapping it in `UpdateOutcome`. `heroes_web.rs` calls this
   instead; its `match` no longer has a `Bulk` arm to be unreachable.
   The four callers that genuinely support both id and bulk
   (`heroes.rs`, `heroes_xml.rs`, `heroes_v1.rs`, `heroes_v1_xml.rs`)
   keep using `resolve_update` unchanged.

7. **`heroes_web`'s two error branches** (`hero_crud.list`/`create`
   failing — unreachable against the in-memory repo these unit tests
   use, which never returns `Err`): add a small fault-injecting
   `Repository` fake in `heroes_web.rs`'s own `#[cfg(test)]` module
   (always returns `Err(RepoError::Backend("boom"))` from `list`/
   `create`, delegates everything else to `HeroMemoryRepository` or
   simply `unimplemented!()`s it since these two tests never call
   anything else), wire an `AppState` with it, and add two tests: `GET
   /heroes/form` and `POST /heroes/form` each return the mapped
   `AppError` response instead of panicking.

8. **`crud_events`'s SSE keep-alive-skip branch** (`first_frames`'s
   `if chunk.starts_with(':') { continue; }`, only reachable once
   axum's 15s keep-alive comment fires): give `KEEP_ALIVE_INTERVAL` a
   `#[cfg(test)]` override of a few milliseconds (a real, common
   pattern — same idea as `REDIS_TIMEOUT`'s existing test-friendliness,
   just via cfg instead of a parameter). Add one dedicated test that
   waits long enough for a keep-alive comment to arrive before the
   first real event and asserts `first_frames` still returns the real
   payload, not the comment. Existing tests using `first_frames` are
   unaffected (they already tolerate/expect real frames only).

### Phase 2 — exercise the Mode::Dev/real-service paths directly

Every service ADR 0010 called out as "connects to a real backend from
an `expect()`-guarded path" (Postgres, Redis, S3/RustFS, Keycloak) is
already a live, healthy dependency of this devcontainer stack — the
`expect()`s only fire on a *misconfigured* connection, which isn't what
these tests would exercise.

9. Add `common::dev_settings(oidc_audience: Option<String>) -> Settings`
   to `tests/common/mod.rs` (`Mode::Dev`, real stack coordinates — the
   same literal `tests/keycloak_oidc.rs` already builds locally) so
   every test below, plus the existing `keycloak_oidc.rs`, shares one
   fixture instead of duplicating the struct literal.

10. New `tests/dev_mode_wiring.rs`: call `template_axum::build_state`
    and `build_health_registry` directly with `common::dev_settings`
    against the real stack; assert every registered health check
    reports healthy, and that `build_router` on the resulting state
    serves `/health/live` and a real Hero create/list round trip
    through the Postgres-backed repository. This is what closes
    `lib.rs`'s `Mode::Dev` halves of both functions.

11. New `tests/s3_health_check.rs` and additions to
    `tests/redis_rate_limiter.rs`/`tests/keycloak_oidc.rs`: construct
    `S3HealthCheck`/`RedisHealthCheck`/`OidcHealthCheck` directly
    against the real RustFS/Redis/Keycloak and assert `check().await`
    reports healthy — the same pattern `tests/postgres_health_check.rs`
    already uses for `DatabaseHealthCheck`.

### Phase 3 — main.rs and process lifecycle

12. Add graceful shutdown to `main.rs`: `axum::serve(listener,
    app...).with_graceful_shutdown(shutdown_signal())`, where
    `shutdown_signal` awaits `tokio::signal::ctrl_c()` or a
    `SIGTERM` (`tokio::signal::unix::signal(SignalKind::terminate())`)
    — Linux-only is fine, this template never runs main.rs anywhere
    else. This is a real operational improvement independent of
    coverage (the process currently has no clean shutdown at all), and
    it's also what lets `main()` return normally instead of running
    forever until killed — normal return is what flushes the LLVM
    coverage profile.

13. Update `tests/e2e.rs`'s `AppProcess::drop`: send `SIGTERM` (via the
    `libc` crate, added as a pinned dev-dependency — `std::process::
    Child` has no signal-other-than-kill API) instead of `.kill()`,
    wait briefly for a clean exit, and fall back to `.kill()` only if
    it doesn't exit in time (so a stuck shutdown can never hang the
    suite).

14. Add two new small tests (in `tests/e2e.rs` or a new
    `tests/main_failure_modes.rs`) that spawn the compiled binary with
    `MODE=bogus` (exercises `Settings::from_env`'s error branch and
    `main`'s `std::process::exit(1)`) and with port 8000 already bound
    by the test itself (exercises the `TcpListener::bind` `.expect()`
    failure). Both let the child exit **on its own** (panic/`exit()`
    both run libc's `atexit` handlers, which is what flushes the
    profile) rather than being killed, so — unlike today — these paths
    actually get attributed.

### Phase 4 — optional, highest effort/risk: MQTT poll-error branches

15. `EventStream::next_event`'s backoff (`consecutive_errors < MAX`)
    and reconnect-limit (`consecutive_errors >= MAX`) branches only run
    when `eventloop.poll()` returns `Err(...)`, which only happens
    against a broken broker connection. The only way to hit this
    deterministically (not by actually killing the shared Mosquitto
    container mid-suite, which risks flaking every other MQTT test
    running concurrently) is dependency injection: wrap rumqttc's
    `EventLoop` behind a small trait (`poll(&mut self) -> Result<Event,
    ConnectionError>`), give `EventStream::Mqtt` a `Box<dyn ...>`
    instead of the concrete type, and add a test-only fake that returns
    a scripted sequence of `Err`s. This is the one item that changes a
    production type signature purely to make it testable, and adds a
    dynamic-dispatch indirection to the hot polling loop.
    **Recommendation: do it** (the indirection cost is negligible next
    to an MQTT round trip), but flagging it separately since it's the
    largest structural change in this plan — worth a explicit go/no-go
    rather than bundling it in silently.

### Phase 5 — close out

16. Re-run `cargo llvm-cov --fail-under-lines 97` after each phase
    above; fix any newly-surfaced single-line gaps (e.g. the new
    `shutdown_signal` function's own branches) before moving on.
17. Once the measured total is 100.00%, set
    `.pre-commit-config.yaml`'s `cargo-llvm-cov` hook and every
    reference to the figure (`tests/README.md`) to
    `--fail-under-lines 100`.
18. Rewrite `docs/adrs/0010-...md`'s "what is deliberately left
    uncovered" section — it should shrink to nothing, or to a single
    sentence if Phase 4 is skipped (in which case name exactly the two
    remaining branches and why, the same way the file already does for
    everything else). Update `docs/nfrs/NFR-0023-test-coverage-gate.md`
    and `docs/nfrs/NFR-0024-independent-test-tiers.md` to match, the
    same way the 92% → 97% change already did.

## Open questions

- **Phase 4 go/no-go**: confirm before starting it — it's the only item
  that changes a production type's shape (not just adding tests or
  extracting a pure helper) purely to make it testable.
- **Sandbox linker OOM**: this environment's `cargo llvm-cov` link step
  OOMs under full parallelism (8 cores/7.7GB) and needs
  `CARGO_BUILD_JOBS=2`; real CI runners likely don't need this, but
  worth confirming once before relying on the gate passing there
  unmodified.
- **S3 becomes gate-load-bearing**: after Phase 2, `cargo llvm-cov`
  additionally requires the devcontainer stack's RustFS/S3 service
  running (previously only Postgres/Redis/MQTT/Keycloak were), the same
  kind of change ADR 0010 already flagged going from 80%→92%→97%.
