# NFR-0026. Communicate deprecated-version sunset via standard headers

## Status

Implemented

## Attribute

Compatibility / API communication.

## Description

Every route on a deprecated API version in this app shall emit an RFC
8594 `Sunset` header (HTTP-date format), a `Deprecation: true` header,
and a `Link` header pointing at the current-version equivalent path.
Current-version routes shall carry none of these headers.

## Source

Port of `docs/nfrs/NFR-0002-deprecation-sunset-headers.md` in
template-fastapi. `FR-0031`/`FR-0032` (`docs/adrs/0017`) gave this
mechanism (`FR-0024`, `docs/adrs/0012`) its first real deprecated
routes: `controllers::heroes_v1`/`heroes_v1_xml`.

## Verification

`controllers::heroes_v1::tests::
every_response_carries_sunset_deprecation_and_link_headers` and
`controllers::heroes_v1_xml::tests::
create_returns_201_with_an_xml_body_and_sunset_headers` assert the
headers are present on the deprecated routers; `controllers::heroes::
tests::current_version_responses_carry_no_deprecation_headers` and
`controllers::heroes_xml::tests::
current_version_responses_carry_no_deprecation_headers` assert their
absence on the matching current-version routers -- the same automated
split assertion the reference makes.
