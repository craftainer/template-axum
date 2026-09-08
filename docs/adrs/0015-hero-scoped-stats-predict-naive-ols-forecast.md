# 0015. Compute /stats and /predict directly against Hero, with a naive OLS forecast

## Status

Accepted

## Context

Tier C item 5 ports two parts of the reference's `docs/adrs/0017-
linear-forecast-and-json-only-events-for-crud-stats.md`: `GET
<prefix>/stats` (count/numeric/categorical/time-series/lifecycle
aggregates) and `GET <prefix>/predict` (an OLS trend forecast), added
generically to `app.controllers.crud_router`'s router factories as an
opt-in flag (`stats_enabled=True`) so any resource built on those
factories gets both routes for free. The reference's `Repository`
Protocol itself grows a `stats()` method every concrete repository
(`SQLAlchemyRepository`/`InMemoryRepository`) implements, and
`app.controllers.crud_stats` sits above that as resource-agnostic
query parsing plus the forecasting math.

This instance has one resource (Hero). Adding a `stats()` method to
the `Repository` trait (and to `dyn_repository!`'s generated
passthrough) the way `docs/adrs/0013` added `count`/`update_many`/
`delete_many` would be building a second-resource-ready abstraction
with a single caller -- exactly the premature-genericity the reference
`docs/adrs/0001` warns against, applied here to a capability (not the
CRUD verbs `docs/adrs/0001`/`0013` already justify generically because
every resource needs them). `docs/adrs/0013`'s own filter/sort/bulk
work stayed generic because it's mechanically derived from a schema and
reused unchanged by every resource; `/stats`/`/predict`'s numeric-
field/time-bucket aggregation, by contrast, still needs resource-
specific field access (which of Hero's fields are summable, how to read
`created_at` off a `hero::Model`) no matter how the surrounding query-
parsing and math are structured.

## Decision

- `controllers::crud_stats` holds the resource-agnostic parts only:
  `TimeBucket` (parse/format), `numeric_fields`/`categorical_fields`
  (narrowing `crud_query::FieldSpec` the same way the reference's
  `crud_stats.py` narrows `field_specs`), `parse_bucket`/
  `parse_predict_field`/`parse_periods`, `bucket_start`/`advance` (UTC
  calendar-bucket truncation/advancement), and `forecast` (the OLS
  regression itself) -- none of this touches `hero::Model` directly.
- `controllers::heroes.rs` owns the one resource-specific seam:
  `numeric_value(hero: &hero::Model, field: &str) -> Option<f64>`/
  `boolean_value` (mirrors `hero_sea_orm.rs`'s `column_for` -- the one
  place a storage-agnostic field name becomes a concrete field access),
  `numeric_stat`/`time_series` (aggregation over a `&[hero::Model]`
  slice), and the two route handlers (`get_stats`/`get_prediction`)
  that call `crud_stats`'s generic pieces and `CrudService::list`/
  `count`. A future second resource would add its own `numeric_value`/
  `boolean_value`/handlers, reusing every generic piece in
  `crud_stats.rs` unchanged -- if and when that duplication actually
  exists twice, *that* is the point to lift it into a trait
  (`docs/adrs/0001`'s own "wait for the second call site" reasoning,
  applied here rather than only to genuinely bespoke logic).
- **Aggregation reads records, not pushed-down SQL aggregates.** Both
  `/stats` and `/predict` call the existing `CrudService::list`
  (capped at `crud_stats::MAX_HISTORY_RECORDS`, matching the
  reference's own cap and its "demo/reporting feature, not a paginated
  listing" reasoning) and compute count/min/max/avg/sum/time-buckets in
  Rust over the fetched `Vec<hero::Model>`, rather than each backend
  pushing `AVG()`/`GROUP BY date_trunc(...)` down to SQL the way the
  reference's `SQLAlchemyRepository.stats` can. This avoids needing a
  second, `stats`-specific SeaORM query-building surface (on top of
  `hero_sea_orm.rs`'s existing filter/sort one) for a capability with
  exactly one caller, at the cost of the same records being fetched
  twice by `/predict` when it needs both a total (`CrudService::count`,
  pushed down) and per-bucket sums (fetched and summed in Rust) -- an
  acceptable trade given the record cap.
- **Forecasting stays a plain OLS regression, stdlib arithmetic only**
  -- direct port of the reference's own `forecast()`, same reasoning
  (`docs/adrs/0017`'s "Forecasting" section): no ML crate, no trained-
  model storage/retraining/versioning surface. Every `/predict`
  response's `method` field is the literal string `"linear_regression"`
  (`views::stats::Prediction`) so a client can't mistake it for a
  fitted model's forecast. `InsufficientHistory` (fewer than 2 time
  buckets) surfaces as `422` with a typed `FieldError`, matching every
  other caller-input problem in this layer.
- **`LifecycleStats` is narrower than the reference's.** The
  reference's `LifecycleStats` has five optional fields (one per
  record-lifecycle mixin: Archivable/Draftable/Schedulable/Lockable).
  Hero in this port only carries `Archivable` (`models::hero`'s own
  doc comment already records this scope cut from phase 2), so
  `views::stats::LifecycleStats` has one field, `archived: u64`
  (always populated, not optional -- there's no "mixin absent" case to
  represent here the way there is for a resource that might carry zero
  record-lifecycle mixins).
- **No `/events` SSE route yet** -- the reference's other `docs/adrs/
  0017` decision (JSON-payload SSE frames even under the XML router) is
  Tier C item 6 in this plan's own numbering, not part of this item.
- **JSON only** -- `/stats`/`/predict` are mounted on
  `controllers::heroes`'s router only, not `heroes_xml`'s. The
  reference hand-assembles nested XML for both routes
  specifically because `xml_codec.to_xml` only supports flat models
  (see `views/stats.py`'s own docstring on why it's shaped as flat
  list-of-item sub-models); replicating that nested-XML assembly was
  judged not worth the cost for a demo/reporting route with zero
  reference parity requirement forcing it (Hero v2 has no XML `/stats`
  in the reference either -- there is no XML router for Hero v2 at
  all, only the deprecated v1's, out of scope per this plan's item 7).

## Consequences

Adding `/stats`/`/predict` cost no changes to `Repository`/
`CrudService`/`dyn_repository!` -- the generic CRUD/filter/sort/bulk
layers `docs/adrs/0001`/`0013` established stay exactly as they were.
The cost: `controllers::heroes.rs`'s `numeric_value`/`boolean_value`
are Hero-specific field-access code that a second resource wanting
stats would have to write its own copy of (not shared via a trait) --
a real, documented duplication risk if a second resource is ever
added, deliberately deferred rather than solved speculatively.

`/predict`'s "happy path" (a real, multi-day forecast) is unit-tested
directly (`crud_stats::tests::forecast_projects_a_straight_line_trend`
and friends control `bucket_start`/series values without needing real
distinct calendar days) rather than through `controllers::heroes::
tests`' HTTP harness, which stamps every record with the actual
process time -- multiple heroes created within one fast test run
always land in the same day bucket, so an end-to-end 200-with-real-
predictions test isn't practical without either injecting a fake
clock (not built this phase) or manipulating `created_at` directly
(not exposed by `HeroCreate`/`HeroUpdate`, by design -- ADR 0007).
`controllers::heroes::tests` instead covers the full request-parsing/
routing/RBAC path via the (realistic, always-reachable) insufficient-
history 422 case.
