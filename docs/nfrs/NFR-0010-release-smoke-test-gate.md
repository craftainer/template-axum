# NFR-0010. Gate releases on a smoke test against real dependencies

## Status

Implemented

## Attribute

Operations / release engineering.

## Description

Before a release is considered verified, the built `runner` image shall
be run against real Postgres, Redis, S3, and Keycloak, with
`/health/ready` polled (5s interval, 5s timeout, 30 retries, ~150s total)
until it reports healthy. A release whose image never reports healthy
shall not proceed to SBOM generation or publishing.

## Source

[Release engineering](../stakeholders.md). Implemented in the root
`compose.yml` (the backing-service stack), invoked from
`.github/workflows/release.yml`.

## Verification

CI: `release.yml`'s smoke-test step fails the release workflow if
`/health/ready` doesn't report healthy within the configured retries,
before `make sbom`/`make publish` run.
