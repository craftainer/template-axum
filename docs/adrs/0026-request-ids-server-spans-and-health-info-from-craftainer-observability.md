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

Pending. template-fastapi measured telemetry unset, 100% and 10% sampling
at the 1 vCPU / 2 GB budget and recommended 10%
(`parentbased_traceidratio`, arg `0.1`). The equivalent three-way table for
this service (L2 harness, `NFR-0030`) is to follow, and the recommended
default sampling will be decided from it. Until then no sampling default is
recommended.

## Consequences

Easier: a client or gateway request ID finds the trace and the log lines;
slow routes show up per matched route, not per URL; pool saturation is
observable; deployments can show which build is running.

Harder: every response gains a header; `/health/info` discloses the version
and commit (the crate keeps it opt-in for that reason, and the template
opts in), so a deployment that dislikes this must drop the merge in
`build_router`; telemetry costs CPU once an endpoint is configured, which
the pending measurement must bound.
