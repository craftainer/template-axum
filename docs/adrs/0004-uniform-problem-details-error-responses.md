# 0004. Render every error as an RFC 9457 problem+json body

## Status

Accepted

## Context

Ported from template-fastapi's `docs/adrs/0004`. Without a single
convention, each handler would invent its own error JSON shape, and a
client would need per-endpoint parsing logic to distinguish a validation
failure from a not-found from an unhandled server error.

## Decision

`src/problem_details.rs`'s `AppError` is the one error type every
controller and extractor returns (`Result<T, AppError>`); its
`IntoResponse` impl is the single place that renders
`application/problem+json`, with fields `type`/`title`/`status`/`detail`
matching RFC 9457 (`instance` is omitted in this phase -- see this doc's
own Consequences). `AppError::UnprocessableEntity` carries a
`Vec<FieldError>` instead of a bare string, so a 422 body's `detail` is a
structured list (field + message) rather than free text, matching
`FR-0018`. `AppError::Internal`'s detail is redacted to a fixed
`"Internal Server Error"` string outside `Mode::Dev`
(`configure_detail_redaction`, called once at startup) -- the real error
always goes to `tracing::error!` first, matching `NFR-0015`.

## Consequences

Every handler in `src/controllers/` returns `Result<_, AppError>` and
uses `?` freely (via `From<RepoError> for AppError`) -- no handler builds
its own error JSON. A 401 response also gets `WWW-Authenticate: Bearer`
attached in the same one place, so that header can never be forgotten on
one route and present on another.

Deviation from the Python original, noted rather than silently dropped:
`instance` (the RFC 9457 field naming the request path that failed) is
omitted here. Threading the current request's path into `AppError`
would need either a request-scoped extension read back out during
`IntoResponse`, or every error site accepting a path parameter it mostly
doesn't have available -- deferred as unnecessary complexity for this
phase; `type`/`title`/`status`/`detail` alone already let a client
branch correctly, and every error is independently logged with
`tracing`'s own path field where relevant (`src/oidc/mod.rs`,
`src/health/checks.rs`).
