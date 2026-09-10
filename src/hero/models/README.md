# hero/models/

Hero's half of the Model layer (see `src/README.md`'s "Generic vs.
Hero-specific split"): the concrete SeaORM entity for this template's
worked CRUD example. See `crate::generic::models` for the resource-
agnostic `HasId` contract this implements.

- `hero.rs` — the `heroes` table entity. Current-shape-only (see
  `docs/nfrs/NFR-0003-db-model-current-shape-only.md`): `id`, `name`
  (nullable), `powers` (nullable `text[]`), `power_level` (nullable),
  `owner_id` (indexed), `archived_at` (nullable — soft-delete marker,
  ADR 0008), `created_at`/`updated_at`.

A new resource adds its own sibling package (`crate::<resource>::models`,
mirroring this one) rather than a file here — this package is Hero's
alone.
