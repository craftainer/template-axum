# 0010. Enforce an 80% line-coverage floor via cargo-llvm-cov, not template-fastapi's 95%

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

We will use `cargo-llvm-cov` and set the automated floor at **80% line
coverage of `src/`** (`cargo llvm-cov --fail-under-lines 80`), not 95% —
a deliberate, smaller number than the Python original, justified by the
compiler-enforced guarantees above, not picked by cargo-culting
template-fastapi's figure. 80% still catches the failure mode a
coverage gate exists for (a whole function, branch, or error path with
*zero* exercised lines slipping in unnoticed) without demanding
diminishing-returns coverage of, e.g., every `Debug`/`Clone` derive or
trivial getter.

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
80`) with the same local/CI behavior as template-fastapi's `pytest
--cov`.

Harder: 80% is a lower bar than template-fastapi's 95%, so a reviewer
comparing the two repos side by side needs the reasoning above, not just
the number, to see it isn't a shortcut. `cargo-llvm-cov` also requires
the `llvm-tools-preview` rustup component (already available via the
pinned toolchain's `profile = "default"`) and its own one-time `cargo
install`, a second coverage toolchain contributors didn't need to reach
for `cargo test` alone. As this instance's Tier B/C (integration, e2e)
tests are added, this ADR's floor should be revisited against the wider
line count they'd newly cover — the number here is a Tier-A floor, not
necessarily the number the eventual multi-tier suite settles on.
