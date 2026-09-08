# 0013. Put generic filter/sort/bulk-action logic in the shared CRUD/repository layer

## Status

Accepted

## Context

Tier C item 3 ports template-fastapi's `docs/adrs/0008`: Hero's list
route only supported `?skip=`/`?limit=` pagination and `?id=`-addressed
single-record get/update/delete. Adding per-field filtering, sorting,
and single/bulk update/delete meant deciding where that logic lives --
the same question ADR 0008 answers for the reference, and this ADR
answers the same way for the same reason: the capability being added is
*mechanically derivable* from Hero's own DTO (a numeric field always
gets `eq`/`min`/`max`/`in`, a string field always gets `eq`/`contains`/
`icontains`), not a bespoke, resource-specific search endpoint. A
resource-by-resource implementation would mean re-deriving the same
logic per controller -- exactly the duplication `docs/adrs/0001`
(this instance's own MVC-layering ADR) already avoids for plain CRUD.
This ADR narrows that ADR's "bespoke query logic belongs in the
controller" guidance the same way the reference's ADR 0008 narrows its
own equivalent, and `crud/README.md` is updated accordingly.

Two things make a faithful, line-by-line port of `repositories/
filtering.py`/`controllers/crud_query.py`/`controllers/crud_actions.py`
impossible, and both are documented here rather than silently dropped:

- **No Pydantic-style runtime schema reflection.** `crud_query.py`'s
  `field_specs()` walks a Pydantic `BaseModel`'s `model_fields` at
  runtime to classify every field by its Python type annotation. Rust
  has no equivalent without a proc-macro crate this phase doesn't pull
  in, so `controllers::heroes::HERO_FIELD_SPECS` is a hand-written
  table -- still mechanically derived (one entry per scalar field,
  never a business rule about what's filterable), just derived by the
  author's hand instead of by a runtime walk. `powers` (a
  `Vec<String>`) has no entry, matching `field_specs()` skipping any
  field type outside its known kind table.
- **No `FilterOp::Regex`.** `field__regex=` reaches Postgres's `~`
  operator or Python's `re.search` verbatim in the reference, which its
  own comment calls out as a ReDoS vector bounded only by a runtime
  evaluation budget (`InMemoryRepository`'s alarm timeout,
  `SQLAlchemyRepository`'s per-transaction `statement_timeout`). That
  budget infrastructure is out of scope for this item; shipping the
  operator without it would be a real, unbounded-worst-case
  vulnerability, not a faithful port. `repositories/filtering.rs`'s
  module doc records this.

## Decision

The same layers as the reference, translated to this app's shape:

- `repositories/filtering.rs`: `FilterOp`/`FilterClause`/`FilterValue`/
  `SortClause` -- plain, storage-agnostic value objects, interpreted by
  each concrete `Repository` impl itself (`hero_sea_orm.rs` maps a
  clause onto a SeaORM `Column`/`Condition` via a small `field ->
  Column` table; `hero_memory.rs` applies it directly against a
  `hero::Model` field via a matching `matches_one`/`field_cmp` pair).
  `FilterValue` exists because Rust has no direct equivalent of
  Python's dynamically-typed `Any` a clause's value needs to hold
  across different field types.
- `Repository`/`CrudService` gain `filters`/`sort` on `list` (via
  `ListOptions`, no longer `Copy` since it now owns `Vec`s), plus
  `count`/`update_many`/`delete_many` -- pure passthrough on the
  `Repository` trait side, matching every other method already there.
  `CrudService::update_many`/`delete_many` inject an `owner_id`
  equality `FilterClause` before delegating, mirroring `CRUDInterface.
  _scoped`'s ownership scoping in the reference (ADR 0007/0011)
  without repositories needing any owner-specific code of their own.
- `controllers::crud_query`: `FieldSpec`/`parse_filters`/`parse_sort`,
  generic over any resource's field table (only the table itself,
  `HERO_FIELD_SPECS`, is resource-specific) -- the wire format (`field=`,
  `field__min=`/`field__max=`/`field__in=`/`field__contains=`/
  `field__icontains=`, `sort=a,-b`) matches the reference exactly.
- `controllers::crud_actions`: `resolve_list_or_get`/`resolve_update`/
  `resolve_delete`, generic over `R: Repository` (plus `R::Model:
  HasId` for the two that report which ids a bulk action touched) --
  the "id present -> single record; otherwise -> filtered list, or a
  bulk action over the given filters" decision in one place, reusable
  by a future sibling router (item 4) the same way `crud_actions.py`
  serves both `build_json_router` and `build_xml_router`.
- `models::HasId`: a one-method trait (`fn id(&self) -> i32`) letting
  `crud_actions` report affected ids without needing resource-specific
  knowledge of a concrete `Model`'s shape -- defined in `models` (the
  lowest layer) so a resource's own model file can implement it
  without `models` needing to see `repositories`/`crud` above it.
- `views::bulk`: `BulkUpdateResult`/`BulkDeleteResult` -- direct port of
  `views/bulk.py`, resource-agnostic.
- A bulk update/delete with no matching filters still requires at
  least one filter or an `?id=` (422 otherwise, `UnprocessableEntity`),
  and is capped by `Settings::bulk_action_max_matched` (default 1000,
  env `BULK_ACTION_MAX_MATCHED`) checked via `count()` before the
  mutation runs -- both match `_check_bulk_action_size`/the
  `RequestValidationError` `_NO_TARGET_ERROR` in the reference.

## Consequences

`/crud/v1/heroes/v2/json` gained filtering, sorting, and single/bulk
update/delete without `controllers::heroes` growing any parsing or
decision logic of its own beyond wiring `HERO_FIELD_SPECS` and calling
the shared `crud_query`/`crud_actions` functions -- the same "the
resource controller barely changes" payoff the reference's ADR 0008
reports for `app/controllers/heroes.py`.

The cost mirrors the reference's own: `Repository`/`CrudService` carry
more surface area, and a future resource needs its own `field ->
Column`/`field -> Model field` mapping in each backend impl (unavoidable
without Rust runtime reflection) plus its own `FIELD_SPECS` table. The
`DELETE ?power_level=5` bulk-delete route now returns `200` with a
`BulkDeleteResult` body instead of always `204` -- a real, intentional
behavior branch (there's no single record's absence to signal with an
empty body for a bulk action), documented in `heroes.rs`'s own handler
doc comment.

`FilterOp::Ne`/`Lt`/`Gt` exist (each concrete repository's match arms
handle them) but are never constructed by `crud_query`'s wire-format
parser -- like the reference's `FilterOp`, the enum is the complete
storage-agnostic vocabulary a `Repository` impl interprets, not only
the subset the current query-string suffix table happens to expose.

`hero_sea_orm.rs`'s filter/sort/bulk code paths are exercised only by
this item's unit tests' pure-function coverage of the field/column
mapping and condition-building helpers -- `tests/README.md`'s existing
"don't reach a real Postgres from a `src/` unit test" rule means the
actual SeaORM query execution (a live `Entity::find()...filter(...)`
round-trip) is left for the not-yet-built integration tier, the same
documented gap `hero_sea_orm.rs`'s other methods already carry.
