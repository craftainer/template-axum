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

What is deliberately left uncovered, and why chasing literal 100% is not
worth it:

- `main.rs` -- a `#[tokio::main]` entry point that binds a socket and
  serves forever. It is thin *by design* (`docs/adrs/0016`): everything
  worth testing was moved into `lib.rs`, which is covered. Exercising
  what remains would mean starting the real process. (`tests/e2e.rs`
  does start it, but kills the child process rather than shutting it
  down, so its coverage profile never flushes -- see that file's own
  `AppProcess::drop`.)
- `telemetry.rs::configure_logging` -- installs a global
  `tracing_subscriber`, which can only be done once per process, so a
  test asserting on it would break every other test's logging. The
  OTLP-exporter-build failure path it also covers is unit-tested
  directly (`otlp_log_layer`, without calling `configure_logging`
  itself).
- The `Mode::Dev`/`Mode::Production` halves of `lib.rs`'s
  `build_state`/`build_health_registry`, and `health::checks`'
  `S3`/`Redis`/`Oidc` checks' own happy paths: these connect to real
  backing services from a path that also calls `std::process::exit`-
  adjacent `expect`s on failure. `DatabaseHealthCheck`'s happy path
  *is* covered (`tests/postgres_health_check.rs`) since Postgres is
  already this tier's own dependency and constructing one doesn't touch
  that `expect`-guarded wiring; `lib::build_event_bus`'s real-broker
  branch is similarly covered directly (`tests/mqtt_events.rs`) since
  the ADR's concern -- an `expect` on connection failure -- doesn't
  apply to it.
- `events.rs`' and `controllers::crud_events`' JSON-encode-failure
  branches: `CrudEvent` is a plain struct of owned scalars, so neither
  is reachable without a type that can't actually fail to serialize.
- `events.rs`' MQTT poll-error backoff/reconnect-limit branches: only
  reachable by actually breaking the broker connection mid-test, which
  the integration tier's other tests already avoid needing (they use
  the real broker's success path throughout).
- `controllers::heroes`'s categorical-stats branch: dead code for Hero
  specifically, which has no boolean/categorical field (see
  `boolean_value`'s own doc comment) -- the mechanism exists for a
  future resource that has one.
- `controllers::crud_stats::forecast`'s zero-denominator branch:
  structurally unreachable given `forecast`'s own bucket-index scheme
  (`0..n` for `n >= 2`), which can never produce equal x-values.
- `controllers::heroes_web`'s two error branches (`hero_crud.list`/
  `create` failing) and its one `unreachable!()` match arm: the
  in-memory repository these unit tests use cannot fail those calls at
  all, and the `unreachable!()` is exactly that.
- A handful of `Ok(_) => panic!(...)` arms inside this tier's own test
  helpers, and one keep-alive-comment-skip branch in an SSE test helper
  (`controllers::crud_events::tests::first_frames`) that would need a
  real ~15s wait for axum's keep-alive interval to fire -- present for
  the case where a test's own assumption breaks, not something the
  suite is meant to exercise.

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
97`) with the same local/CI behavior as template-fastapi's `pytest
--cov`.

Harder: raising the floor to 92, and then 97, means the integration
tier is now load-bearing for the gate: `cargo llvm-cov` no longer passes
without the devcontainer stack's Postgres, Redis, MQTT and Keycloak
services running, where the 80% Tier-A-only floor did.

`cargo-llvm-cov` also requires
the `llvm-tools-preview` rustup component (already available via the
pinned toolchain's `profile = "default"`) and its own one-time `cargo
install`, a second coverage toolchain contributors didn't need to reach
for `cargo test` alone. The Tier B (integration) revision this
section originally anticipated has now happened, in place, above. The
same should happen again if an e2e/perf tier is added.
