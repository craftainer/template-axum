# 0003. Validate bearer tokens server-side against any Authorization Code + PKCE OIDC provider

## Status

Accepted

## Context

Ported from template-fastapi's `docs/adrs/0003`, same reasoning: a
frontend obtains a token via the OAuth2 Authorization Code + PKCE flow
against whatever OIDC provider a deployment configures (Keycloak locally,
via `.devcontainer/stack/keycloak/`); this backend's only job is to
*validate* that bearer token per request, entirely server-side, with no
session state and no trust placed in the frontend. A federated/multi-
region deployment must be able to run identical backend code against
different regional issuers, varying only `OIDC_ISSUER_URL`.

## Decision

`src/oidc/mod.rs`'s `OidcVerifier` fetches `{issuer}/.well-known/
openid-configuration`, then the JWKS it points at, caching the key set
for 5 minutes (`JWKS_CACHE_TTL`). `decode_bearer_token` verifies
signature (via `jsonwebtoken`, matched against the token's `kid`),
issuer, expiry, and — when `Settings.oidc_audience` is configured —
audience. None of this code branches on which provider issued the token;
`OidcVerifier` only needs `OIDC_ISSUER_URL` to point somewhere exposing
standard OIDC discovery.

Role-based authorization is a separate, explicitly Keycloak-shaped
concern: `Claims::require_any_role` reads
`resource_access.<oidc_client_id>.roles` (`src/oidc/mod.rs`), matching
`FR-0014`/`FR-0015`'s role matrix. This is the one place the
provider-agnostic promise above doesn't hold — documented inline, not
silently assumed, per `NFR-0014`'s equivalent below.

`AuthClaims`, an axum `FromRequestParts` extractor, is the sole
integration point: a handler that doesn't take `AuthClaims` as a
parameter is public by default (`docs/nfrs/` "routes public by default"
below) — there is no global auth middleware to opt out of.

## Consequences

Authorization decisions live entirely in this backend, independent per
request — no shared session store, no central auth-service round trip
per call (beyond the amortized JWKS cache refresh). This is what lets
the same binary run unmodified across regions.

The cost, same as the Python original: any provider whose client-role
claim isn't shaped like Keycloak's `resource_access.<client>.roles`
needs `Claims::require_any_role`'s claim-path logic changed (or a second
lookup path added) before RBAC works against it — token validation
itself needs no such change. `AuthClaims` doesn't yet support scope-based
or other RBAC shapes; that's deferred until a second provider is actually
in scope.
