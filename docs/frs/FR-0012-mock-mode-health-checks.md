# FR-0012. Replace every health check with an always-healthy stub under MODE=mock

## Status

Implemented

## Description

Under `MODE=mock`, the system shall register a `MockHealthCheck` (always
`healthy: true, detail: "mocked"`) for every dependency name instead of
the real check, performing no network access.

## Source

Zero-infrastructure local/CI use; see ADR 0005.

## Acceptance criteria

- `main.rs::build_health_registry` branches on `Mode::Mock` before
  constructing any real client (Postgres connection, Redis client, S3
  client, reqwest client).
- `GET /health/ready` under `MODE=mock` returns 200 with every check
  `detail: "mocked"` -- verified in this phase's smoke test.
