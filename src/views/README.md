# views/

The View layer: request/response DTOs. May import from `models` (for
`From<Model> for ReadDto` conversions), never the reverse.

- `mod.rs` — `FieldError`, the shared field-level validation-failure
  shape every resource's `*Create`/`*Update::validate()` returns.
- `hero.rs` — `HeroCreate`/`HeroUpdate`/`HeroRead`/`HeroListQuery` and
  their validation (`FR-0003`).

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
