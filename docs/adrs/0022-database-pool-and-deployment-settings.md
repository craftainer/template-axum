# 0022. Configure the database pool from the environment; PgBouncer is a deployment requirement

## Status

Proposed

## Context

`template-axum` opened its Postgres pool with `Database::connect(url)`:
sea-orm's and sqlx's own defaults, with no way to size the pool, bound a
wait for a connection, require TLS or skip the startup migration.
`template-fastapi` solved the same deployment problem with `DB_POOL_MODE`
(its `docs/adrs/0022`, `NFR-0026`), `DB_POOL_SIZE` and friends,
`POSTGRES_SSLMODE` and `RUN_MIGRATIONS_ON_STARTUP`, and the two templates
should be configured alike where the stacks allow it.

The performance part of fastapi's change does not carry over. It left
`NullPool`, which paid a TCP handshake, authentication and a backend fork on
every request. sqlx always pools, so the axum app already reuses
connections: this ADR is a configuration and deployment-readiness item, not
a performance claim.

Library facts (sea-orm 2.0.2 over sqlx 0.9), each checked in the crate
source and, for the third, by a test against a real Postgres:

- sea-orm's `ConnectOptions::connect_timeout` and `acquire_timeout` both set
  sqlx's single `acquire_timeout`, which covers waiting for a free slot and
  opening a new connection. There is one timeout, not two.
- sea-orm has no setter for the libpq `sslmode`. It is on sqlx's
  `PgConnectOptions`, reached through `ConnectOptions::map_sqlx_postgres_opts`.
- **There is no app-side switch that makes this stack safe behind a
  transaction-mode pooler.** In `sqlx-postgres` 0.9.0
  (`connection/executor.rs`), a statement gets the unnamed id only when the
  query is not `persistent`; otherwise it takes a per-connection
  incrementing name. `statement_cache_capacity(0)` only stops caching and
  closing: the statements are still named, and never closed. A test that set
  it found 3 named statements left on a one-connection pool
  (`pg_prepared_statements`) after 3 queries. sea-orm does not expose sqlx's
  `Query::persistent(false)` (no occurrence of `persistent` in its source).
  fastapi's `external` mode (a unique name per prepared statement, for
  asyncpg) therefore has no equivalent here.

## Decision

We will add `Settings.db` (`config::DbSettings`), read from the environment
only (`NFR-0017`), and `src/db.rs` (`connect_options`, `connect`), which
builds sea-orm's `ConnectOptions` from it. A value is passed on only when its
variable is set, so an unconfigured process keeps today's behaviour.

| Variable | Default | Effect |
| --- | --- | --- |
| `DB_POOL_SIZE` | unset (sqlx: 10) | Maximum pool connections, integer >= 1. |
| `DB_POOL_MIN_SIZE` | unset (sqlx: 0) | Connections kept open while idle, integer >= 1, not above `DB_POOL_SIZE`. Not in fastapi. |
| `DB_POOL_RECYCLE` | unset (sqlx: 30 min) | Seconds before a connection is replaced (`max_lifetime`), integer >= 1. |
| `DB_POOL_IDLE_TIMEOUT` | unset (sqlx: 10 min) | Seconds an unused connection may idle, integer >= 1. Not in fastapi. |
| `DB_CONNECT_TIMEOUT` | unset (sqlx: 30 s) | Seconds a caller waits for a connection, free slot or new connection alike, integer >= 1. fastapi's name; its value is a float of asyncpg's connect timeout only. |
| `POSTGRES_SSLMODE` | unset | libpq `sslmode`: `disable`, `allow`, `prefer`, `require`, `verify-ca`, `verify-full`. Overrides a `sslmode` in `DATABASE_URL`. Under `MODE=production` the effective mode (this variable, else `DATABASE_URL`'s `sslmode=`) must be `require`, `verify-ca` or `verify-full`. |
| `RUN_MIGRATIONS_ON_STARTUP` | `true` | `false` skips `Migrator::up` at startup. |

**Parity gaps with template-fastapi, on purpose:**

- No `DB_POOL_MODE`. Its `null` value has no meaning (sqlx always pools), and
  its `external` value cannot be built on sea-orm (above). A deployment
  behind PgBouncer is a deployment requirement instead, below.
- No `DB_MAX_OVERFLOW`: sqlx has no overflow, so `DB_POOL_SIZE` is the whole
  per-process budget.
- No `DB_COMMAND_TIMEOUT`: sqlx has no client-side statement timeout, and the
  alternative, a server-side `statement_timeout` startup parameter, is
  rejected by PgBouncer unless it is listed in `ignore_startup_parameters`.
- `DB_CONNECT_TIMEOUT` is one timeout for both acquiring and connecting, so
  a low value also fails requests that merely wait for a free pooled
  connection under load.

**Connection budget.** replicas x `DB_POOL_SIZE` must stay below Postgres's
`max_connections`, minus reserved slots and headroom for migrations and
admin sessions. When replica autoscaling pushes the total past that limit,
put a pooler in front.

**PgBouncer requirement.** sqlx prepares and names statements on every
connection. In transaction (or statement) pooling the app is only safe behind
PgBouncer >= 1.21 with `max_prepared_statements` set to a non-zero value
(PgBouncer's `config.md`; its NEWS for 1.21.0 added protocol-level named
prepared statements): it then tracks each client's named statements and
re-prepares them on whichever server connection a transaction lands on.
Leave the app's statement cache at its default; it closes evicted statements,
which `statement_cache_capacity(0)` does not. Session pooling needs nothing
special. The devcontainer stack has no PgBouncer, so this is verified only by
a live test on the first deployment behind one.

**Production guardrail (behaviour change).** `Settings::from_env` now refuses
`MODE=production` unless `POSTGRES_SSLMODE` is `require`, `verify-ca` or
`verify-full`, the same rule as fastapi's. The error is joined with the
existing S3, Redis and OIDC scheme checks. A production deployment that had
no `POSTGRES_SSLMODE` must now set one.

**Migrations.** `RUN_MIGRATIONS_ON_STARTUP=false` is for deployments where
several replicas could race or a canary runs. This template has no separate
migrate command, so migrations then have to be applied by a release that runs
with the default `true`, or by tooling the operator provides
(`sea-orm-cli migrate up`). Do not run migrations through a transaction-mode
pooler; point them at Postgres directly.

**Shutdown.** `main` keeps a handle to the pool (`connect_database`,
`build_state_with_db`) and closes it after `axum::serve` returns, so pooled
connections end cleanly. Closing any clone closes the shared sqlx pool.
Bounding that drain with a timeout belongs to the graceful-shutdown item.

`MODE=mock` has no database: it builds no pool, and the settings are parsed
and validated but unused. This repository has no root `compose.yml` (removed
by `0019`) and the devcontainer's compose files belong to `template-docker`,
so neither sets these variables. `tests/perf/run-real-stack.sh` forwards them
to the app container.

## Consequences

Easier: a deployment can size and bound its pool, require TLS and skip the
startup migration with environment variables alone, in the names of the
FastAPI template. The PgBouncer requirement is written down with the reason
and not hidden behind a switch that would not work.

Harder: `DB_POOL_SIZE` is now a number operators budget against
`max_connections`. The two templates differ (no `DB_POOL_MODE` here), so
operators moving between them must read this ADR. PgBouncer support rests on
PgBouncer's prepared-statement tracking, unverified in this repository's
stack. Production deployments now need `POSTGRES_SSLMODE`.

Measured effect: not yet. Expected near zero (see Context). The perf tier's
default-versus-configured comparison is recorded here when it has run; the
defaults stay unchanged unless it shows a saving above the run-to-run spread
and of at least 3% (`NFR-0031`).
