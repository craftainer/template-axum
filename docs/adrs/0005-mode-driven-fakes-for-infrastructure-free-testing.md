# 0005. Drive every backend swap from one startup-time MODE setting

## Status

Accepted

## Context

Ported from template-fastapi's `docs/adrs/0006`. Running the full local
stack (Postgres, Redis, S3, Keycloak) just to exercise CRUD/health/RBAC
logic is unnecessary friction for quick local runs and for any future
test suite that doesn't need to verify the real backends themselves.

## Decision

`config::Mode` (env var `MODE`, one of `dev`/`mock`/`production`) is read
once at startup in `main.rs` and threaded through every layer that needs
to pick a backend:

- `repositories`: `Mode::Mock` selects `HeroMemoryRepository`; otherwise
  `HeroSeaOrmRepository` against a real `sea_orm::DatabaseConnection`.
- `health`: `Mode::Mock` registers `MockHealthCheck` (always
  `healthy: true, detail: "mocked"`, no network) for every dependency
  name; otherwise the real `DatabaseHealthCheck`/`RedisHealthCheck`/
  `S3HealthCheck`/`OidcHealthCheck`.
- `oidc`: `Mode::Mock` trusts a bearer token's claims without JWKS
  verification (`decode_without_verification`); `POST /mock/token`
  (`controllers::mock`) is mounted only under this mode, minting a
  Keycloak-shaped token so RBAC is still exercisable.
- Migrations: skipped entirely under `Mode::Mock` (no database exists to
  migrate).

`Mode::Mock` additionally requires `ALLOW_MOCK_MODE=1`
(`Settings::allow_mock_mode`) -- `Settings::from_env` refuses to
construct otherwise, so this mode (which bypasses auth entirely) can
never be reached by `MODE`'s own default/typo alone.

## Consequences

The app is fully CRUD/health/RBAC-functional under `MODE=mock` with zero
containers running -- verified in this phase's own smoke test (mock
token mint -> role-gated Hero create/list, all in-process). This is the
mode this port's automated tests implicitly rely on being possible, even
though phase 2 doesn't yet build a dedicated integration-test harness
around it (see this repo's own `docs/plans/` note, once phase 3 adds
one).

The cost, same as the Python original: every mode-branching decision
lives in `main.rs`'s startup wiring rather than each module owning its
own mode logic -- reading "what does `MODE=mock` actually change" means
reading `main.rs::build_health_registry` and the two hero-repository
branches, not one file.
