# FR-0036. Publish resource-server metadata, discovery documents and security headers

## Status

Implemented

## Description

The system shall behave as an OAuth 2.0 resource server that tells clients
how to authenticate and what it offers:

- accept a bearer token only when its `iss` equals `OIDC_ISSUER` (falling
  back to `OIDC_ISSUER_URL`) and its `aud` contains `OIDC_AUDIENCE`
  (default `api`), fetching discovery from `OIDC_DISCOVERY_URL`;
- serve RFC 9728 metadata at `/.well-known/oauth-protected-resource`, and
  carry `WWW-Authenticate: Bearer resource_metadata="<url>"` on every 401;
- serve `/.well-known/security.txt` only when a contact is configured;
- serve the CRUD manifest at `/api/_crud/manifest` and the RFC 9727
  api-catalog at `/.well-known/api-catalog`;
- set baseline security headers on every response.

## Source

Security reviewers and API client authors (`../stakeholders.md`); the
operator who needs a vulnerability-reporting channel. Prompted
[ADR 0030](../adrs/0030-resource-server-metadata-security-headers-and-api-catalog.md);
port of template-fastapi's FR-0025.

## Acceptance criteria

- A token from the expected issuer without `api` in `aud`, or from another
  issuer, is rejected with 401 (`tests/keycloak_oidc.rs`, against Keycloak).
- With `OIDC_ISSUER` (public) and `OIDC_DISCOVERY_URL` (in-network) set to
  different hosts, a real Keycloak token verifies (`tests/e2e.rs`).
- `GET /.well-known/oauth-protected-resource` returns `resource`,
  `authorization_servers`, `scopes_supported`, `bearer_methods_supported` and
  `resource_name`, built from configuration.
- A 401 for a request with no token has
  `WWW-Authenticate: Bearer resource_metadata="<PUBLIC_BASE_URL>/.well-known/oauth-protected-resource"`;
  one for a rejected token adds `, error="invalid_token"`. The URL ignores
  the request's `Host` header. Verified over a real socket in `tests/e2e.rs`.
- `security.txt` returns 404 unless `SECURITY_TXT_CONTACT` is set; with it,
  `SECURITY_TXT_EXPIRES` is required and at most a year ahead, else startup
  fails. `PUBLIC_BASE_URL` is required under `MODE=production`.
- `GET /api/_crud/manifest` lists Hero v2 and v1 to a caller holding a hero
  read role and nothing to an anonymous caller or one without a hero role.
- `GET /.well-known/api-catalog` returns an `application/linkset+json`
  linkset anchored at `PUBLIC_BASE_URL` pointing at the manifest.
- Every response, including 401/404 and the `/.well-known/` documents,
  carries `Content-Security-Policy`, `X-Content-Type-Options`,
  `X-Frame-Options`, `Referrer-Policy`, `Strict-Transport-Security` and
  `Permissions-Policy`.
