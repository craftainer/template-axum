# 0010. Enforce a line-coverage floor via cargo-llvm-cov, sized to the suite's own reach rather than copying template-fastapi's 95%

## Status

Accepted

## Context

template-fastapi gates both its `pytest` runs at 95% coverage of
`src/app` (`docs/nfrs/NFR-0020-test-coverage-gate.md` in that repo).
Phase 3 of this instance needs an equivalent automated floor, but Python
and Rust don't carry the same amount of runtime risk per line: a
Pydantic field with the wrong type, a `None` reaching a method that
assumes a value, an unhandled variant of a union type, or a typo'd
attribute name are all things Python's runtime discovers only when a
test (or a user) hits that line — which is a large part of *why*
template-fastapi's floor is set as high as 95%.

`rustc`'s borrow checker, exhaustive `match`, `Option`/`Result` instead
of `None`/exceptions, and a `cargo check`/`clippy -D warnings` pre-commit
gate that already runs on every commit close off that entire category of
bug before a test is ever written — a line of Rust that compiles has
already had far more verification applied to it than a line of Python
that merely parses. Copying 95% onto this instance would be treating a
Python-shaped number as if it measured the same kind of risk in Rust,
when a meaningful fraction of what the 95% figure buys in the Python
original is bought here by the compiler instead, for free, on every
build.

Two tools were viable: `cargo-tarpaulin` (ptrace-based instrumentation,
Linux-only, historically flaky under some codegen patterns) and
`cargo-llvm-cov` (built on LLVM's native source-based coverage
instrumentation, the same mechanism `rustc -C instrument-coverage` uses
directly, cross-platform, and the tool `rustc`'s own test suite uses).

## Decision

We will use `cargo-llvm-cov` and set the automated floor at what this
instance's own suite actually reaches, justified by the
compiler-enforced guarantees above rather than picked by cargo-culting
template-fastapi's figure — the point was never to land below the
Python original's 95% for its own sake, only to not treat that number
as the target just because it's what the reference repo uses. It
started well below 95% (80%, then 92%) while entire failure paths and
real-service branches were still uncovered by design; once those gaps
were closed on purpose (see below), the same "what the suite actually
reaches, minus a small margin" rule pushed the floor past 95% too. (This
file keeps its original `0010-80-...` filename so existing links stay
valid; the number in the *title* was never meant to be read as "80" --
see the Decision text's own numbers for what's current.) The floor
catches the failure mode a
coverage gate exists for (a whole function, branch, or error path with
*zero* exercised lines slipping in unnoticed) without demanding
diminishing-returns coverage of, e.g., every `Debug`/`Clone` derive or
trivial getter.

**The floor was 80% while Tier A (unit tests) was the only tier
collected, then 92%** once the integration tier landed (`docs/adrs/
0016`'s `[lib]`/`[[bin]]` split is what made a `tests/` tier possible at
all) -- the suite measured 93.48% of `src/` at the time of that change,
against 87.78% before it.

**It is now 97%** (`cargo llvm-cov --fail-under-lines 97`), raised again
once the integration tier was extended specifically to close every gap
that didn't have a documented reason to stay open: a real Keycloak
verification path (`tests/keycloak_oidc.rs`), a real Redis rate-limiter
backend (`tests/redis_rate_limiter.rs`), `DatabaseHealthCheck`'s happy
path against real Postgres (`tests/postgres_health_check.rs`), the
`heroes` migration's `down()` (`tests/postgres_migration_down.rs`),
`lib::build_event_bus`'s real-broker branch and a subscriber skipping an
undecodable MQTT payload (both in `tests/mqtt_events.rs`), plus direct
unit tests for every `FilterOp` arm and error/edge branch across
`controllers::crud_actions`/`crud_query`/`crud_stats`, the `heroes*`
sibling routers' bulk/get-by-id/delete paths, and `repositories::
hero_memory`'s filter/sort functions. The suite measured 98.06% of
`src/` at the time of this change; 97 leaves a small margin rather than
pinning the gate to the exact measurement, so an unrelated refactor that
shifts a few lines doesn't fail the build for no reason.

**It is now 99%** (`cargo llvm-cov --fail-under-lines 99`), raised again
after `docs/plans/2026-09-real-100-percent-coverage.md` re-examined every
item on the list below against what this devcontainer stack actually
provides (Postgres, Redis, MQTT, Keycloak *and* S3/RustFS are all live,
not just the four the gate previously depended on) and closed all but a
small, individually-justified remainder. The suite measured 99.37% of
`src/` at the time of this change; 99 leaves a small margin rather than
pinning the gate to the exact measurement, the same reasoning as every
earlier raise. Notably:

- `telemetry::configure_logging` -- moved out of "untestable" into its
  own integration-tier file (`tests/telemetry_configure_logging.rs`),
  since Cargo builds every `tests/` file as its own process, so calling
  it there once never collides with any other test's global
  `tracing_subscriber`.
- The `Mode::Dev` halves of `lib.rs`'s `build_state`/
  `build_health_registry`, and `health::checks`'s `S3`/`Redis`/`Oidc`
  checks' own happy paths: now exercised directly against the
  devcontainer stack's real services (`tests/dev_mode_wiring.rs`,
  `tests/s3_health_check.rs`, and additions to
  `tests/redis_rate_limiter.rs`/`tests/keycloak_oidc.rs`) -- the
  `expect()`s these paths also carry only fire on a *misconfigured*
  connection, which running against the live stack never exercises.
- `events.rs`' MQTT poll-error backoff/reconnect-limit branches: rather
  than risk the shared broker connection mid-suite (which would flake
  every other MQTT test running concurrently), `rumqttc`'s `EventLoop`
  is now wrapped behind a small `events::MqttPoll` trait
  (`poll(&mut self) -> Result<Event, ConnectionError>`), and a
  test-only fake returns a scripted sequence of errors -- both the
  subscriber-side (`EventStream::next_event`) and publisher-side
  (`drive_publisher_eventloop`) poll loops are covered this way. This is
  the one production type whose shape changed purely for testability
  (flagged as a separate go/no-go in the plan before it was done).
- `main.rs` -- no longer thin-by-design-and-therefore-unreachable:
  `axum::serve(..).with_graceful_shutdown(shutdown_signal)` was added
  (a real operational improvement on its own -- the process previously
  had no clean shutdown at all), and `tests/e2e.rs`'s `AppProcess::drop`
  now sends a real `SIGTERM` (falling back to `.kill()` only if the
  child doesn't exit within ~2s) instead of killing the child outright,
  so the process's normal-return path -- which is what flushes an LLVM
  coverage profile -- actually gets attributed. `tests/
  main_failure_modes.rs` separately covers `Settings::from_env`'s error
  branch (`MODE=bogus`) and `TcpListener::bind`'s `.expect()` (port
  already in use), letting the child exit **on its own** in both cases
  (a panic/`exit()` both run libc's `atexit` handlers) rather than being
  killed by `AppProcess`, plus the `shutdown_signal`'s `SIGINT` arm.
- `controllers::heroes`'s categorical-stats branch: the counting loop
  moved into `crud_stats::categorical_counts`, generic over any record
  type the same way `numeric_stat`/`numeric_fields` already are, and is
  unit-tested directly there with a local struct that *does* have a
  boolean field -- `heroes.rs`'s own call site (still zero iterations
  for Hero, which has none) is unaffected.
- `controllers::crud_stats::forecast`'s zero-denominator branch: the
  slope/intercept computation moved into its own pure `slope_intercept`
  function, unit-tested directly with a degenerate all-equal-`x` input
  `forecast` itself can never produce.
- `controllers::heroes_web`'s two error branches (`hero_crud.list`/
  `create` failing) and its one `unreachable!()` match arm: a
  fault-injecting `Repository` fake now covers the two error branches
  directly (`FaultyRepository` in `heroes_web.rs`'s own test module),
  and the `unreachable!()` is gone entirely -- `update` now calls a new
  `crud_actions::resolve_update_by_id`, the id-only half of
  `resolve_update` factored out, so its `match` no longer has a `Bulk`
  arm to be unreachable.
- A handful of `Ok(_) => panic!(...)` arms inside this tier's own test
  helpers became a plain `.expect_err(...)`, and the keep-alive-comment-
  skip branches in two SSE test helpers (`controllers::crud_events`'s
  and `controllers::heroes`'s) are now covered directly: the keep-alive
  interval those helpers wait on has a `#[cfg(test)]` override of a few
  milliseconds (the same pattern `REDIS_TIMEOUT` already used), so a
  dedicated test can wait past it before publishing the real event it
  asserts on.

What is still deliberately left uncovered, and why chasing the exact
remainder is not worth it:

- `events.rs`' and `controllers::crud_events`' JSON-encode-failure
  *dispatch* arms (the `Err(err) => ...` line itself, as opposed to the
  handler function it now calls, which *is* unit-tested): `CrudEvent` is
  a plain struct of owned scalars, so neither is reachable without a
  type that can't actually fail to serialize.
- `events.rs`' live-broker publish-failure `warn!` (`EventBus::publish`'s
  `Mqtt` arm): unlike the poll loops above, `AsyncClient::publish`
  itself isn't behind an injectable trait -- wrapping it too would mean
  changing a second production type's shape purely for testability,
  which the plan's own go/no-go scoped to the `EventLoop` abstraction
  only.
- A `let-else { panic!(...) }`/`{ break; }` arm apiece inside two of this
  tier's own test helpers (`crud_query.rs`'s filter-parsing test, and
  the two colocated `sse_frames`/`first_frames` SSE helpers' stream-
  ended-early fallback) -- present for the case where a test's own
  assumption breaks, not something the suite is meant to exercise.
- `controllers::heroes_web`'s `FaultyRepository` test fake: every method
  besides `list`/`create` (the two these tests actually exercise) is an
  `unimplemented!()` stub, by design (the plan's own words: "these two
  tests never call anything else") -- those stub bodies are counted as
  uncovered lines but are not lines any real behavior runs through.
- A small number of individually pre-existing, narrow branches
  elsewhere (`config.rs`, `oidc/mod.rs`, `rate_limit.rs`,
  `repositories/hero_sea_orm.rs`, `heroes_v1.rs`, `heroes_v1_xml.rs`,
  `heroes_xml.rs`, `controllers::mock`) that predate this pass and
  remain within the floor's margin -- each is a single defensive
  arm/branch of the same already-covered shape as its siblings, not a
  distinct untested code path.

Wired as a `pre-push`/`manual`-stage hook in `.pre-commit-config.yaml`
(`cargo-llvm-cov`), alongside the existing `cargo-check`/`cargo-audit`
hooks at that same stage — not `pre-commit`, since instrumented test
runs cost real time and shouldn't tax every commit, only what CI
(`prek run --all-files --hook-stage manual`) and an explicit local
`pre-push` both already run.

## Consequences

Easier: a real, CI-enforced signal exists that a change didn't add an
entirely untested code path, at a floor that reflects what Rust's own
type system already verifies rather than double-charging for it.
Contributors get one clear command (`cargo llvm-cov --fail-under-lines
99`) with the same local/CI behavior as template-fastapi's `pytest
--cov`.

Harder: raising the floor to 92, then 97, and then 99, means the
integration tier is now load-bearing for the gate: `cargo llvm-cov` no
longer passes without the devcontainer stack's Postgres, Redis,
S3/RustFS, MQTT and Keycloak services running, where the 80%
Tier-A-only floor did.

`cargo-llvm-cov` also requires
the `llvm-tools-preview` rustup component (already available via the
pinned toolchain's `profile = "default"`) and its own one-time `cargo
install`, a second coverage toolchain contributors didn't need to reach
for `cargo test` alone. The Tier B (integration) revision this
section originally anticipated has now happened, in place, above. The
same should happen again if an e2e/perf tier is added.
