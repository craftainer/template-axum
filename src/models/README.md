# models/

The Model layer: SeaORM entities. Lowest layer besides `config`/`oidc`
(see `src/README.md`'s "Layering") — never imports from
`views`/`repositories`/`crud`/`health`/`controllers`.

- `hero.rs` — the `heroes` table entity. Current-shape-only (see
  `docs/nfrs/0003-db-model-current-shape-only.md`): `id`, `name`
  (nullable), `powers` (nullable `text[]`), `power_level` (nullable),
  `owner_id` (indexed), `archived_at` (nullable — soft-delete marker,
  ADR 0008), `created_at`/`updated_at`.

A new resource adds one entity module here plus a `pub mod` line in
`mod.rs` — no other file in this module needs to change.
