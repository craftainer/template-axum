# FR-0011. Provide a readiness endpoint aggregating every dependency check

## Status

Implemented

## Description

The system shall expose `GET /health/ready` running every registered
`HealthCheck` concurrently, returning 200 with a per-check breakdown if
all are healthy, else 503 with `status: "degraded"`.

## Source

Kubernetes readiness-probe convention; see ADR 0001.

## Acceptance criteria

- `src/health/mod.rs`'s `HealthRegistry::run_all` runs every check via
  `futures::future::join_all` (concurrently, not sequentially).
- Response body: `{"status": "ok"|"degraded", "checks": {name:
  {"healthy": bool, "detail": string|null}}}`.
- Verified against a real Postgres instance (database check reports
  healthy; Redis/S3/OIDC correctly report unhealthy when those services
  are absent) in this phase's smoke test.
