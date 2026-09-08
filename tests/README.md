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
- **integration**, **e2e**, **perf** — not yet built; see "What's not
  here yet" below for what each will look like and why they were
  deferred.

This top-level `tests/` directory itself is reserved for Cargo's own
"integration test" convention (a black-box binary per file, linked
against the crate's public API only) once the integration tier
(real Postgres/Redis/S3/Keycloak, see `.devcontainer/stack/`) is built —
that's *why* it's the natural home for what template-fastapi calls
`tests/integration`, unlike unit tests, which Rust convention keeps
inside `src/`.

## Coverage gate

`cargo llvm-cov --fail-under-lines 80` measures `src/`'s line coverage
across the unit tier (currently the only tier collected) and fails
below an 80% floor — see `docs/nfrs/NFR-0023-test-coverage-gate.md` for
why 80%, not template-fastapi's 95%: Rust's type system statically
rules out a class of bug the Python floor is partly there to catch at
runtime, so the same number would be cargo-culted, not justified.

```bash
cargo llvm-cov --fail-under-lines 80
```

Wired into `.pre-commit-config.yaml` as `cargo-llvm-cov`, `pre-push`/
`manual` stage (same split as the existing `cargo-check`/`cargo-audit`
hooks) — so `prek run --all-files --hook-stage manual` (what CI runs)
enforces it, but a fast `pre-commit`-stage commit doesn't pay a full
instrumented test-suite run.

## What's not here yet

Tier B (integration, against the real devcontainer stack) and Tier C
(e2e-equivalent + perf) from the phase-3 test-suite plan were not
reached in this pass — Tier A (unit tests + the coverage gate) was the
"must complete" tier, and the remaining budget went to making it
thorough (every FR-0003 validation branch, the full FR-0015 role
matrix, health-check isolation/concurrency, config fail-fast on every
production setting) rather than starting Tier B half-finished. Adding
them later means: a `tests/integration/` Cargo integration-test binary
(or `#[cfg(feature = "integration")]` block) reaching the real stack
services per `docs/adrs/0005-mode-driven-fakes-for-infrastructure-free-
testing.md`'s "when this isn't the right layer" note, then the e2e/perf
tiers noted above.

## Do

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
  belongs in the not-yet-built integration tier once it lands.
- Depend on test execution order or leak state between tests (a set env
  var, a mutated static) without a lock guard.
