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
