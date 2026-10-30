# 0030. Act as an OAuth 2.0 resource server: separate issuer and discovery URL, audience always on, RFC 9728 metadata, security headers, and the CRUD manifest with an api-catalog

## Status

Proposed

Amends [ADR 0003](0003-auth-strategy-provider-agnostic-oidc.md).

## Context

Three gaps in the bearer-token handling of ADR 0003, plus two discovery
documents the sibling services already publish:

- **One URL played two roles.** `OIDC_ISSUER_URL` was both the `iss` a token
  must carry and the host the app fetches discovery from. Once Keycloak is
  given a fixed public hostname (`KC_HOSTNAME`, BK2), tokens carry
  `http://localhost:8080/realms/template-axum` while the app can only reach
  `http://keycloak:8080/...` inside the compose network, so the single
  variable rejected every real token or could not fetch discovery.
- **The audience check was opt-in.** With `OIDC_AUDIENCE` unset, any token
  from the issuer was accepted for any resource, which RFC 9700 section 2.3
  names as a confused-deputy risk.
- **A 401 told a client nothing.** The challenge was a bare `Bearer`; an
  OAuth client had no way to learn which authorization server to use
  (RFC 9728).
- No security headers were set; `src/http_headers.rs` held only `Sunset`.
- `craftainer-crud` ships a manifest route and an RFC 9727 api-catalog
  (`craftainer_crud::manifest`), already mounted by template-fastapi, but
  this app did not mount them.

template-fastapi settled the same questions in its ADR 0026 (RS1); this ADR
ports that behaviour to Rust.

## Decision

We will:

1. **Split the issuer from the discovery URL.** `OIDC_ISSUER` is the expected
   `iss`, compared as an exact string. `OIDC_DISCOVERY_URL` is the full
   `.well-known/openid-configuration` URL the app (and the `oidc` health
   check) fetches. Each falls back to `OIDC_ISSUER_URL`, so an instance that
   sets only the legacy variable behaves as before.
2. **Always check the audience.** `OIDC_AUDIENCE` defaults to `api` and cannot
   be switched off; `exp`, `iss` and `aud` are required claims. A rejected
   token is the new `AppError::InvalidToken`, distinct from
   `AppError::Unauthorized` (no credentials at all).
3. **Publish RFC 9728 metadata** at `/.well-known/oauth-protected-resource`
   (`resource`, `authorization_servers`, `scopes_supported`,
   `bearer_methods_supported: ["header"]`, `resource_name`) and put
   `WWW-Authenticate: Bearer resource_metadata="<PUBLIC_BASE_URL>/..."` on
   every 401, with `error="invalid_token"` when a token was rejected. The URL
   comes from `PUBLIC_BASE_URL`, never from the `Host` header. `AppError`
   cannot know the URL, so a router layer
   (`problem_details::add_resource_metadata`) adds it to every 401 response,
   whatever produced it.
4. **Serve `security.txt`** (RFC 9116) only when `SECURITY_TXT_CONTACT` is set;
   `SECURITY_TXT_EXPIRES` is then required and at most a year ahead.
   `PUBLIC_BASE_URL` is required (and https) under `MODE=production`.
5. **Set baseline security headers on every response**, errors and the
   well-known documents included, with the values template-fastapi uses
   (`http_headers::security_headers`).
6. **Mount `craftainer-crud`'s manifest and api-catalog**:
   `GET /api/_crud/manifest`, filtered to the resources the caller's roles may
   read, with Hero v2 and v1 registered, and `GET /.well-known/api-catalog`
   anchored at `PUBLIC_BASE_URL`. The manifest's `oidc_discovery_url` is the
   public issuer's discovery URL.

The resource-server settings live in `config::ResourceServer`, read next to
`Settings` rather than inside it, and the `/.well-known/` routes in
`src/oidc/well_known.rs`: `.github/scripts/check_layering.py` has no category
for a new top-level module.

## Consequences

- A deployment that sets only `OIDC_ISSUER_URL` keeps working, but its tokens
  now need `aud` to contain `api` (or `OIDC_AUDIENCE` set to what its provider
  emits). The bundled realm's `api` and `bff` clients both add `api`.
- Behind a reverse proxy or in production, `PUBLIC_BASE_URL` is one more
  required setting; the cost of not trusting `Host`.
- The security headers include `Strict-Transport-Security` unconditionally,
  as template-fastapi does; browsers ignore it over plain HTTP.
- The api-catalog has no `service-desc` link: this app serves no OpenAPI
  document.
- The HTML views still accept `?token=` (a token in a URL); that is a
  separate finding, not changed here.
