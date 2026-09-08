# 0008. Soft-delete Hero via an `archived_at` marker column, not a second table or row removal

## Status

Accepted

## Context

Ported from template-fastapi's `docs/adrs/0012`. `DELETE
/crud/v1/heroes/v2/json?id=` needs to be reversible in principle (a
future `restore` action) and needs deleted records to stay excluded from
normal reads without a second "trash" table to keep in sync.

## Decision

`heroes.archived_at` (`Option<NaiveDateTime>`, nullable, `NULL` = active)
is a plain column on the same row (`src/models/hero.rs`). `Repository::
delete` sets it to the current time rather than removing the row;
`Repository::list`/`get` filter `archived_at IS NULL` unless the caller
passes `?include_archived=true` (`src/views/hero.rs`'s `HeroListQuery`).
`update`/`delete` additionally refuse to act on an already-archived
record (both repository impls check `archived_at.is_none()` before
allowing a write) -- an archived hero is read-only until restored.

## Consequences

Every generic mechanism (pagination, owner-scoping) works against
archived rows for free, since they're ordinary rows with one extra
column, not a separate code path. `DELETE`'s 204 response is
indistinguishable from a hard delete to the caller -- the archived state
only becomes visible via `?include_archived=true`.

Deviation from the Python original, noted rather than silently dropped:
this phase does not build a `POST .../restore` action (clearing
`archived_at`) or the `Draftable`/`Schedulable`/`Lockable` mixins the
Python `Hero` also demonstrates -- Tier A's scope for this port is
narrower (`archived_at` only; see the plan this phase executed). The
column and the exclusion-by-default behavior exist and are tested
(`src/repositories/hero_memory.rs`'s `delete_is_soft_and_excluded_by_default`);
only the reversal endpoint is deferred.
