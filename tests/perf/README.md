# tests/perf/

The perf tier (one of the four tiers `docs/nfrs/NFR-0024-independent-test-tiers.md`
requires; what it reports is `docs/nfrs/NFR-0030-perf-harness-reports-rps-latency-errors-cpu.md`;
the tool choice is `docs/adrs/0018`): a [goose](https://book.goose.rs/) load
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

Mock mode writes the same JSON summary (`perf-summary.json`; its `cpu` is
`null` unless `PERF_CGROUP_CPU_STAT` is set -- `perf.yml` sets it and
caps the target at 1 vCPU / 2 GB like real-stack mode).

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

1. caps the app container at `PERF_CPUS` vCPUs (default `1`) and
   `PERF_MEMORY` (default `2g`, swap disabled), so throughput and CPU per
   request mean the same on every host; both are recorded in the summary
   (`docs/nfrs/NFR-0030-perf-harness-reports-rps-latency-errors-cpu.md`);
2. forwards the pool settings under test (`DB_POOL_SIZE`,
   `DB_POOL_MIN_SIZE`, `DB_POOL_RECYCLE`, `DB_POOL_IDLE_TIMEOUT`,
   `DB_CONNECT_TIMEOUT`, `RUN_MIGRATIONS_ON_STARTUP`) from the caller's
   environment to the app container, when set (`docs/adrs/0022`), e.g.
   `DB_POOL_SIZE=4 DB_POOL_MIN_SIZE=4 tests/perf/run-real-stack.sh ...`;
   it also forwards the telemetry settings under test (`OTEL_EXPORTER_OTLP_ENDPOINT`,
   `OTEL_SERVICE_NAME`, `OTEL_TRACES_SAMPLER`, `OTEL_TRACES_SAMPLER_ARG`,
   `OTEL_LOGS_EXPORTER`), when set (`docs/adrs/0026`), and
   `STATS_CACHE_TTL_SECONDS` (`docs/adrs/0024`; the cache uses the script's
   Redis and MQTT, and the `X-Stats-Cache` response header shows whether it is
   on). The telemetry endpoint must be reachable from the `keycloak_default` network, for example an OpenTelemetry
   Collector container with a `nop` exporter attached to it;
3. drops and recreates the `ax4h_perf` database on the shared Postgres
   (`PERF_DB` to rename it; no other database is touched);
4. starts the image as `ax4h-perf-app` on the Keycloak network, also
   attached to the Postgres, Redis and MQTT networks, with Redis DB 14
   and the rate limits raised as in mock mode;
5. seeds `PERF_SEED` records (default 500) through the API, using the
   `HeroSeedReal` goose scenario (`PERF_SEED=0` skips it);
6. runs `HeroJourneyReal` (`--scenarios HeroJourneyReal`) from the Rust
   toolchain image on the same network, so the token's issuer URL
   (`http://keycloak:8080/realms/template-axum`) is identical for goose
   and the app. The app container's cgroup directory is bind-mounted
   read-only (`tests/perf/cgroup-path.sh` finds it) so the client can
   sample its CPU; goose's HTML report and the JSON summary land in
   `PERF_OUT_DIR` (default `target/perf/`) as `report-<UTC timestamp>.html`
   and `summary-<UTC timestamp>.json`;
7. removes the app container on exit. The database is left behind for
   inspection and dropped by the next run.

Do not pass `--scenarios`; the script selects it. Settings read from the
environment by the goose client: `PERF_TOKEN_URL`, `PERF_CLIENT_ID`,
`PERF_USERNAME`, `PERF_PASSWORD` (defaults match the stack's realm; the client is `test-cli`). Not
run in CI: `perf.yml` stays on mock mode, because the GitHub runner has no
Keycloak/Postgres stack to point at.

For a comparable run use `--startup-time` (the warm-up ramp: goose
discards everything measured until all users are running) and a fixed
`--users`/`--run-time`, e.g. `--users 25 --startup-time 30s --run-time 300s`.

## JSON summary and CPU sampling

Every run of the client (both modes) writes one JSON file after the run
(`PERF_SUMMARY_FILE`; default `perf-summary.json` in the working
directory; set but empty skips it -- the seed run does). It covers the
**measured window** only: goose resets its metrics when all users are
running, so the `--startup-time` ramp is `warmup_s` and is excluded from
every figure. `run-real-stack.sh` also sets `PERF_GIT_SHA`, `PERF_IMAGE`,
`PERF_CPUS` and `PERF_MEMORY`, which the client records.

CPU comes from the app container's cgroup v2 `cpu.stat`
(`PERF_CGROUP_CPU_STAT`, a file path; unset means no CPU numbers). The
client reads it every `PERF_CPU_SAMPLE_MS` (default 500) for the whole run
and takes the delta over the window, interpolating linearly between the
samples that bracket each edge. If the samples do not cover the window,
`cpu` is `null` and a warning is printed.

Schema (`schema_version` 1; names carry their unit and are stable, a
rename, removal or change of meaning bumps the version; absent values are
`null`):

| Field | Meaning |
|---|---|
| `schema_version` | Integer, `1`. |
| `scenario`, `mode` | `HeroJourney`/`mock` or `HeroJourneyReal`/`real`. |
| `git_sha`, `image_tag` | Commit (`-dirty` suffix if the tree was modified) and image under test; `unknown` when not provided. |
| `users` | Maximum simulated users. |
| `warmup_s`, `duration_s` | Ramp excluded from the figures; length of the measured window. |
| `requests`, `requests_success`, `errors` | Counts in the window. A success is a 2xx response, every other status is an error. |
| `error_rate` | `errors / requests`, 0..1. |
| `rps` | `requests / duration_s`. The primary comparison metric. |
| `p50_ms`, `p95_ms`, `p99_ms`, `mean_ms`, `max_ms` | Response time over all requests. Percentiles are nearest-rank over goose's histogram, which rounds times above 100 ms (to 10 ms, then 100 ms, then 1000 ms). |
| `cpu.cpu_s`, `cpu.user_s`, `cpu.system_s` | CPU seconds the app container used in the window. |
| `cpu.cpu_ms_per_request` | `cpu_s * 1000 / requests_success`. |
| `cpu.avg_cores` | `cpu_s / duration_s`, to compare with the cap. |
| `cpu.nr_throttled`, `cpu.throttled_s` | CFS periods throttled and time throttled in the window; non-zero means the cap bit. |
| `cpu.samples`, `cpu.sample_interval_ms` | How the CPU figures were taken. |
| `limits.cpu_limit_cores`, `limits.memory_limit_mib` | The caps the app ran under (`PERF_CPUS`, `PERF_MEMORY`). |
| `endpoints[]` | Per method and path: `method`, `path`, `requests`, `failed`, `latency.{p50_ms,p95_ms,p99_ms}`. |

The `cpu` block only measures the app container: the load generator's
own CPU is not included, and a generator that cannot keep up shows as
a low `rps` with `cpu.avg_cores` below the cap.

## Why goose, not the reference's Locust

`docs/adrs/0018` records this: Locust is Python-only tooling this
Rust-only repository has no other reason to depend on: goose is a
Rust-native load-testing framework with a comparable feature set
(weighted scenarios/transactions, live metrics, a CLI matching Locust's
own `--users`/`--run-time` shape), so a contributor already working in
this codebase's language writes and reads the load test in it too.
