# 0010. Enforce a line-coverage floor via cargo-llvm-cov, not template-fastapi's 95%

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
instance's own suite actually reaches — deliberately a different, smaller
number than the Python original's 95%, justified by the
compiler-enforced guarantees above rather than picked by cargo-culting
template-fastapi's figure. (This file keeps its original `0010-80-...`
filename so existing links stay valid; the number in the *title* is the
one to trust.) The floor catches the failure mode a
coverage gate exists for (a whole function, branch, or error path with
*zero* exercised lines slipping in unnoticed) without demanding
diminishing-returns coverage of, e.g., every `Debug`/`Clone` derive or
trivial getter.

**The floor was 80% while Tier A (unit tests) was the only tier
collected. It is now 92%** (`cargo llvm-cov --fail-under-lines 92`),
raised in place -- as this ADR's own Consequences section said it should
be -- once the integration tier landed (`docs/adrs/0016`'s `[lib]`/
`[[bin]]` split is what made a `tests/` tier possible at all). The
suite measured 93.48% of `src/` at the time of the change, against
87.78% before it; 92 leaves a small margin rather than pinning the gate
to the exact measurement, so an unrelated refactor that shifts a few
lines doesn't fail the build for no reason.

What is deliberately left uncovered, and why chasing literal 100% is not
worth it:

- `main.rs` -- a `#[tokio::main]` entry point that binds a socket and
  serves forever. It is thin *by design* (`docs/adrs/0016`): everything
  worth testing was moved into `lib.rs`, which is covered. Exercising
  what remains would mean starting the real process.
- `telemetry.rs::configure_logging` -- installs a global
  `tracing_subscriber`, which can only be done once per process, so a
  test asserting on it would break every other test's logging.
- The `Mode::Dev`/`Mode::Production` halves of `lib.rs`'s
  `build_state`/`build_health_registry` and `health::checks`'
  S3/OIDC/Redis checks: these connect to real backing services from a
  path that also calls `std::process::exit`-adjacent `expect`s on
  failure. The integration tier reaches the repository and event-bus
  behaviour behind them directly instead.
- `repositories::filtering`'s `FilterOp::Ne`/`Lt`/`Gt` arms in
  `hero_memory`/`hero_sea_orm`: deliberately unreachable through the
  HTTP layer today (see that module's own doc comment), kept because
  they complete the storage-agnostic vocabulary.

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
92`) with the same local/CI behavior as template-fastapi's `pytest
--cov`.

Harder: the floor is still a lower bar than template-fastapi's 95%, so a
reviewer comparing the two repos side by side needs the reasoning above,
not just the number, to see it isn't a shortcut. Raising it to 92 also
means the integration tier is now load-bearing for the gate: `cargo
llvm-cov` no longer passes without the devcontainer stack's Postgres and
MQTT services running, where the 80% Tier-A-only floor did.

`cargo-llvm-cov` also requires
the `llvm-tools-preview` rustup component (already available via the
pinned toolchain's `profile = "default"`) and its own one-time `cargo
install`, a second coverage toolchain contributors didn't need to reach
for `cargo test` alone. The Tier B (integration) revision this
section originally anticipated has now happened, in place, above. The
same should happen again if an e2e/perf tier is added.
