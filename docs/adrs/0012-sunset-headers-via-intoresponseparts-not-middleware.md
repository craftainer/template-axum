# 0012. Attach Sunset/Deprecation headers via `IntoResponseParts`, not middleware

## Status

Accepted

## Context

Tier B item 2 ports the `sunset()` half of `app/http_headers.py`: an
RFC 8594 `Sunset` header (HTTP-date), `Deprecation: true`, and an
optional `Link: <path>; rel="sunset"` header, attached to a deprecated
route's response only -- never to a current-version route's response
(`NFR-0002`). In FastAPI, `sunset()` returns a per-route dependency
(`Callable[[Response], None]`) a route opts into via
`Depends(sunset(...))`; the module's own docstring explains this is
deliberately *not* the app's global `add_security_headers` ASGI
middleware, because a dependency only runs for the specific routes that
declare it.

axum has two natural analogues for "attach these headers to this
response, and only this one": `axum::middleware::from_fn` (a
`tower`-style layer) or a type implementing
`axum::response::IntoResponseParts`, combined into a handler's return
type via a tuple (e.g. `(Sunset::new(...), Json(body))`). A middleware
layer, like `src/rate_limit.rs`'s reasoning in ADR 0011 for the same
per-route-not-global concern, would need attaching per-route (or per
sub-router), and — worse here — still runs *after* the handler decides
what to return, needing some side-channel (an `Extension` the handler
inserts) to tell the middleware whether the specific request actually
hit a deprecated path. `IntoResponseParts` needs no such side-channel:
the handler that already knows it's serving a deprecated route just
includes `Sunset::new(...)` directly in what it returns.

## Decision

`src/http_headers.rs::Sunset` implements `axum::response::
IntoResponseParts`. A handler opts in by returning a tuple with
`Sunset` as one element, e.g. `(Sunset::new(sunset_at, Some(link)),
Json(body))` -- axum combines it into the final response's headers
without wrapping or intercepting the body, so it composes with any
other response type (`Json`, a plain string, a `StatusCode` tuple) the
same way `Depends(sunset(...))` composes with any route's return type
in the reference.

No route uses `Sunset` yet: the Hero v1 compat routes that would
(`NFR-0002`'s "every route on a deprecated API version") are Tier C
item 7, out of scope for this plan per its own text (this app hasn't
established an API-versioning pattern -- `docs/adrs/0002`, `0009`).
`#![allow(dead_code)]` in `http_headers.rs` is a temporary marker for
that gap, not a permanent exemption.

## Consequences

Adding a deprecated route later is a one-line change to its handler's
return type, with no router restructuring and no risk of a global
middleware accidentally tagging a current-version response (the
concrete bug `NFR-0002`'s "current-version routes shall carry none of
these headers" acceptance test guards against in the reference). The
trade-off: because nothing calls `Sunset` outside its own tests yet,
its real integration into a route (interaction with `AuthClaims`
extraction order, whichever error paths a deprecated handler has, etc.)
is unverified until item 7 lands -- a documented, deliberate gap, not
an oversight.
