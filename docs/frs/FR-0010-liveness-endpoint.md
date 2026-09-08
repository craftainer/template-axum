# FR-0010. Provide a dependency-free liveness endpoint

## Status

Implemented

## Description

The system shall expose `GET /health/live` returning `200 {"status":
"ok"}` unconditionally, performing no dependency checks.

## Source

Kubernetes liveness-probe convention; see ADR 0001.

## Acceptance criteria

- `GET /health/live` always returns 200 while the process is running,
  regardless of Postgres/Redis/S3/OIDC availability.
- Verified in this phase's smoke test.
