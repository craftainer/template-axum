# NFR-0015. Redact internal error detail outside dev mode

## Status

Implemented

## Attribute

Security.

## Description

An unhandled internal error's response `detail` shall be a fixed,
non-leaking string outside `Mode::Dev`; the real error text is only
included in the HTTP response under `Mode::Dev`, and is always logged
via `tracing::error!` regardless of mode.

## Source

See ADR 0004.

## Verification

Manual/code review: `problem_details::configure_detail_redaction` is
called once in `main()` with the resolved `Mode`; `AppError::Internal`'s
`IntoResponse` consults the resulting flag before choosing between the
real message and `"Internal Server Error"`.
