# NFR-0025. A rate-limit backend outage fails fast, not hung

## Status

Implemented

## Attribute

Reliability / availability.

## Description

A rate-limited route (`FR-0023`) whose Redis backend is unreachable
shall fail within a bounded time (a few seconds) with
`AppError::Internal`, rather than the request hanging indefinitely
waiting on connection retries.

## Source

Discovered during this item's own implementation: `redis::aio::
ConnectionManager`'s default configuration has no connection/response
timeout (`None`/`None`) and retries up to 6 times with exponential
backoff, which turned an "unreachable Redis" unit test into a multi-
minute hang instead of a fast failure. See ADR 0011.

## Verification

`RateLimiter::connect` sets an explicit 2-second connection/response
timeout and caps retries to 1 via `redis::aio::
ConnectionManagerConfig`. `rate_limit::tests::
connect_fails_fast_for_an_unreachable_server` asserts `connect()`
against an unrouted address returns promptly (the whole suite,
including this test, completes in under 2 seconds).
