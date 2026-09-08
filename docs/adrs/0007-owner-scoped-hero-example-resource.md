# 0007. Scope Hero's writes to the caller's own records, reads open to every authenticated caller

## Status

Accepted

## Context

Ported from template-fastapi's `docs/adrs/0011`. The generic
`CrudService`/`Repository` layer (ADR 0001) is deliberately unaware of
per-user ownership -- adding it there would make every future resource
pay for a concept most won't need. Hero is this template's worked
example of a resource that *does* need it: every authenticated user
should be able to browse every hero, but only mutate their own.

## Decision

Ownership scoping lives in `controllers::heroes`, not in
`repositories`/`crud`: every mutating handler (`create`/`update`/
`delete_hero`) passes `claims.subject()` (the token's `sub` claim) as
`owner_id` into `Repository::create`/`update`/`delete`. Both
`HeroSeaOrmRepository` and `HeroMemoryRepository` filter `update`/
`delete` by `owner_id == owner_id` themselves (returning `None`/`false`
for a record that exists but isn't the caller's -- indistinguishable
from "not found", so a write attempt against someone else's hero doesn't
leak that the record exists). `create` always stamps `owner_id` from the
verified token, never from client-supplied JSON (`HeroCreate` has no
`owner_id` field at all -- the caller cannot even attempt to set it).
`list`/`get` take no `owner_id` and return every hero regardless of
owner, matching Hero's read-open choice.

## Consequences

Hero demonstrates the pattern a future per-user resource follows:
owner-filter the two repository methods that mutate, stamp the create
payload, leave `list`/`get` alone. Unlike the Python original's
`OwnerScope(read_scoped: bool)` toggle (a single flag flipping between
"reads open" and "reads owner-scoped" on one shared abstraction), this
port has no such toggle -- `HeroSeaOrmRepository`/`HeroMemoryRepository`
hard-code the read-open behavior directly, since Hero is the only
resource built this phase and a real second data point is what should
drive whether that toggle is worth generalizing. A future resource
needing owner-scoped *reads* adds that filter in its own repository
`impl`, not by extending `Repository`'s generic surface.
