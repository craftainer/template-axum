# 0023. Conditional requests on the Hero JSON and XML routes

## Status

Accepted

## Context

`template-fastapi` gives every Hero read an `ETag`, answers a matching
`If-None-Match` with a bodiless `304`, and makes a single-record `PATCH`/
`DELETE` honour `If-Match` (`412` on a mismatch), so a client can revalidate
cheaply and avoid lost updates (its `docs/adrs/0023`; the routes themselves
are `craftainer_tool_crud`'s `crud_router.py`). The SDKs cache the tag of what
they read and send it back as `If-Match`, so this server has to produce the
same tags and the same status codes.

`craftainer-crud` 0.2.0 ships the building blocks (its `docs/adrs/0008`, the
`conditional` module): `Versioned`, `record_etag` (strong), `list_etag`
(weak), `not_modified`, `check_if_match`, `revalidation_headers` and
`CrudError::PreconditionFailed`, which `problem_details::AppError` already
maps to a `412` problem response (`docs/adrs/0020`). The crate has no routers,
so the wiring is this app's.

## Decision

We will apply them to the four Hero JSON/XML routers, route for route as
`crud_router.py` does:

| Route | Behaviour |
| --- | --- |
| `GET ?id=` (v2 JSON, v2 XML, v1 JSON, v1 XML) | Strong `ETag` and `Cache-Control: private, no-cache`; a matching `If-None-Match` (weak comparison, `*`, lists) is a bodiless `304` carrying the same two headers. |
| `GET` list (same four) | The same headers with a weak `ETag` over the page's ids and `updated_at` values, in order. |
| `PATCH ?id=` | `If-Match` checked first (strong comparison, `*` matches a record that exists, a weak tag never matches): `412` `application/problem+json`, `Hero was modified since the supplied If-Match`. A success carries the record's new `ETag` (no `Cache-Control`). |
| `DELETE ?id=` | `If-Match` checked the same way; a success is `204` with no tag. |
| bulk `PATCH`/`DELETE` (no `id`) | `If-Match` ignored, no `ETag` (no one representation to compare). |
| `POST`, `/stats`, `/predict`, `/events`, the web routes | Unchanged. The web router serves no records. |

`hero::models::hero::Model` implements `Versioned` (`updated_at`, which both
repositories stamp on create, update and soft delete). The tag is computed from
the model, not the view, so one row has the same tag on the v2 and the v1 route
and in JSON and XML, as in `template-fastapi`. A shared helper in
`hero::controllers` (`read_etag`, `enforce_if_match`) keeps the four routers
alike.

Details that follow `template-fastapi` and `craftainer-crud` rather than
being new choices:

- No `If-Match` header passes. A `PATCH`/`DELETE` of a record that does not
  exist skips the check and falls through to the write's `404`.
- The `If-Match` read uses the same `include_archived` visibility as the
  write, so a record the write would `404` on also `404`s here.
- That read is not owner-scoped, though the write is (`docs/adrs/0007`). Reads
  are open to every caller with a read role, so a `412` tells a caller nothing
  that a `GET` of the same id does not; only a write by a non-owner stays a
  `404`.
- The order is rate limit, role, body validation (`422`), then `If-Match`
  (`412`), then the write, as in `crud_router.py`.
- The v1 routes still attach `Sunset`/`Deprecation`/`Link` to a `304`, as
  `template-fastapi` does through its route dependency. A `412` is an error
  response and carries none, in both.
- No `Vary` is set, as in `template-fastapi`. `craftainer-crud`'s ADR 0008
  leaves `Vary` to the host; nothing here varies a response by a request header
  other than the bearer token, which `private` already confines to the client's
  own cache. A route that starts to vary by `Accept` or similar adds it.

**Deliberate differences from template-fastapi:**

- The `ETag` input is the model's `updated_at` at microsecond precision
  (`craftainer-crud` hashes no finer digits). Postgres stores microseconds;
  the in-memory repository used under `MODE=mock` stores nanoseconds, so two
  writes inside one microsecond would share a tag there. This cannot happen
  through HTTP.
- `Features::conditional_requests` is not set: this app does not mount the
  `craftainer-crud` manifest route yet (`docs/adrs/0020`, "not wired"). Whoever
  mounts it sets the flag to `true`, which is then accurate (it reports `etag`,
  strong `record_etag` and weak `list_etag`, which is what these routes
  produce).
- Python falls back to hashing the serialized view for a view with no
  `updated_at`; `Versioned` requires one, and `Hero` has one.

## Consequences

Easier: a client revalidates a read without re-downloading it and can refuse to
overwrite a change it has not seen; the tags match across `template-axum` and
`template-fastapi` for the same `id` and `updated_at`.

Harder: clients must handle `412`. The `304` saves serialization and
bandwidth, not the query, which still runs. The check and the write are
separate statements, so two writers racing inside that gap can both pass (as in
`template-fastapi`); closing it would need a `WHERE updated_at = ?` in the
repository's update, which this does not add. Nothing was measured for it: like
`template-fastapi`'s, it is a correctness and bandwidth feature, not accepted
on a CPU win.
