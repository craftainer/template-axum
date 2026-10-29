# 0026. Wire request IDs, server spans, pool gauges and `/health/info` from `craftainer-observability`

## Status

Proposed

## Context

[`0006`](0006-structured-json-logging-only.md) kept the app to JSON logs
and [`0020`](0020-use-craftainer-crud-and-craftainer-observability.md)
moved logging into `craftainer-observability`. Since 0.3.0 that crate also
provides what template-fastapi gets from `craftainer-tool-observability`
(its `docs/adrs/0021-opentelemetry-traces-and-metrics-pushed-over-otlp.md`
and `docs/nfrs/NFR-0025-observability-scope-logs-traces-and-metrics.md`): a
validated request-ID layer, a server-span layer, pool gauges and an opt-in
`/health/info` router. `main.rs` already calls `configure_telemetry()` (it
owns the subscriber and the OTLP providers); the pieces below were not
mounted yet.

## Decision

We will wire, in `build_router` and `main.rs`, and add no setting of our own:

- **Request ID.** `RequestIdLayer` is the outermost layer. It keeps an
  incoming `X-Request-ID` only when it is 1 to 128 characters from
  `[A-Za-z0-9._-]`, otherwise creates a UUID, echoes the result on the
  response, and opens a `request` span carrying `request_id`, so every log
  line below it (including `TraceLayer`'s) has it. This adds one response
  header, `X-Request-ID`, to every route.
- **Server span.** `ServerSpanLayer` sits inside the request-ID layer and
  outside `TraceLayer`. It names spans `METHOD /matched/route`
  (`MatchedPath`), records `http.server.request.duration`, and excludes
  `/health/*`. Because it is applied with `Router::layer` it runs after
  routing. It is built in `build_router`, after `configure_telemetry()` in
  `main`; without telemetry it is a no-op.
- **Pool gauges.** `main.rs` calls `instrument_db` with the sqlx `PgPool`
  that sea-orm wraps (`DatabaseConnection::get_postgres_connection_pool`;
  sea-orm and the crate share one sqlx 0.9.0, so no new dependency).
  Not called under `Mode::Mock`, which has no pool.
- **`/health/info`.** `info_router()` is merged next to the crate's health
  router under `/health`: `GET /health/info` returns
  `{"version", "commit"}` from `APP_VERSION` and `APP_COMMIT` (read per
  request, empty or unset becomes `null`). The `Dockerfile` already turns
  the `APP_VERSION`/`APP_COMMIT` build args (`image.mk`,
  [`0019`](0019-re-parent-onto-template-docker.md)) into runtime
  environment variables; the same variable also sets `service.version` on
  all signals.
- **Sampling.** No knob in the app. The OpenTelemetry SDK reads
  `OTEL_TRACES_SAMPLER` and `OTEL_TRACES_SAMPLER_ARG` itself; unset means
  `parentbased_always_on`. As in template-fastapi, probes stay out of
  traces and the behaviour with `OTEL_EXPORTER_OTLP_ENDPOINT` unset is
  unchanged.

### Overhead budget

Measured on `a342b04` (image `template-axum:l2-a342b04`, id
`sha256:fdf446b9bc12`, fat LTO, `Dockerfile` `runner` target), 2026-10-28,
with the L2 harness (`NFR-0030`): 20 users, 60 s (plus 20 s warm-up), the
default 1 vCPU / 2 GB caps, real stack (`tests/perf/run-real-stack.sh`),
0 errors in every run. Telemetry went to an OpenTelemetry Collector
`otel/opentelemetry-collector:0.162.0` (core distribution; digest
`sha256:310a800ad69ee430e7c541796852a242c9c7db97aaad4daa5ccf843c525fbdb2`)
running uncapped on the same Docker network, with an `otlp` receiver, the
`batch` processor and the `nop` exporter for traces, metrics and logs; this
mirrors template-fastapi's table (`nop` exporter against a perf-only
collector), which used 25 users and 300 s, so absolute numbers are not
comparable across the two services. The Collector was started for the runs
only and is not part of the devcontainer stack. All export arms set
`OTEL_EXPORTER_OTLP_ENDPOINT=http://l2-otel-collector:4318` and
`OTEL_SERVICE_NAME`.

Five arms, five runs each, in a randomised order (fixed seed) with an extra
run of the unset arm at the very start and very end. Medians, with the range
over the runs in brackets; overhead is against the unset median:

| Arm | Setting | req/s | CPU ms/request | p50 / p95 / p98 / p99 (ms) |
| --- | --- | --- | --- | --- |
| U | telemetry unset (7 runs) | 2300.6 (2279-2339) | 0.430 (0.425-0.434) | 7 / 21 / 23 / 24 |
| F | traces 100% | 1975.5 (1897-2000), -14.1% | 0.506 (0.499-0.526), +17.7% | 8 / 24 / 27 / 29 |
| T | traces 10% | 2042.1 (1974-2059), -11.2% | 0.489 (0.485-0.502), +13.7% | 8 / 24 / 26 / 27 |
| TL | traces 10%, `OTEL_LOGS_EXPORTER=none` | 2104.6 (1963-2121), -8.5% | 0.473 (0.469-0.505), +10.0% | 8 / 23 / 25 / 26 |
| Z | export on, `OTEL_TRACES_SAMPLER=always_off` | 2057.1 (1991-2063), -10.6% | 0.485 (0.483-0.499), +12.8% | 8 / 23 / 26 / 27 |

Traces at 100% and 10% use `OTEL_TRACES_SAMPLER=parentbased_traceidratio`
with `OTEL_TRACES_SAMPLER_ARG` 1.0 and 0.1. The first and last unset runs
were 2338.8 and 2300.6 req/s (-1.6%), inside the unset spread (2279-2339),
so the table is not drift-limited. Host swap was in use throughout (about
3 GB), but swap-in during runs was small (350 to 5000 pages) and not
correlated with the arm. Latency percentiles move by 1 to 5 ms, close to
the harness's 1 ms resolution; only throughput and CPU per request are
read as the result. The Collector's counters per run (the app flushes
metrics once per run, which is why 8 metric points are seen for every
exporting arm):

| Arm | Spans per run (median) | Log records per run (median) | Log records per request |
| --- | --- | --- | --- |
| F | 281,824 | 191,350 | about 1.6 |
| T | 28,994 | 199,003 | about 1.6 |
| TL | 29,514 | 0 | 0 |
| Z | 0 | 200,519 | about 1.6 |

(Counts include the 20 s warm-up and the seeding, which the request count
excludes, so the per-request figure is an upper bound of roughly 1.2 to
1.6.)

What the numbers say:

- Turning export on costs 9 to 14% of throughput and 10 to 18% more CPU per
  request, outside the unset spread in every arm.
- Sampling matters less than expected. 100% against 10% adds about 3
  points of throughput loss and 4 points of CPU per request, which is
  inside the run-to-run ranges of the two arms (they overlap). The
  `always_off` arm (Z), with no spans at all, is as costly as 10% sampling
  (+12.8% against +13.7%), so a large part of the cost is not span
  creation or sampling.
- Log export is the other large component. Switching it off at 10% sampling
  (TL against T) recovers about 3.7 points of CPU per request and 3 points
  of throughput; the ranges overlap, so this is indicative only.
- Not measured: an arm with no log export and no spans, so the fixed cost
  of the metrics, request-ID and server-span layers alone is unknown, and
  the capacity (maximum users) sweep fastapi reports.
- Log volume is about 1.6 records per request (at most 200,000 per run),
  about 75 times the record rate of template-fastapi's table, which
  reports about 9.8k per 300 s run. The records are all `sqlx::query`
  statement events at INFO (sea-orm's default `sqlx_logging`, one per
  query, one to two per request, none for `/health/*`), written to stdout as
  JSON in every arm and, with an endpoint, also exported over OTLP. The
  `TraceLayer` request and response events are at DEBUG and are not
  emitted at the default `info` filter.

Lean on the default, for the maintainers to decide: recommend
`OTEL_TRACES_SAMPLER=parentbased_traceidratio` with
`OTEL_TRACES_SAMPLER_ARG=0.1` as the template's documented starting point.
The reason is the lower trace volume and the (statistically weak) gain over
100%, not a measured capacity difference. The measured gap to telemetry
unset is mostly the log export of statement events and the always-on layers.
Follow-up for the maintainers to consider, not done here: stop exporting
(or emitting) `sqlx::query` events at INFO, for example through
`ConnectOptions::sqlx_logging(false)` or a higher level, then re-measure
the table with the TL and Z arms; a code change, so it needs its own work
item. The recommended default is not yet decided.

## Consequences

Easier: a client or gateway request ID finds the trace and the log lines;
slow routes show up per matched route, not per URL; pool saturation is
observable; deployments can show which build is running.

Harder: every response gains a header; `/health/info` discloses the version
and commit (the crate keeps it opt-in for that reason, and the template
opts in), so a deployment that dislikes this must drop the merge in
`build_router`; telemetry costs CPU once an endpoint is configured, which
the overhead table above bounds (about 10 to 18% more CPU per request
at the reference budget with export on).
