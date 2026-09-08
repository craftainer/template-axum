# FR-0018. Render every error as application/problem+json

## Status

Implemented

## Description

The system shall render every 4xx/5xx response (not-found, forbidden,
unauthorized, validation failure, internal error) as a single
`application/problem+json` shape with `type`/`title`/`status`/`detail`.

## Source

RFC 9457; see ADR 0004.

## Acceptance criteria

- Every controller returns `Result<_, AppError>`; `AppError::
  into_response` is the sole place building the response body.
- A validation failure's `detail` is a list of `{field, msg}` objects;
  every other error's `detail` is a string.
