# migration/

SeaORM migrations, applied automatically at startup (`FR-0020`) — see
`docs/nfrs/0003-db-model-current-shape-only.md` and
`docs/nfrs/0019-migrations-immutable-once-applied.md`.

- `mod.rs` — `Migrator`, the `MigratorTrait` impl listing every
  migration in order.
- `m20260907_000001_create_heroes.rs` — creates the `heroes` table in
  its current shape (see `src/models/hero.rs`'s doc comment for the
  template-fastapi Alembic history this collapses into one revision).

A schema change adds a new `mYYYYMMDD_HHMMSS_*.rs` file and appends it
to `Migrator::migrations()` — an applied migration is never edited in
place.
