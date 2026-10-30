# oidc/

Provider-agnostic OIDC bearer-token validation plus Keycloak client-role
RBAC — see `docs/adrs/0003-auth-strategy-provider-agnostic-oidc.md`.

- `OidcVerifier` — fetches OIDC discovery + JWKS from
  `ResourceServer::discovery_url`, caches the key set for 5 minutes, and
  verifies a bearer token's signature, exact issuer, expiry and
  audience (always; `OIDC_AUDIENCE`, default `api`). A rejected token is
  `AppError::InvalidToken`. Under `Mode::Mock`, verification is skipped
  entirely (`FR-0017`).
- `well_known` — the unauthenticated `/.well-known/` routes
  (`oauth-protected-resource`, `security.txt`) and `ManifestCallers`, the
  `CallerResolver` behind `GET /api/_crud/manifest` (`docs/adrs/0030`).
  Here, not a top-level module, because
  `.github/scripts/check_layering.py` has no category for a new one.
- `Claims` — the decoded token, kept as a raw `serde_json::Value` (never
  assumes any claim beyond `sub` — `docs/nfrs/0014`).
  `Claims::require_any_role(client_id, roles)` is the Keycloak-specific
  RBAC check (`resource_access.<client_id>.roles`), isolated from the
  provider-agnostic verification path above.
- `AuthClaims` — an axum `FromRequestParts` extractor; a handler that
  takes it as a parameter requires auth, one that doesn't stays public
  (`docs/nfrs/0012`).

`HasOidcVerifier` is the trait `AppState` implements so `AuthClaims` can
be generic over any state type carrying an `OidcVerifier`, rather than
hard-coding `controllers::AppState` into `oidc/` (which would violate
this repo's own layering: `oidc` sits below `controllers`).
