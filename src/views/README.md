# views/

The View layer: request/response DTOs. May import from `models` (for
`From<Model> for ReadDto` conversions), never the reverse.

- `mod.rs` — `FieldError`, the shared field-level validation-failure
  shape every resource's `*Create`/`*Update::validate()` returns.
- `hero.rs` — `HeroCreate`/`HeroUpdate`/`HeroRead`/`HeroListQuery` and
  their validation (`FR-0003`).
- `hero_xml.rs` — `HeroReadXml`, a field-for-field mirror of `HeroRead`
  with `Option` fields omitted (rather than rendered) when absent, for
  `controllers::heroes_xml`'s XML responses (`docs/adrs/0014`).
  `HeroCreate`/`HeroUpdate` are reused as-is for XML input.
- `bulk.rs` — `BulkUpdateResult`/`BulkDeleteResult`, the resource-
  agnostic result shapes a bulk update/delete returns (`docs/adrs/0013`).
- `stats.rs` — `ResourceStats`/`Prediction` and their sub-shapes, the
  `GET /stats`/`GET /predict` response bodies (`docs/adrs/0015`).

Convention: three DTOs per resource — a `*Create` (required fields), a
`*Update` (all-optional, PATCH semantics per `FR-0004`), and a plain
read DTO with `id` and every stored field. `*Create`/`*Update` never
include server-managed fields (`id`, `created_at`, `updated_at`,
`owner_id`, `archived_at`) — see `docs/adrs/0007` for why `owner_id`
specifically is never client-settable.

`src/views/hero.rs`'s module doc records a deliberate limitation: a
plain `Option<T>` field can't distinguish "omitted" from "explicitly
null" the way Pydantic's `exclude_unset` can, so `HeroUpdate` cannot
clear a previously-set field via `PATCH` in this phase.
