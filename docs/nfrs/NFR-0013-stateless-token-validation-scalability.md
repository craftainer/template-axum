# NFR-0013. Validate tokens statelessly, per backend instance

## Status

Implemented

## Attribute

Scalability.

## Description

Bearer-token validation shall require no shared session state across
backend instances; the same code shall run unmodified across a
multi-region deployment, with only `OIDC_ISSUER_URL` varying.

## Source

See ADR 0003.

## Verification

Manual/code review: `OidcVerifier` holds only a per-instance JWKS cache
(`tokio::sync::RwLock`, in-process); no shared cache/session store
(Redis, a database table) backs token validation.
