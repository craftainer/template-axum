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

Two modes, picked by the `PERF_TARGET` environment variable (goose owns
the command line, so extra options go through the environment): `mock`
(default, the `HeroJourney` scenario) and `real` (the `HeroJourneyReal`
scenario). See "Mock mode" and "Real-stack mode" below for what each can
and cannot measure.

## Mock mode

Every simulated user mints its own `MODE=mock` bearer token in `on_start`
(`maintainer` — the one role in both `HERO_WRITE_ROLES` and
`HERO_DELETE_ROLES`, so a user's own create/update/delete never hits
`docs/adrs/0007`'s ownership scoping), then runs a weighted mix of
`GET /crud/v1/heroes/v2/json`, `GET /health/live`, and a
create→update→delete round trip against its own record. `MODE=mock`
(rather than real Postgres/Redis/S3/Keycloak) keeps this tier
self-contained — one container, no stack — while still measuring the
actual compiled release binary inside its actual release image.

Because there is no database, cache or broker behind it, mock mode
cannot measure anything that depends on them: pool settings, a stats
cache, conditional requests against stored rows, OTel export overhead,
MQTT event publishing. It is the CI scenario and a quick smoke check;
use real-stack mode for those.

### Running it

```bash
# From the repo root:
make build                                  # builds the runner image
docker run --rm -d -p 8000:8000 \
  -e MODE=mock -e ALLOW_MOCK_MODE=1 \
  -e RATE_LIMIT_HERO_WRITE_PER_MINUTE=1000000 \
  -e RATE_LIMIT_MOCK_TOKEN_PER_MINUTE=1000000 \
  --name template-axum-perf-target \
  template-axum:<version>                   # RELEASE_VERSION from `make build`

cargo run -p template-axum-perf -- \
  --host http://localhost:8000 \
  --users 20 --run-time 60s

docker stop template-axum-perf-target
```

The two `RATE_LIMIT_*` variables raise the app's per-IP limiters
(`docs/adrs/0011`; defaults 10 and 20 per minute): every simulated user
shares the load generator's IP, so at the defaults most writes would
come back `429` and count as errors.

`--users`/`--run-time`/`--startup-time` and every other goose CLI flag
are goose's own (`cargo run -p template-axum-perf -- --help`) — nothing
here wraps or restricts them.

## Real-stack mode

The same `runner` image, run as `MODE=dev` against the devcontainer
stack's own services instead of fakes: Postgres (migrations applied at
startup), Redis (rate limiter), the MQTT broker (CRUD events) and
Keycloak. This is the mode that can measure the parity items of
the AX4 parity work (pool settings, stats cache, conditional requests,
OTel overhead, graceful shutdown).

**Auth is the production path.** The app validates every token against
Keycloak's JWKS (issuer, audience, expiry); there is no bypass. Each
simulated user fetches a real access token with the OIDC password grant
as the stack's seeded `maintainer` user (public client `api`, which has
`directAccessGrantsEnabled` in `.devcontainer/stack/keycloak/realm-export.json`;
`maintainer` is the one role in both `HERO_WRITE_ROLES` and
`HERO_DELETE_ROLES`) and re-fetches it shortly before it expires, so
runs longer than the access-token lifespan keep working.

The journey adds `GET /crud/v1/heroes/v2/json/stats` (weight 2) to the
mock scenario's mix. Deleted records are soft-deleted (`archived_at`),
so the table grows during a run; the script therefore starts from a
fresh database every time.

```bash
# Prerequisite: the stack containers are up -- postgres-postgres-1,
# redis-redis-1, mqtt-mqtt-1, keycloak-keycloak-1 (one compose project per
# fragment in .devcontainer/stack/; start any that is missing with
# `docker compose -p <name> -f .devcontainer/stack/<name>/compose.yml up -d --wait`).
make build                                  # prints the image tag
tests/perf/run-real-stack.sh template-axum:<version> --users 20 --run-time 60s
```

`run-real-stack.sh` (everything after the image is passed to goose):

1. drops and recreates the `ax4h_perf` database on the shared Postgres
   (`PERF_DB` to rename it; no other database is touched);
2. starts the image as `ax4h-perf-app` on the Keycloak network, also
   attached to the Postgres, Redis and MQTT networks, with Redis DB 14
   and the rate limits raised as in mock mode;
3. seeds `PERF_SEED` records (default 500) through the API, using the
   `HeroSeedReal` goose scenario (`PERF_SEED=0` skips it);
4. runs `HeroJourneyReal` (`--scenarios HeroJourneyReal`) from the Rust
   toolchain image on the same network, so the token's issuer URL
   (`http://keycloak:8080/realms/template-axum`) is identical for goose
   and the app;
5. removes the app container on exit. The database is left behind for
   inspection and dropped by the next run.

Settings read from the environment by the goose client:
(Do not pass `--scenarios`; the script selects it.) `PERF_TOKEN_URL`, `PERF_CLIENT_ID`, `PERF_USERNAME`, `PERF_PASSWORD`
(defaults match the stack's realm). Not run in CI: `perf.yml` stays on
mock mode, because the GitHub runner has no Keycloak/Postgres stack to
point at.

## Why goose, not the reference's Locust

`docs/adrs/0018` records this: Locust is Python-only tooling this
Rust-only repository has no other reason to depend on: goose is a
Rust-native load-testing framework with a comparable feature set
(weighted scenarios/transactions, live metrics, a CLI matching Locust's
own `--users`/`--run-time` shape), so a contributor already working in
this codebase's language writes and reads the load test in it too.
