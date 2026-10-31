# 0029. Record product-analytics events with the library's `track()`, privacy-first

## Status

Proposed

## Context

The plan in `docs/plans/2026-10-bff-wellknown-analytics-owasp.md` adds product
analytics across the template family: a self-hosted, cookieless
[Umami](https://umami.is) in the devcontainer stack
(`.devcontainer/stack/umami/`), and one `track()` contract that the Python and Rust
backends and the BFF all call. This template needs to emit a few server-side domain
events, without turning the API into a tracking surface.

`craftainer-observability` 0.4 ships that contract as its `analytics` feature
(`craftainer_observability::analytics`, its ADR 0007). It validates events, applies the
`ANALYTICS_*` environment, and delivers through adapters (`none`, `log`, `umami`, `otel`)
behind a bounded queue. It cannot see HTTP, so what a request says about tracking
(Global Privacy Control, Do Not Track, consent) has to be handed to it by the
application, through a per-request context it provides.

## Decision

We will:

- Enable the `analytics` feature of `craftainer-observability` 0.4 and call its
  `track(name, props, url)`. Nothing of the delivery, validation or queueing is
  reimplemented here, and `config.rs` carries no `ANALYTICS_*` fields: the library reads
  that environment itself (`ANALYTICS_ENABLED`, `_BACKEND`, `_ENDPOINT`, `_SITE_ID`,
  `_HOSTNAME`, `_CONSENT`, `_RESPECT_GPC`, `_GEO`, `_PROP_KEYS`), so there is one source of
  truth.
- Run each request inside the library's `RequestContext { gpc, consent }`, from an axum
  middleware (`hero::controllers::analytics::analytics_context`, a `from_fn` layer in
  `build_router`) that wraps the rest of the request's future in `RequestContext::scope`.
  `Sec-GPC: 1` or `DNT: 1` sets `gpc`, and the library drops the request's events while
  `ANALYTICS_RESPECT_GPC` is true (the default). `Analytics-Consent: 1` sets `consent`,
  which matters only under `ANALYTICS_CONSENT=opt-in`: without it no event is recorded. A
  browser never sends that header by itself, so a front end or BFF that collected consent
  adds it to its calls. The context is a thread-local set around every poll of the future,
  so a task a handler spawns does not inherit it; none of the events here are tracked
  from a spawned task.
- Record two sample domain events from the Hero v2 JSON handlers, `hero.created` and
  `hero.deleted`, right after the repository write has succeeded and the CRUD event has
  been published. A request that fails validation, authorisation or the write returns
  before that point and records nothing. Updates, bulk deletes and the XML, v1 and web
  siblings are not tracked.
- Send only counts: `powers_count` (an integer) is the one prop, and it is sent only when
  it is on `ANALYTICS_PROP_KEYS` (default empty, so nothing is sent until an operator opts
  in). Names, ids, owners, claims and request headers are never passed to `track()`. The
  event `url` is the route's constant path (`/crud/v1/heroes/v2/json`), never the
  request's, so query strings and record ids cannot leak into it.
- Treat analytics as best effort: `track()` never fails, panics or waits for the network,
  the Umami queue drops on overflow, and `main` flushes it once on shutdown, after the
  server has stopped and off the async workers (`spawn_blocking`). A request is never
  slowed or failed by analytics.
- Keep the CNIL audience-measurement exemption as the privacy reference (see
  [NFR-0033](../nfrs/NFR-0033-analytics-privacy.md)), in its stricter reading.

## Consequences

- With `ANALYTICS_BACKEND` unset (`none`) the template behaves as before; the
  devcontainer's `analytics.env` selects `umami` and the seeded website.
- Because the library, not this app, validates events, a new event or prop needs three
  things: a conforming name (`^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$`, at most 50 characters),
  a scalar value of at most 200 characters, and its key added to `ANALYTICS_PROP_KEYS`.
- Backend events carry `payload.ip = server` (the library's constant), so Umami stores no
  client address and derives no geography. A deployment cannot get visitor-level analytics
  from this service; page-level analytics belong to the BFF, which sees the browser.
- `Analytics-Consent` is this template's own convention. A deployment that uses
  `opt-in` has to make its front end send it, or no event is ever recorded. The name has
  no `X-` prefix (RFC 6648). Backends trust this header only behind the BFF, which strips
  any inbound copy and sets it from its own consent state; a direct client can set it,
  which is accepted for opt-in mode in development and documented here.
- The events are two function calls in the handlers, not a wrapper around the CRUD event
  bus: the Rust handlers already announce each write on the bus right after it succeeds,
  and a resource that wants events adds the same call next to that one.
- The middleware lives in `hero::controllers` (not a new top-level module) because the
  layering check (`.github/scripts/check_layering.py`, template-owned) knows no other
  home for it; it has no Hero-specific code and can move once the checker has a category
  for it.
- Library-wide failure modes (an unknown `ANALYTICS_CONSENT` or `_RESPECT_GPC` value
  falls back to its default) are the library's and are tracked upstream.
- Umami OSS has no retention setting; the 25-month limit is a manual purge
  (`.devcontainer/stack/umami/purge.sql`), not scheduled here.
