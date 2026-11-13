# FR-0034. Serve Hero CRUD as a progressively-enhanced HTML form

## Status

Implemented

## Description

The system shall expose `GET /heroes/form` as a server-rendered HTML
page that lists Hero records and supports create/update/delete through
plain `<form>` submissions -- working with no client-side JavaScript.
The system shall also expose `GET /heroes/components.js`, vanilla
JavaScript that, once loaded, intercepts those same forms and submits
them to the same form routes via `fetch` instead of a full page
navigation, without removing the no-JS path's own working handlers.
Authentication is a session cookie (ADR 0031), never a token in a URL.

## Source

Port of the reference implementation's `FR-0009`. Targets the existing
v2 JSON endpoints directly (no dependency on the Hero v1 items in this
plan). This app has no browser session/cookie login flow (see
`controllers::heroes_web`'s own module doc and ADR 0031 for how this router
authenticates instead: a bearer header, or the `HttpOnly` session cookie set
by `POST /heroes/form/session`; the earlier `?token=` query parameter was
removed, RFC 6750 section 2.3).

## Acceptance criteria

- `GET /heroes/form` renders a list of Hero records plus a create form,
  and (per record) update and delete forms, with no JavaScript
  required for any of the three operations to work.
- `powers` submits/renders as one comma-separated text input; splitting
  drops empty/whitespace-only entries.
- A validation failure on create/update returns `422` via the existing
  RFC 9457 Problem Details path (`FR-0018`) -- not a re-rendered form.
- A successful create/update/delete responds `303 See Other` back to
  `GET /heroes/form`.
- `GET /heroes/components.js` is served as `text/javascript`; when
  loaded, it intercepts the same forms via `fetch` against the same form
  routes and cookie, showing field errors inline on a `422` rather than
  navigating.
- The token is never in a URL or rendered into a page: a `?token=` query
  parameter is ignored, `POST /heroes/form/session` takes the token in the
  request body and answers `303` with an `HttpOnly`, `SameSite=Strict`,
  `Path=/heroes` cookie (`Secure` when `PUBLIC_BASE_URL` is https), and
  `POST /heroes/form/logout` clears it. A request authenticated by the cookie
  that the browser marks cross-site (`Sec-Fetch-Site`) is refused with `403`,
  and the pages are served `Cache-Control: no-store`.
- Every RBAC/ownership/rate-limit rule the JSON router enforces
  (`FR-0015`, `docs/adrs/0007`, `docs/adrs/0011`) applies identically
  here -- this router calls the same `craftainer_crud::crud::CrudService`/`crud_actions` path.
- Verified by `controllers::heroes_web::tests` and `views::hero_form::
  tests`.
