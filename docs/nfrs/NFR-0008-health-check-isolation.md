# NFR-0008. Isolate each health check's failure from the others

## Status

Implemented

## Attribute

Availability / fault isolation.

## Description

A single dependency's health-check failure shall never propagate as a
panic or fail the whole `/health/ready` response; each check catches its
own dependency's error type and reports `healthy: false` instead.

## Source

See ADR 0001.

## Verification

Manual/code review: every `HealthCheck::check` impl in
`src/health/checks.rs` returns `HealthCheckResult` from a `match`/`?`-free
body -- no `.unwrap()`/`.expect()` on a fallible call. Verified live: with
only Postgres running, `/health/ready` still returns a well-formed body
reporting Redis/S3/OIDC as unhealthy rather than the request failing.
