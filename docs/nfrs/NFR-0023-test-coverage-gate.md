# NFR-0023. Enforce a 97% automated line-coverage gate

## Status

Implemented

## Attribute

Quality / process.

## Description

`cargo llvm-cov` shall fail below 97% line coverage of `src/`, measured
over the unit **and** integration tiers (`cargo test`, which builds
both -- see `tests/README.md`). See
`docs/adrs/0010-80-percent-line-coverage-floor-via-cargo-llvm-cov.md`
for why 97%, not template-fastapi's 95% — a deliberate, justified
choice given Rust's compile-time guarantees, not a blind copy of the
Python original's figure. The floor was 80% while the unit tier was the
only one collected, then 92% once the integration tier's coverage
folded into the same run (the way template-fastapi's `tests/unit` +
`tests/integration` do), then 97% once that tier was extended to close
every gap that didn't have a documented reason to stay open. It may be
revisited again if an e2e/perf tier is added.

Because the integration tier reaches real services, this gate now
requires the devcontainer stack's Postgres, Redis, MQTT and Keycloak
services to be running — it is no longer satisfiable with zero
containers.

## Source

Developers; QA/CI. Configured via `.pre-commit-config.yaml`'s
`cargo-llvm-cov` hook; documented in `tests/README.md`'s "Coverage
gate" section.

## Verification

CI runs `prek run --all-files --hook-stage manual` (`.github/workflows/
checks.yml`), which includes the `cargo-llvm-cov` hook and fails the
build if coverage drops below 97%. Locally: `cargo llvm-cov
--fail-under-lines 97`.
