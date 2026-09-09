# FR-0033. Expose an audit endpoint reporting the caller's own identity and roles

## Status

Implemented

## Description

The system shall expose `GET /audit`, restricted to callers holding the
`security` or `detective` role, returning the caller's own subject and
the full set of roles granted to them for this app's OIDC client. Any
other caller (including one with no bearer token) shall be refused.

## Source

Port of the reference implementation's `FR-0016`. The `security`/
`detective` roles already exist in this app's role matrix (`FR-0015`,
`src/oidc/mod.rs`'s tests) but had no route consuming them before this.

## Acceptance criteria

- `GET /audit` returns `200` with `{"subject": "...", "roles": [...]}`
  for a caller holding `security` or `detective`.
- Every other role (and a request with no bearer token) is refused --
  `403` for a token with neither role, `401` for no token.
- `roles` reports every role the caller's token grants for this app's
  configured OIDC client (`resource_access.<client>.roles`), not only
  `security`/`detective`.
- Verified by `controllers::audit::tests`.
