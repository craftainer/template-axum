# NFR-0031. The database pool is per-deployment configuration, within the connection budget

## Status

Proposed

## Attribute

Performance / scalability, and secure-by-default deployment.

## Description

The service shall:

- configure its Postgres pool only through environment variables read per
  `NFR-0017` (`DB_POOL_SIZE`, `DB_POOL_MIN_SIZE`, `DB_POOL_RECYCLE`,
  `DB_POOL_IDLE_TIMEOUT`, `DB_CONNECT_TIMEOUT`, `POSTGRES_SSLMODE`,
  `RUN_MIGRATIONS_ON_STARTUP`);
- behave exactly as before when none of them is set: sea-orm and sqlx
  defaults, migrations on startup;
- reject an invalid value at startup (unknown `sslmode`, a size or timeout
  below 1, a minimum above the maximum) rather than ignore it;
- keep replicas x `DB_POOL_SIZE` below Postgres's `max_connections` minus
  reserved slots and headroom for migrations and admin sessions;
- when run behind a transaction-mode pooler, require PgBouncer >= 1.21 with a
  non-zero `max_prepared_statements`, because sqlx names its prepared
  statements whatever the statement-cache size (`ADR-0022`);
- under `MODE=production`, refuse to start unless `POSTGRES_SSLMODE` is
  `require`, `verify-ca` or `verify-full`;
- close the pool when the server shuts down.

No performance target is set: unlike its FastAPI counterpart `NFR-0026`,
this service never ran without a pool, so a non-default setting is not
expected to save CPU per request. A setting becomes a shipped default only
if the perf tier shows a saving above the run-to-run spread and of at least
3%, the acceptance rule template-fastapi's `ADR-0019` set for its pool change.

## Source

Operations, preparing a deployment that autoscales; parity with
template-fastapi's `NFR-0026` where the stack allows. Detailed by
[`ADR-0022`](../adrs/0022-database-pool-and-deployment-settings.md).

## Verification

- Unit tests: `config.rs` (parsing, defaults, every rejection, the
  production `sslmode` rule) and `db.rs` (each setting reaches the
  `ConnectOptions` or `PgConnectOptions`).
- `tests/postgres_db_pool.rs`, against the devcontainer's Postgres: the sizes
  and timeouts are on the live pool, and closing one handle closes every
  clone.
- `tests/postgres_connect_database.rs`: `RUN_MIGRATIONS_ON_STARTUP=false`
  leaves a fresh database empty and the default migrates it.
- `tests/e2e.rs`: the binary starts and shuts down cleanly with the pool
  closing.
- The PgBouncer requirement is verified by live test on the first deployment
  behind a pooler; no PgBouncer is in the devcontainer stack.
