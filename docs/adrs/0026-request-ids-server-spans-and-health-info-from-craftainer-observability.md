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
  `parentbased_always_on`. The documented default for operators is
  `OTEL_TRACES_SAMPLER=parentbased_traceidratio`,
  `OTEL_TRACES_SAMPLER_ARG=0.1` with l### Overhead budget

Measured on `main` at `a7e0500` (image `template-axum:l2-a7e0500`, id
`sha256:b59276c43890…`, which includes #23: sqlx statement logging off),
2026-10-29/30, with the L2 harness (`NFR-0030`):
`tests/perf/run-real-stack.sh <image> --users 20 --run-time 60s`, default
1 vCPU / 2 GB caps, real stack. Telemetry went to a throwaway
`otel/opentelemetry-collector:0.162.0` with an OTLP receiver and `nop`
exporters (as in template-fastapi's table), started for the runs only and
not part of the devcontainer stack. Five runs per arm in randomised order
(seed 20261029), plus drift checks of the unset arm at the start and end.
Acceptance rule: the median difference must exceed the larger of the two
arms' spreads and be at least 3%.

| Arm | Setting | median req/s | spread | vs U | CPU ms/request | p95 / p99 (ms) |
| --- | --- | --- | --- | --- | --- | --- |
| U | no `OTEL_*` | 2412.0 | 50.5 | - | 0.413 | 20 / 23 |
| F | `parentbased_always_on` (100%) | 2114.7 | 201.1 | -12.3% | 0.473 | 23 / 27 |
| T | `parentbased_traceidratio`, arg 0.1 | 2167.1 | 59.7 | -10.2% | 0.461 | 22 / 26 |
| TL | T plus `OTEL_LOGS_EXPORTER=none` | 2220.0 | 51.9 | -8.0% | 0.450 | 22 / 25 |

F, T and TL against U are real by the rule. F against T (+2.5%) and TL
against T (+2.4%) are within noise. The drift between the first and last
unset checks (U02 to U9) was +1.9%, within the spread; the very first run
(U0) was affected by host swap and is excluded.

Volume: about 2.1 to 2.4 spans per request at 100%, about a tenth of that
at 0.1. Log records: 7 startup lines per run, none per request (since #23
there are no per-request log events and sqlx statement logging is off).

Conclusion: the cost is the tracing/OpenTelemetry layer itself, not the
exported volume. Sampling at 10% and log export on or off do not change it
beyond noise.

Superseded: the earlier table measured on `a342b04` (#22) showed -8.5% to
-14.1% throughput and about 1.6 log records per request. Those numbers were
dominated by sqlx statement events at INFO, which #23 removed; do not use
them.

Decision (2026-10-30, user): the documented default sampling is
`OTEL_TRACES_SAMPLER=parentbased_traceidratio`,
`OTEL_TRACES_SAMPLER_ARG=0.1`, with log export on; the same as
template-fastapi ADR 0021. Rejected: 10% with logs off (the gain is within
noise and it differs from fastapi), and 100% (ten times the spans and the
noisiest arm).

Decision (2026-10-30, user): accept the 8 to 12% throughput cost of
telemetry on as the overhead budget; no span-layer investigation.
Telemetry stays opt-in (off by default).

lse)` or a higher level, then re-measure
the table with the TL and Z arms; a code change, so it needs its own work
item. The recommended default is not yet decided.

## Consequences

Easier: a client or gateway request ID finds the trace and the log lines;
slow routes show up per matched route, not per URL; pool saturation is
observable; deployments can show which build is running.

Harder: every response gains a header; `/health/info` discloses the version
and commit (the crate keeps it opt-in for that reason, and the template
opts in), so a deployment that dislikes this must drop the merge in
`build_router`; telemetry costs throughput once an endpoint is configured, which
the overhead table above bounds (8 to 12% less throughput at the reference
budget with export on); that is the accepted overhead budget.
