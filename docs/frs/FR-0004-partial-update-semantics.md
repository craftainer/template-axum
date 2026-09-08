# FR-0004. Apply only explicitly-supplied fields on PATCH

## Status

Implemented

## Description

The system shall never overwrite a field with a default value on
`PATCH`; only fields present in the request body are applied to the
stored record.

## Source

Standard partial-update semantics; see ADR 0001.

## Acceptance criteria

- `HeroUpdate`'s fields are all `Option<T>`; `None` (an omitted JSON key)
  leaves the corresponding column untouched in both
  `HeroSeaOrmRepository::update` and `HeroMemoryRepository::update`.
- A `PATCH` body containing only `{"power_level": 7}` does not change
  `name`/`powers`.

## Known limitation

Unlike the Python original (which distinguishes "field omitted" from
"field explicitly set to `null`" via Pydantic's `exclude_unset`), this
port's plain `Option<T>` fields cannot represent "explicitly clear this
field" -- `None` always means "leave unchanged". Explicitly nulling a
previously-set `name`/`powers`/`power_level` via `PATCH` is out of scope
for this phase; see `src/views/hero.rs`'s module doc.
