# FR-0014. Gate a route by the bearer token's client roles

## Status

Implemented

## Description

The system shall grant access to a role-gated route only if the bearer
token's `resource_access.<oidc_client_id>.roles` claim intersects that
route's required role set; otherwise it shall return 403.

## Source

Keycloak client-role RBAC convention; see ADR 0003.

## Acceptance criteria

- `Claims::require_any_role` (`src/oidc/mod.rs`) returns
  `AppError::Forbidden` when no required role is granted.
- Every failed role check logs a `tracing::warn!` with the subject and
  required roles.
- Verified: a `POST /mock/token` with `roles: ["editor"]` can create a
  Hero (write-role gated); this phase's smoke test used
  `["editor","maintainer"]`.
