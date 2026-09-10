# NFR-0024. Keep the test suite split into independent tiers

## Status

Implemented

## Attribute

Quality / process.

## Description

Tests shall be split into independent tiers, each runnable without the
others being present or passing: **unit** (`#[cfg(test)] mod tests`
blocks colocated in `src/`, against fakes/stubs — an in-memory
`Repository`, a stub `HealthCheck`, `Mode::Mock`'s unsigned-JWT path —
no real Postgres/Redis/S3/Keycloak, no network), **integration** (real
backing services already running in the devcontainer stack, no mocks),
**e2e-equivalent** (`reqwest` against one live `MODE=dev` process and
one live `MODE=mock` process, role-journey style, mirroring
template-fastapi's per-role `tests/e2e/` subdirectories), and **perf**
(load test against the `runner`-stage image, never a `cargo run` dev
loop — same reasoning as template-fastapi's `docs/adrs/0010-locust-for-
load-testing.md`, tool choice recorded separately in this repo's own
`docs/adrs/0018`).

## Source

Developers; QA/CI. Documented in `tests/README.md`. Mirrors template-
fastapi's `NFR-0021-three-tier-test-suite.md`, widened to four tiers to
match this repo's own e2e/perf split (`docs/adrs/0010-...md` in that
repo's numbering).

## Verification

CI runs each tier as a distinct step/command; each tier's existence and
scope is reviewed against `tests/README.md`.

- **unit** + **integration**: `cargo test` / `cargo llvm-cov
  --fail-under-lines 99`, via `.pre-commit-config.yaml`'s
  `cargo-test`/`cargo-llvm-cov` hooks (`checks.yml`).
- **e2e-equivalent**: `tests/e2e.rs`, run by the same `cargo test`
  invocation above (it's an ordinary Cargo integration-test binary) —
  spawns the compiled binary under `MODE=dev` and `MODE=mock` in turn
  and drives each over real HTTP.
- **perf**: `tests/perf` (`cargo run -p template-axum-perf`), its own
  workspace member excluded from the coverage-gated run
  (`Cargo.toml`'s `default-members`) since it never runs as part of
  every commit — `.github/workflows/perf.yml`, manually triggered.
- The release smoke-test gate (`.github/workflows/release.yml`, between
  `make build` and `make sbom`) is a fifth, release-specific check, not
  one of the four tiers above — see that workflow's own comments.
