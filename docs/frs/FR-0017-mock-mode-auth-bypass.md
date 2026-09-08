# FR-0017. Mint a Keycloak-shaped token and skip signature verification under MODE=mock

## Status

Implemented

## Description

Under `MODE=mock` only, the system shall mount `POST /mock/token`
(accepting `{"sub": string, "roles": [string]}`) minting a JWT with the
same `resource_access.<client>.roles` claim shape a real Keycloak token
carries, and `decode_bearer_token` shall skip JWKS/signature verification
for every incoming token in this mode.

## Source

RBAC-testable without a running OIDC provider; see ADR 0005.

## Acceptance criteria

- `POST /mock/token` is not mounted (404) outside `MODE=mock`
  (`main.rs` only nests `controllers::mock::router()` when
  `settings.mode == Mode::Mock`).
- A minted token's claims round-trip through `decode_bearer_token`
  unverified under `MODE=mock` -- verified in this phase's smoke test.
