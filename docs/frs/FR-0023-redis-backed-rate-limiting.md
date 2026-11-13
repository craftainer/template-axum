# FR-0023. Rate-limit Hero's mutating routes and mock-token minting

## Status

Implemented

## Description

The system shall reject, with `429 Too Many Requests`, a caller who
exceeds a per-caller-IP request budget on `POST`/`PATCH`/`DELETE
/crud/v1/heroes/v2/json` (create/update/delete) or `POST /mock/token`
within a rolling 60-second window, sharing the counter across every
process via Redis rather than counting per-process.

## Source

Port of `app/rate_limit.py`. See ADR 0011.

## Acceptance criteria

- Each of create/update/delete and mint-token has its own configurable
  per-minute limit (`Settings::rate_limit_hero_write_per_minute`,
  `Settings::rate_limit_mock_token_per_minute`).
- Exceeding the limit returns `429` with an
  `application/problem+json` body (`AppError::TooManyRequests`).
- The counter is keyed by scope + caller, so different callers (or
  different scopes for the same caller) have independent budgets --
  verified by
  `rate_limit::tests::mock_backend_scopes_counters_independently_per_scope_and_ip`.
  The caller is the token's `sub` for the Hero write routes and the peer
  address for the unauthenticated mint-token route (ADR 0031).
- The Hero write limit is charged after the role check: a caller refused
  with `403` spends no budget -- verified by
  `controllers::heroes::tests::the_write_limit_is_charged_after_the_role_check_and_keyed_on_the_subject`.
- `GET` (list/get) is never rate-limited.
- Under `Mode::Mock`, the same behavior holds with no Redis container
  running (`NFR-0022`) -- verified by
  `controllers::heroes::tests::hero_write_routes_return_429_once_the_per_caller_limit_is_exceeded`
  and `controllers::mock::tests::mint_token_returns_429_once_the_per_caller_limit_is_exceeded`.
