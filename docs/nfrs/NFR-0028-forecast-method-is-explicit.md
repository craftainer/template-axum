# NFR-0028. A forecast response never implies a trained model

## Status

Implemented

## Attribute

Usability / transparency (preventing a client from over-trusting a
result).

## Description

Every `GET /crud/v1/heroes/v2/json/predict` response shall name its
forecasting method explicitly (`"linear_regression"`), so a client
cannot mistake a naive straight-line projection for a trained,
validated model's output.

## Source

Port of `docs/adrs/0017-linear-forecast-and-json-only-events-for-crud-
stats.md`'s "Forecasting" decision in template-fastapi. See ADR 0015.

## Verification

`views::stats::Prediction::method` is a `&'static str` fixed to
`"linear_regression"` at every construction site (there is exactly
one, `controllers::heroes::get_prediction`) -- not a caller-suppliable
or configurable value. No test asserts this can be overridden, since
the type itself makes that impossible to express.
