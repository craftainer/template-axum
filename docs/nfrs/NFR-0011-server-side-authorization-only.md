# NFR-0011. Enforce authorization entirely server-side

## Status

Implemented

## Attribute

Security.

## Description

Authorization decisions shall be made entirely by this backend,
independent per request; the frontend is trusted only to obtain and
attach a bearer token, never to assert its own permissions.

## Source

See ADR 0003.

## Verification

Manual/code review: every role check happens inside a handler via
`Claims::require_any_role` against the *verified* token's claims -- no
client-supplied header/body field is treated as an authorization
assertion anywhere in `src/controllers/`.
