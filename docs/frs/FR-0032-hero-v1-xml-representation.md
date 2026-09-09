# FR-0032. Serve the Hero v1 compat resource as XML as well as JSON

## Status

Implemented

## Description

The system shall expose `GET`/`POST`/`PATCH`/`DELETE
/crud/v1/heroes/v1/xml` as the XML sibling representation of
`FR-0031`'s Hero v1 compat resource, following the same sibling-router
pattern `FR-0027` establishes for the current (v2) Hero resource.

## Source

Port of the reference implementation's `FR-0008`'s v1 half (the v1
compat resource's XML representation). Depends on `FR-0031` and reuses
`docs/adrs/0014`'s `quick-xml` serde mechanism and `docs/adrs/0017`'s
v1 conversion.

## Acceptance criteria

- `/crud/v1/heroes/v1/xml` supports the same operations as `/crud/v1/
  heroes/v1/json`, sharing the same underlying records and business
  logic -- only (de)serialization differs.
- A single record renders as `<hero>...</hero>`; a list renders as
  `<heroes><hero>...</hero>...</heroes>`, matching `FR-0027`'s existing
  v2 XML shape with `superpower` in place of `powers`.
- Malformed XML or a failed validation still renders as
  `application/problem+json` (`FR-0018`), uniformly with every other
  router in this app.
- Verified by `controllers::heroes_v1_xml::tests` and `views::
  hero_v1_xml::tests`.
