# hero/views/

Hero's half of the View layer (see `src/README.md`'s "Generic vs.
Hero-specific split"): request/response DTOs for this template's worked
CRUD example. May import from `hero::models`/`generic::views` (for
`From<Model> for ReadDto` conversions and the shared `FieldError`),
never the reverse.

- `hero.rs` — `HeroCreate`/`HeroUpdate`/`HeroRead`/`HeroListQuery` and
  their validation (`FR-0003`).
- `hero_xml.rs` — `HeroReadXml`, a field-for-field mirror of `HeroRead`
  with `Option` fields omitted (rather than rendered) when absent, for
  `hero::controllers::heroes_xml`'s XML responses (`docs/adrs/0014`).
  `HeroCreate`/`HeroUpdate` are reused as-is for XML input.
- `hero_form.rs` — `HeroFormFields`, the shape backing the progressively-
  enhanced HTML form (`hero::controllers::heroes_web`, `FR-0034`).
- `hero_v1.rs` / `hero_v1_xml.rs` — the deprecated `superpower: String`-
  shaped v1 compat DTOs (`docs/adrs/0017`, `FR-0031`/`FR-0032`), lossily
  converted from/to the v2 DTOs above.

`hero.rs`'s module doc records a deliberate limitation: a plain
`Option<T>` field can't distinguish "omitted" from "explicitly null" the
way Pydantic's `exclude_unset` can, so `HeroUpdate` cannot clear a
previously-set field via `PATCH` in this phase.
