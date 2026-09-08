# FR-0029. Provide a linear-regression trend forecast for Hero

## Status

Implemented

## Description

The system shall expose `GET /crud/v1/heroes/v2/json/predict`,
projecting `?periods=` (default 4, 1-52) future time buckets of either
record count (the default) or a `?field=`-named numeric field's
per-bucket sum, via an ordinary-least-squares linear regression over
existing history, bucketed by `?bucket=day|week|month` (default
`day`).

## Source

Port of `app.controllers.crud_router`'s `stats_enabled=True` `/predict`
route and `app.controllers.crud_stats.forecast`. See ADR 0015.

## Acceptance criteria

- The response's `method` field is always the literal string
  `"linear_regression"`.
- `field` is `null` when forecasting record count, or the requested
  field name when forecasting a numeric field's sum.
- Fewer than 2 time buckets of history returns `422` with a
  `FieldError` naming how many buckets were actually available.
- `?field=<a non-numeric field>` returns `422`.
- `?periods=` outside `1..=52`, or not an integer, returns `422`.
- Gated by the same read-role requirement as `/stats`.
- Verified by `controllers::heroes::tests::
  predict_below_two_buckets_of_history_returns_422`/
  `predict_rejects_a_non_numeric_field_with_422`/
  `predict_rejects_periods_out_of_range_with_422`/
  `predict_reports_the_linear_regression_method_and_field_on_success_shape`,
  and the regression math itself (multi-bucket projection, month/week/
  day bucket advancement) by `controllers::crud_stats::tests`.
