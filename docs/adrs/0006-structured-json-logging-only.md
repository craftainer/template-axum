# 0006. Emit structured JSON logs, and nothing else

## Status

Accepted

## Context

Ported from template-fastapi's `docs/adrs/0007`. A production deployment
needs machine-parseable logs (for log aggregation/alerting); a human
developer running locally still wants to read them. Tracing/metrics
instrumentation beyond logs was explicitly out of scope for this phase.

## Decision

`src/telemetry.rs::configure_logging` attaches `tracing_subscriber`'s
JSON formatter to every `tracing` call, unconditionally, called once at
the very start of `main()` before any other setup runs. This is the
*only* log output shape -- there is no plain-text fallback, and no
tracing span/metrics exporter wired up. The log level is controlled by
`RUST_LOG` (via `tracing_subscriber::EnvFilter`), defaulting to `info`.
`tower_http::trace::TraceLayer` (already present from phase 1) rides the
same subscriber, so HTTP request logs come out as the same structured
JSON as every other `tracing::info!`/`warn!`/`error!` call
(`src/oidc/mod.rs`'s rejected-token warnings, `src/health/checks.rs`'s
failure logs).

## Consequences

Every log line, from every layer, is uniformly JSON -- a log aggregator
never has to special-case one subsystem's output format against
another's. The cost: local development output is denser/less readable
than a plain-text formatter would give; this is accepted the same way
the Python original accepts it, on the reasoning that a structured-only
default is more valuable long-term than a nicer local dev experience,
and `RUST_LOG=debug` plus `jq` covers the difference when needed.

Optional OTLP log export (`FR-0035`) later filled in the deferred gap
this section originally described: `telemetry::configure_logging`
attaches a second `tracing_subscriber::Layer`
(`opentelemetry-appender-tracing`'s `OpenTelemetryTracingBridge`,
forwarding every `tracing` event to a `SdkLoggerProvider`/
`BatchLogProcessor` over OTLP/HTTP) whenever `OTEL_EXPORTER_OTLP_ENDPOINT`
or `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT` is set in the environment -- read
directly, not as a new `Settings` field, since these are OpenTelemetry's
own standardized env vars, not something this app should re-invent under
a different name. This is additive, exactly as anticipated below: the
structured-JSON-stdout output above stays the unconditional default
either way, and the OTLP layer is simply absent when neither env var is
set.
