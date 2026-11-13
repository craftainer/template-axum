# FR-0015. Enforce Hero's role matrix

## Status

Implemented

## Description

The system shall allow `viewer`/`editor`/`maintainer`/`detective` roles
to read Hero records, `editor`/`maintainer` to create/update, and
`maintainer` alone to delete.

## Source

Mirrors template-fastapi's Hero role matrix; see ADR 0003.

## Acceptance criteria

- `src/hero/controllers/mod.rs`'s `HERO_READ_ROLES`/`HERO_WRITE_ROLES`/
  `HERO_DELETE_ROLES` constants match this matrix exactly.
- `list_or_get` requires a read role; `create`/`update` require a write
  role; `delete_hero` requires `maintainer`.
- Reading archived (soft-deleted) rows -- `include_archived=true` or
  `only_archived=true` on a list, a single read, `/stats` or the `If-Match`
  pre-read of a write -- requires `maintainer` (`HERO_ARCHIVE_READ_ROLES`,
  ADR 0031); any other read role gets `403`. Verified by
  `controllers::heroes::tests::archived_rows_are_readable_only_with_an_archive_role`.
