# FR-0031. Serve a deprecated Hero v1 CRUD API alongside the current v2 shape

## Status

Implemented

## Description

The system shall expose `GET`/`POST`/`PATCH`/`DELETE
/crud/v1/heroes/v1/json` as a deprecated, `superpower: String`-shaped
compat sibling of the current `/crud/v1/heroes/v2/json` resource,
backed by the exact same underlying Hero records -- not a second table
or a separately-maintained copy. `superpower` replaces v2's `powers:
Vec<String>`; every other field (`name`, `power_level`, `owner_id`,
timestamps) is identical between the two shapes.

## Source

Port of the reference implementation's `crud_1/heroes/heroes_v1.py`
(reference `FR-0005`). Depends on `docs/adrs/0017` for the storage-
reuse and conversion decision, and reuses `docs/adrs/0002`'s URL-
versioning shape, `docs/adrs/0013`'s filter/sort/bulk mechanism, and
`docs/adrs/0007`/`0008`'s ownership/soft-delete behavior unchanged.

## Acceptance criteria

- `GET`/`POST`/`PATCH`/`DELETE /crud/v1/heroes/v1/json` behave
  identically to their `/v2/json` equivalents for role checks
  (`FR-0015`), ownership scoping, rate limiting, filtering/sorting, and
  bulk update/delete -- only the request/response field shape differs.
- A record created via v1 is immediately visible (with its full
  `powers` list) via `/v2/json`, and vice versa -- both routers read
  and write the same record.
- `POST`/`PATCH` accept `superpower` (a single string, 1-200
  characters, same validation as v2's `name`); `GET`/list responses
  render `superpower` as `powers[0]` (`None` when `powers` is empty or
  `None`).
- A `PATCH` that omits `superpower` leaves the record's existing
  `powers` unchanged (mirrors `FR-0004`'s existing v2 contract).
- A create/update/delete through v1 publishes the same CRUD event
  (`FR-0030`) a v2 mutation would, onto the same resource topic.
- Verified by `controllers::heroes_v1::tests` and `views::hero_v1::
  tests`.
