# NFR-0012. Keep routes public unless they explicitly opt into auth

## Status

Implemented

## Attribute

Security / least-surprise.

## Description

A route shall require authentication only if its handler declares the
`AuthClaims` extractor as a parameter; there is no global auth
middleware a route must opt out of.

## Source

See ADR 0003.

## Verification

Manual/code review: `/health/live` and `/health/ready` take no
`AuthClaims` parameter and are reachable unauthenticated (verified in
this phase's smoke test); every Hero handler does take `AuthClaims`.
