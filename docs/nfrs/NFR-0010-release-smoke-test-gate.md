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
shall not proceed to `finalize`, the git tag or the GitHub release. Each
per-arch image additionally passes a mock-mode liveness smoke
(`/health/live`) before it is pushed.

## Source

[Release engineering](../stakeholders.md). Implemented as `image.mk`'s
`stack-smoke` target (a `release-check` prerequisite, run inside the
devcontainer where the stack is up), invoked from
`.github/workflows/release.yml`; the per-arch liveness smoke is
`make release-assets`' `image-smoke` step. See
`docs/adrs/0019-re-parent-onto-template-docker.md`.

## Verification

CI: `release.yml`'s `release-check` step fails the release workflow if
`/health/ready` doesn't report healthy within the configured retries,
before `make finalize` and `gh release create` run. A per-arch image that
fails its mock-mode `/health/live` smoke fails its `build` job before
`make publish`.
