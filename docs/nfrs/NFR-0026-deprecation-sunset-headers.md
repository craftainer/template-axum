# NFR-0026. Communicate deprecated-version sunset via standard headers

## Status

Proposed

## Attribute

Compatibility / API communication.

## Description

Once a deprecated API version exists in this app (Tier C item 7, out
of scope for the plan that implemented this mechanism -- see ADR 0012),
every route on it shall emit an RFC 8594 `Sunset` header (HTTP-date
format), a `Deprecation: true` header, and a `Link` header pointing at
the current-version equivalent path. Current-version routes shall carry
none of these headers.

## Source

Port of `docs/nfrs/NFR-0002-deprecation-sunset-headers.md` in
template-fastapi. Status is `Proposed`, not `Implemented`, because this
instance has no deprecated route yet to apply the mechanism to -- only
the mechanism itself (`FR-0024`) is implemented. Revisit this NFR's
status when Tier C item 7 (or its own separate plan, per this plan's
own text) adds a real deprecated route.

## Verification

Not yet verifiable end-to-end (no deprecated route exists). `FR-0024`'s
acceptance criteria cover the mechanism in isolation; once a deprecated
route exists, add the same automated assertion the reference makes
(`Sunset`/`Deprecation`/`Link` present on every deprecated route,
absent on every current-version one).
