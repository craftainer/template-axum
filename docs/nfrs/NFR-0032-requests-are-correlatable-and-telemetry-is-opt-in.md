# NFR-0032. Requests are correlatable by ID, and telemetry is opt-in and low-overhead

## Status

Proposed

## Attribute

Observability / performance.

## Description

The service shall:

- give every response an `X-Request-ID` header: the client's value when it
  is 1 to 128 characters from `[A-Za-z0-9._-]`, otherwise a generated one,
  so a client cannot put control characters into a log line;
- put that ID on every log line written while handling the request and on
  the request's server span;
- when `OTEL_EXPORTER_OTLP_ENDPOINT` is set, export traces and metrics (and
  logs) over OTLP, configured only through OpenTelemetry's standard
  environment variables; when it is unset, behave as before apart from the
  request ID;
- name server spans after the matched route, never the raw path, and keep
  `/health/*` out of traces and the duration metric;
- report the running `version` and `commit` at `GET /health/info`, with
  `null` for a value that is not set;
- keep the cost of telemetry within an accepted overhead budget at the
  1 vCPU / 2 GB reference budget, measured against telemetry unset: with the
  documented sampling default (`OTEL_TRACES_SAMPLER=parentbased_traceidratio`,
  `OTEL_TRACES_SAMPLER_ARG=0.1`, log export on) throughput falls by about
  10% (8 to 12% across the export-on arms). Telemetry stays opt-in (off by
  default), so the cost applies only where an endpoint is configured.

## Source

Operators preparing a Kubernetes deployment with OpenTelemetry; parity with
template-fastapi's `NFR-0025`
(`docs/nfrs/NFR-0025-observability-scope-logs-traces-and-metrics.md` there).
Detailed by [`ADR-0026`](../adrs/0026-request-ids-server-spans-and-health-info-from-craftainer-observability.md).

## Verification

- `src/lib.rs` unit tests: a valid ID is echoed, a missing, malformed or
  over-long one is replaced, on every route including a 404; `/health/info`
  is mounted.
- `tests/health_info.rs`: `/health/info` returns the version and commit, and
  `null` when a variable is unset or empty.
- `tests/otlp_log_export.rs` still passes (logs reach the collector).
- Not covered by an automated test: that `/health/*` produces no server
  span and that the ID reaches log lines (the crate's own tests cover both;
  `craftainer-observability` 0.3.0 `tests/request_id_logs.rs`).
- Overhead: measured 2026-10-29/30 with the `tests/perf/` harness (image
  `template-axum:l2-a7e0500`, 20 users, 60 s, 1 vCPU / 2 GB, `nop` exporter
  on an OpenTelemetry Collector 0.162.0, five runs per arm): against
  telemetry unset (2412.0 req/s, 0.413 CPU ms/request), 100% sampling
  -12.3%, 10% sampling -10.2%, 10% with `OTEL_LOGS_EXPORTER=none` -8.0%;
  100% against 10% and logs on against off are within noise. User decision
  2026-10-30: this 8 to 12% is the accepted budget and the documented
  default is 10% with logs on (method and table in `ADR-0026`, "Overhead
  budget"). The earlier `a342b04` numbers were dominated by sqlx statement
  logs and are superseded.
