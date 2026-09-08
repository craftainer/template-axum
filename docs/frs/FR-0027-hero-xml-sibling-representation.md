# FR-0027. Provide an XML sibling representation of Hero's CRUD routes

## Status

Implemented

## Description

The system shall expose `/crud/v1/heroes/v2/xml`, supporting the same
list/get/create/update/delete/filter/sort/bulk operations as
`/crud/v1/heroes/v2/json` (`FR-0002`, `FR-0025`, `FR-0026`), reading
and writing XML request/response bodies instead of JSON, backed by the
same `CrudService`/repository dependency (no separate business logic).

## Source

Port of the reference's sibling-router pattern
(`docs/adrs/0005-multi-format-representations-via-sibling-routers.md`)
applied to Hero v2 specifically (the reference's own XML router exists
only for the deprecated Hero v1, out of scope per this plan's item 7).
See ADR 0014.

## Acceptance criteria

- `POST /crud/v1/heroes/v2/xml` with an XML body creates a record,
  returning `201` with an XML body and `Content-Type:
  application/xml`.
- `GET ?id=`/`GET ` (list, with the same filter/sort query parameters
  as the JSON router) return a single `<hero>` or a `<heroes>` wrapping
  repeated `<hero>` elements.
- `PATCH`/`DELETE` support the same single-record (`?id=`) and bulk
  (filter-only) forms as the JSON router, returning the same status
  codes.
- A malformed XML body, or a body that fails the same validation rules
  `HeroCreate`/`HeroUpdate` enforce for JSON, returns `422` as
  `application/problem+json` (errors stay uniform across both
  routers).
- RBAC and rate limiting apply identically to both routers (shared
  role constants and rate-limit scope).
- Verified by `controllers::heroes_xml::tests` (create/get/list/update/
  delete, bulk update/delete, malformed-XML and validation-failure
  422s, RBAC forbidden).
