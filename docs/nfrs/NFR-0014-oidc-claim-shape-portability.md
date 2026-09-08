# NFR-0014. Assume no claim beyond `sub` is portable across providers

## Status

Implemented

## Attribute

Portability.

## Description

Token-validation code shall not assume any claim beyond `sub` is present
on every provider's tokens; the Keycloak-specific
`resource_access.<client>.roles` lookup shall be isolated to RBAC code,
not mixed into signature/issuer/audience verification.

## Source

See ADR 0003.

## Verification

Manual/code review: `OidcVerifier::decode_bearer_token` never reads
`resource_access`; only `Claims::require_any_role`/`granted_roles` do,
and both are documented as Keycloak-specific in `src/oidc/mod.rs`'s
module doc.
