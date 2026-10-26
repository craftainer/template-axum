# FR-0035. Optionally export structured logs via OTLP

## Status

Implemented

## Description

The system shall, when `OTEL_EXPORTER_OTLP_ENDPOINT` or
`OTEL_EXPORTER_OTLP_LOGS_ENDPOINT` is set in the process environment,
export every `tracing` log event to an OTLP log collector over HTTP, in
addition to (never instead of) the unconditional structured-JSON stdout
output (`docs/adrs/0006`). No new application setting is introduced --
this reads OpenTelemetry's own standardized environment variables
directly.

## Source

Port of the reference implementation's `FR-0023`, deferred in
`docs/adrs/0006`'s original text ("wiring one is additive... and
doesn't change this decision's shape if added later").

## Acceptance criteria

- With neither `OTEL_EXPORTER_OTLP_ENDPOINT` nor
  `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT` set, log output is unchanged from
  before this requirement (JSON stdout only, no outbound OTLP traffic).
- With either variable set, `tracing` events are additionally forwarded
  to the configured OTLP endpoint via a batch log processor -- stdout
  JSON output continues unchanged alongside it.
- Verified by the `craftainer-observability` crate's own tests of
  `craftainer_observability::logs` (the gating on the OTLP endpoint
  variables), and end-to-end by `tests/otlp_log_export.rs`, which runs the
  real binary against an in-process OTLP receiver.
