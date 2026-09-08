# FR-0019. Return 404 for any get/update/delete against a nonexistent or inaccessible record

## Status

Implemented

## Description

The system shall return 404 (rendered per FR-0018) when `GET`/`PATCH`/
`DELETE ?id=` targets a record that does not exist, is archived (and
`?include_archived` was not set), or is not owned by the caller (for a
write).

## Source

Standard REST convention; see ADR 0007 (owner scoping does not leak
existence of another user's record).

## Acceptance criteria

- `list_or_get`/`update`/`delete_hero` (`src/controllers/heroes.rs`) map
  a repository `None`/`false` result to `AppError::NotFound`.
- A `PATCH`/`DELETE` against another user's Hero returns 404, not 403
  (ADR 0007's ownership-hiding property).
