# FR-0020. Apply pending migrations automatically before serving traffic

## Status

Implemented

## Description

The system shall apply any pending SeaORM migration before accepting
connections, off any per-request path, except under `MODE=mock` (no
database exists).

## Source

Zero-manual-step startup; see ADR 0005.

## Acceptance criteria

- `main()` calls `migration::Migrator::up` once, before
  `axum::serve` starts, only when `Mode != Mock`.
- Verified against a real ephemeral Postgres instance: the `heroes`
  table and `ix_heroes_owner_id` index exist after startup with no
  manual migration step.
