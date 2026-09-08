# FR-0022. Emit every log line as structured JSON

## Status

Implemented

## Description

The system shall format every log line (application and HTTP-request
logs) as a single JSON object, with no plain-text fallback.

## Source

See ADR 0006.

## Acceptance criteria

- `telemetry::configure_logging` is the only place a `tracing` subscriber
  is installed, called once at the start of `main()`.
- Verified in this phase's smoke test: startup, migration, and request
  logs all render as one-JSON-object-per-line on stdout.
