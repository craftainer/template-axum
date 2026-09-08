# FR-0028. Provide aggregate statistics for Hero

## Status

Implemented

## Description

The system shall expose `GET /crud/v1/heroes/v2/json/stats`, returning
the total record count, per-numeric-field aggregate statistics (count/
min/max/average/total), a categorical value distribution (empty for
Hero, which has no boolean/enum field), an optional time-bucketed
record-count series (when `?bucket=day|week|month` is given), and a
lifecycle breakdown (archived-record count).

## Source

Port of `app.controllers.crud_router`'s `stats_enabled=True` `/stats`
route. See ADR 0015.

## Acceptance criteria

- `GET /stats` with no query params returns `total`, `numeric` (`id`
  and `power_level` aggregates), `categorical` (`[]`), and `lifecycle`,
  with no `time_series` key.
- `GET /stats?bucket=day` additionally returns `time_series`, one entry
  per UTC calendar day with at least one matching record.
- `GET /stats?bucket=<invalid>` returns `422`.
- `GET /stats?include_archived=true` includes archived records in
  `total` and the aggregates.
- Gated by the same read-role requirement as the plain `GET` list
  route (`FR-0015`).
- Verified by `controllers::heroes::tests::stats_reports_total_and_numeric_aggregates`/
  `stats_includes_a_time_series_when_bucket_is_given`/
  `stats_rejects_an_unrecognized_bucket_with_422`/
  `stats_and_predict_require_a_read_role`.
