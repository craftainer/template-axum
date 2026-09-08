# NFR-0022. Boot fully functional under MODE=mock with zero real infrastructure

## Status

Implemented

## Attribute

Testability / developer experience.

## Description

Under `MODE=mock`, the application shall start and serve CRUD, health,
and RBAC-gated requests correctly with no Postgres, Redis, S3, or OIDC
provider running.

## Source

See ADR 0005.

## Verification

Manual, this phase: `docker run -e MODE=mock -e ALLOW_MOCK_MODE=1
... cargo run` with no other containers running; `POST /mock/token` ->
role-gated `POST`/`GET /crud/v1/heroes/v2/json` and `GET /health/ready`
(all four checks `"mocked"`) all succeeded.
