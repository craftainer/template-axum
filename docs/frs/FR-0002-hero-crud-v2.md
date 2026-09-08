# FR-0002. Provide Hero v2 CRUD operations

## Status

Implemented

## Description

The system shall expose `/crud/v1/heroes/v2/json` supporting: list
(`?skip=`/`?limit=`, default limit 100, max 1000), create (`POST` -> 201),
get by id (`GET ?id=`), partial update (`PATCH ?id=`), and delete
(`DELETE ?id=` -> 204).

## Source

This template's worked CRUD example; see ADR 0001, ADR 0002.

## Acceptance criteria

- `GET /crud/v1/heroes/v2/json` with no `?id=` returns a JSON array.
- `GET /crud/v1/heroes/v2/json?id=<n>` returns the record or 404.
- `POST` with a valid body returns 201 and the created record.
- `PATCH ?id=` updates only the fields supplied.
- `DELETE ?id=` returns 204 and excludes the record from subsequent
  reads (see ADR 0008).
- Verified end-to-end in this phase's `MODE=mock` smoke test (create then
  list round-trip) and manually against a real Postgres instance.
