# NFR-0023. Enforce an 80% automated line-coverage gate

## Status

Implemented

## Attribute

Quality / process.

## Description

`cargo llvm-cov` shall fail below 80% line coverage of `src/`, measured
over the unit test tier (`cargo test`). See
`docs/adrs/0010-80-percent-line-coverage-floor-via-cargo-llvm-cov.md`
for why 80%, not template-fastapi's 95% — a deliberate, justified
choice given Rust's compile-time guarantees, not a blind copy of the
Python original's figure. As the integration/e2e tiers (`tests/README.md`'s
"What's not here yet") are added, their coverage should fold into the
same gate the way template-fastapi's `tests/unit` + `tests/integration`
combine into one run — this NFR's floor may be revisited once that
happens.

## Source

Developers; QA/CI. Configured via `.pre-commit-config.yaml`'s
`cargo-llvm-cov` hook; documented in `tests/README.md`'s "Coverage
gate" section.

## Verification

CI runs `prek run --all-files --hook-stage manual` (`.github/workflows/
checks.yml`), which includes the `cargo-llvm-cov` hook and fails the
build if coverage drops below 80%. Locally: `cargo llvm-cov
--fail-under-lines 80`.
