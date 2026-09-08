# 0002. Use explicit CRUD-router and model-version URL segments

## Status

Accepted

## Context

template-fastapi's `docs/adrs/0009` establishes a URL shape with two
independent version numbers: a router/factory version (`ROUTER_VERSION`,
naming the shape the generic CRUD router factory produces) and a per-
resource model version (`v2` for the current Hero shape, `v1` for a
deprecated compat sibling), plus an explicit response-format segment,
composed as `/crud/v{router_version}/heroes/v{model_version}/{format}`.

Phase 2 of this Rust port builds Hero v2 only (the deprecated `v1`
compat router, and non-JSON formats, are explicitly out of scope for this
phase — see this repo's own `docs/plans/` note, folded away once this
work landed). The URL-shape *decision* still needs recording, though,
since it constrains every future resource this template gains: the two
version numbers and the trailing format segment are independent axes,
not folded together, even when only one value of each currently exists.

## Decision

Hero v2 is mounted at exactly `/crud/v1/heroes/v2/json` — matching
template-fastapi's own worked example path verbatim (`docs/TEMPLATE.md`'s
"Getting started"), so a reader already familiar with the reference
implementation recognizes the shape immediately. `1` here names this
port's own CRUD-router convention (a plain `Router` built from
`controllers::heroes::router()`, nested under `/crud/v1` in `main.rs`);
`2` names Hero's own DTO shape. Record addressing stays a query
parameter (`?id=`), never a path segment, matching template-fastapi's
`crud_router.py` convention — this keeps a future bulk update/delete
route (Tier C, deferred) representable without a second URL shape.

A future breaking change to Hero's own fields would add a `heroes/v3/`
sibling module in `views/`/`controllers/`, never mutate `heroes/v2/`'s
existing DTOs or database columns in place (`docs/nfrs/0003`).

## Consequences

Every resource this template gains after Hero follows the same three-
segment shape, so a client (or this repo's own future code) never has to
special-case one resource's URL structure against another's. The `/json`
suffix is mounted now with no sibling `/xml`/`/web` router (those are
Tier C/deferred, per this phase's plan) — a reader must not infer their
absence means the format segment is optional; it's reserved, not omitted.

Unlike the Python original, this phase never actually exercises the
"two independent version numbers" property (only `router_version=1`,
`model_version=2` exist), so the payoff of keeping them separate axes is
speculative until a second resource or a Hero v3 lands. It costs nothing
to keep them separate now and would cost a URL-breaking change to merge
them later, so the axes stay independent regardless.
