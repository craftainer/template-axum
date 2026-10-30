# 0028. Hero writes are single `UPDATE ... RETURNING` statements; bulk operations are set-based

## Status

Proposed

## Context

`HeroSeaOrmRepository` did every write as read-modify-write:

- `update` and `delete` ran a `SELECT` (by id, owner and `archived_at IS
  NULL`) and then an `UPDATE` of the row they had read.
- `update_many` and `delete_many` ran one `SELECT` for the matching rows and
  then one `UPDATE` per row, so a bulk call of N rows cost N + 1 round
  trips, each in its own implicit transaction.

Two things were wrong with that besides the count. The visibility check and
the write were separate statements, so a delete (or an update by another
request) landing between them was overwritten as if the row had still been
live. And a bulk call that failed part-way left some rows changed and some
not.

Decision log, 2026-10-30 (AX15): run it after the AX14 cache A/B and keep it
on correctness (single-statement `UPDATE ... RETURNING`, set-based
`update_many`/`delete_many`) even without a measured throughput win.

## Decision

Each write is one statement built with SeaORM's `Entity::update_many()`, a
`SET` list from an `ActiveModel` (only the fields the request carries, plus
`updated_at`; the soft delete sets `archived_at` and `updated_at`), the same
`WHERE` the `SELECT` had, and `exec_with_returning` (Postgres `RETURNING *`,
sea-orm 2.0.2):

| Method | `WHERE` | Result |
| --- | --- | --- |
| `update` | `id`, `owner_id`, `archived_at IS NULL` | first returned row, `None` on no match |
| `delete` | `id`, `owner_id`, `archived_at IS NULL` | `true` if a row was returned |
| `update_many` | `archived_at IS NULL` and the filters | returned rows |
| `delete_many` | `archived_at IS NULL` and the filters | returned rows |

`apply_filters` became generic over `QueryFilter` so the same filter mapping
serves `Select` and `UpdateMany`. `RETURNING` does not order rows, so the bulk
methods sort by `id` (what the per-row loops returned in practice).

Behaviour is unchanged: the same values (`None` / `false` / empty `Vec` on a
miss), the same filters and owner scoping, and the same `updated_at` stamping
that the ETag is hashed from (`docs/adrs/0023`). The repository has no
version check, revision or audit side effect of its own. Events are published
by the handlers from the returned rows, as before, and the bulk size cap
(`docs/nfrs/NFR-0027`) is enforced in the controller; neither moved. The
transaction boundary is stronger: one statement is atomic, so a bulk write
now changes all matching rows or none.

Before and after, statements sent to Postgres (`tests/postgres_hero_round_trips.rs`
counts them with sea-orm's metric callback):

| Operation | Before | After |
| --- | --- | --- |
| `update`, row found | 2 | 1 |
| `update`, miss | 1 | 1 |
| `delete`, row found | 2 | 1 |
| `delete`, miss | 1 | 1 |
| `update_many`, N rows | N + 1 | 1 |
| `delete_many`, N rows | N + 1 | 1 |

Nothing was kept as-is: every path could be expressed as one statement
without changing what it returns.

## Consequences

- Lost-update window closed for visibility: an update can no longer write to a
  row archived after it was read. `tests/postgres_hero_round_trips.rs` races
  updates against a delete and checks the archived row is never touched
  afterwards.
- A bulk call holds its row locks for one statement instead of taking them
  row by row, and a large `update_many` is one longer statement rather than N
  short ones. The controller's size cap bounds it.
- No throughput gain is claimed. Whether the saved round trips show up under
  load is a separate benchmark run; the change stands on the correctness
  properties above (decision log).
- The `Mode::Mock` repository is untouched: it already mutates under one lock.
