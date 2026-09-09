# tests/perf/

The perf tier (`docs/nfrs/NFR-0023-test-coverage-gate.md`'s sibling
`NFR-0024`, `docs/adrs/0018`): a [goose](https://book.goose.rs/) load
test client, `template-axum-perf`, against a *running* `template-axum`
server. A separate Cargo workspace member, not a dependency of the app
crate or a Cargo integration test under `../` — it never links against
`template-axum` itself, only talks to it over HTTP, and pulls in its own
`reqwest`/TLS stack unrelated to the app's own.

## What it targets

The `runner`-stage Docker image specifically (`docs/TEMPLATE.md`'s
"Release: a Makefile contract" — `make build` produces it), never a
`cargo run` dev loop. Running the actual release artifact is the point:
this tier answers "does the thing we ship handle load", not "does the
Rust code perform well in a debug build."

Every simulated user mints its own `MODE=mock` bearer token in `on_start`
(`maintainer` — the one role in both `HERO_WRITE_ROLES` and
`HERO_DELETE_ROLES`, so a user's own create/update/delete never hits
`docs/adrs/0007`'s ownership scoping), then runs a weighted mix of
`GET /crud/v1/heroes/v2/json`, `GET /health/live`, and a
create→update→delete round trip against its own record. `MODE=mock`
(rather than real Postgres/Redis/S3/Keycloak) keeps this tier
self-contained — one container, no stack — while still measuring the
actual compiled release binary inside its actual release image.

## Running it

```bash
# From the repo root:
make build                                  # builds the runner image
docker run --rm -d -p 8000:8000 \
  -e MODE=mock -e ALLOW_MOCK_MODE=1 \
  --name template-axum-perf-target \
  template-axum:<version>                   # RELEASE_VERSION from `make build`

cargo run -p template-axum-perf -- \
  --host http://localhost:8000 \
  --users 20 --run-time 60s

docker stop template-axum-perf-target
```

`--users`/`--run-time`/`--startup-time` and every other goose CLI flag
are goose's own (`cargo run -p template-axum-perf -- --help`) — nothing
here wraps or restricts them.

## Why goose, not the reference's Locust

`docs/adrs/0018` records this: Locust is Python-only tooling this
Rust-only repository has no other reason to depend on: goose is a
Rust-native load-testing framework with a comparable feature set
(weighted scenarios/transactions, live metrics, a CLI matching Locust's
own `--users`/`--run-time` shape), so a contributor already working in
this codebase's language writes and reads the load test in it too.
