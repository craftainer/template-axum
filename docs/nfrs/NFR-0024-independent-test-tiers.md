# NFR-0024. Keep the test suite split into independent tiers

## Status

Proposed

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
(load test against the `runner`-stage image, never `dev`/`mock` — same
reasoning as template-fastapi's `docs/adrs/0010-locust-for-load-
testing.md`). Only the unit tier is implemented as of this NFR's
`Proposed` status; see `tests/README.md`'s "What's not here yet" for
the remaining three.

## Source

Developers; QA/CI. Documented in `tests/README.md`. Mirrors template-
fastapi's `NFR-0021-three-tier-test-suite.md`, widened to four tiers to
match this repo's own e2e/perf split (`docs/adrs/0010-...md` in that
repo's numbering).

## Verification

CI runs each implemented tier as a distinct step/command; each tier's
existence and scope is reviewed against `tests/README.md`. Currently:
`cargo test` (unit, via the `cargo-llvm-cov` pre-commit hook). This
NFR moves to `Implemented` once the integration tier lands with its own
CI step.
