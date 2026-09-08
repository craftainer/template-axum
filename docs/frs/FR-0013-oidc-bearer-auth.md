# FR-0013. Require a valid OIDC bearer token on any route that declares it

## Status

Implemented

## Description

The system shall validate an `Authorization: Bearer <token>` header
against the configured OIDC issuer's discovery document and JWKS
(signature, issuer, expiry, and audience when configured) for any route
using the `AuthClaims` extractor; a missing or invalid token shall return
401 with `WWW-Authenticate: Bearer`.

## Source

Standard bearer-token resource-server validation; see ADR 0003.

## Acceptance criteria

- `src/oidc/mod.rs::AuthClaims` rejects a request with no `Authorization`
  header, a non-`Bearer` scheme, or a token failing verification.
- Under `Mode::Mock`, verification is skipped (claims trusted as-is) per
  FR-0017.
- 401 responses carry `WWW-Authenticate: Bearer` (`src/problem_details.rs`).
- Verified end-to-end via `POST /mock/token` + role-gated Hero create in
  this phase's smoke test.
