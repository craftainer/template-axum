# 0018. Load-test with `goose`, not the reference's Locust

## Status

Accepted

## Context

`NFR-0024`'s perf tier (the last of the four tiers that NFR requires)
ports the reference implementation's own load-test tier
(`docs/adrs/0010-locust-for-load-testing.md` in that repo's numbering),
which uses [Locust](https://locust.io/) — a natural choice there, since
the reference application itself is Python. This repo's own plan
(`docs/plans/2026-09-fastapi-parity-gaps.md`) explicitly left the tool
choice open: "this repo isn't bound to that choice."

Locust would still work here — it's a black-box HTTP load generator, and
a Python process can drive load against any HTTP server regardless of
what language that server is written in. But pulling it in would mean
this otherwise Rust-only repository suddenly depends on a Python
interpreter, `pip`/`uv`-managed dependencies, and Locust's own
Python-scripted `HttpUser` classes for one directory's worth of tests —
introducing exactly the kind of second-language footprint
`docs/TEMPLATE.md`'s "Versions and config" and this repo's own
`Cargo.toml`-everywhere discipline is otherwise careful to avoid (the
only Python in this repo is `scripts/develop.sh`'s infrastructure
tooling — `prek`'s own hook venvs and `.github/scripts/*.py` — never
application or test code).

## Decision

`tests/perf/` is a separate Cargo workspace member, `template-axum-perf`,
built on [`goose`](https://book.goose.rs/) — a Rust-native load-testing
framework explicitly modeled on Locust's own concepts (weighted
scenarios/transactions playing Locust's `TaskSet`/`@task` role, a
comparable CLI: `--host`/`--users`/`--run-time`), so the shape of a
goose load test is immediately recognizable to anyone who has read a
Locust one. See `tests/perf/README.md` for what it tests and how to run
it.

## Consequences

The perf tier stays in the same language as the rest of this repo — a
contributor who can read `src/` can read and extend `tests/perf/src/
main.rs` without picking up Python or Locust's own DSL. `goose` pulls in
its own `reqwest`/TLS/tokio-adjacent dependency graph, isolated from the
app's own (`tests/perf/Cargo.toml`'s own comment on why it's a workspace
member, not a `[dev-dependencies]` addition) — the cost is a second,
larger `Cargo.lock` contribution, accepted the same way pulling in any
tool-specific dependency tree is.

The trade-off against the reference's choice: Locust's ecosystem
(distributed load generation across multiple worker machines, its web
UI) is more mature than goose's equivalent features, which matters at a
scale this template's own worked example (a single Hero CRUD resource)
was never going to reach. If a real deployment of an instance of this
template later needs distributed load generation goose's `--manager`/
`--worker` gaggle mode doesn't cover, that's a reason to revisit this
ADR then, not a reason to default to Locust now.

## Addendum (2026-10-25): a real-stack mode next to the mock one

The original scenario runs only under `MODE=mock`, with no Postgres,
Redis or MQTT behind it, so it cannot measure anything those services
influence (pool settings, a stats cache, conditional requests, tracing
overhead). `tests/perf` now has two modes selected by `PERF_TARGET`:
`mock` (unchanged: the `HeroJourney` scenario, still what `perf.yml`
runs) and `real` (`HeroJourneyReal`, plus a `HeroSeedReal` seeding
scenario), driven by `tests/perf/run-real-stack.sh` against the
devcontainer stack.

Decisions:

- **Auth stays the production path.** Real mode runs the app as
  `MODE=dev`, which verifies Keycloak-signed tokens; goose obtains them
  with the realm's existing password grant for the seeded `maintainer`
  user. No test-only bypass was added to the app, and none is needed.
- **Fresh database per run** (`ax4h_perf`, dropped and recreated by the
  script) because deletes are soft and the table would otherwise drift
  between A/B runs. Seed data goes through the API, not SQL.
- **Environment variables, not CLI flags**, for the mode and credentials,
  since goose owns the command line.
- **CI stays mock-only.** A GitHub runner has no Keycloak/Postgres stack;
  wiring service containers plus a realm import would duplicate the
  devcontainer stack for a manually triggered workflow. Real-mode numbers
  are taken on the devcontainer host.
- The documented mock command now raises the app's per-IP rate limits;
  at the defaults the single load-generator IP is throttled (`429`) and
  the run reports errors.

See `tests/perf/README.md` for usage.

## Addendum (2026-10-27): CPU per request, a JSON summary and a fixed cap

L2 A/B comparisons need numbers a script can read and a CPU budget that
means the same on every host (`docs/nfrs/NFR-0030-perf-harness-reports-rps-latency-errors-cpu.md`).
Extending the addendum above rather than opening a new ADR, because it
adds to the same harness and reverses nothing:

- **The sampler is Rust, inside the goose client** (`tests/perf/src/cpu.rs`),
  which keeps the repository Python-free as this ADR requires. It reads
  the app container's cgroup v2 `cpu.stat` as a file, which
  `run-real-stack.sh` bind-mounts read-only into the goose container
  (`tests/perf/cgroup-path.sh` finds the directory), instead of shelling
  out to `docker exec` per sample. The window delta is interpolated
  between the samples that bracket the measured window, whose start is
  where goose resets its metrics (all users running), so the warm-up ramp
  is excluded without a second clock.
- **One JSON summary per run**, built from goose's metrics by pure
  functions (`tests/perf/src/summary.rs`), schema versioned and
  documented in `tests/perf/README.md`. Percentiles come from goose's
  rounded histogram, which is adequate for A/B ratios and is stated in
  the schema.
- **1 vCPU / 2 GB by default** via `docker create --cpus/--memory`
  (`PERF_CPUS`, `PERF_MEMORY`), recorded in the summary. No
  `compose.perf.yml`: the script already starts the container with
  `docker create`, so the flags are the smallest change.
- **`perf.yml` stays mock-only** for the reason above, but now caps the
  target the same way, samples its cgroup when the runner exposes it, and
  uploads the JSON summary next to the goose report.
- **Not built:** a capacity (step-load) mode. RPS at a fixed user count
  is the primary metric; revisit if an A/B needs a saturation curve.
