# hero/repositories/

Hero's half of storage-agnostic CRUD access (see `src/README.md`'s
"Generic vs. Hero-specific split"): the two concrete `Repository`
implementations backing Hero's `CrudService`. See
`crate::generic::repositories` for the trait/`dyn_repository!` machinery
these implement.

- `hero_sea_orm.rs` — `HeroSeaOrmRepository`, the real Postgres-backed
  implementation. Maps a `FilterClause`/`SortClause`'s field name onto
  a SeaORM `Column` via its own small `column_for` table.
- `hero_memory.rs` — `HeroMemoryRepository`, the `Mode::Mock`
  in-memory fake (`BTreeMap`-backed, auto-incrementing id). Interprets
  filter/sort clauses directly against `hero::Model` fields.

Every mutating method takes `owner_id`; `create` always stamps it from
the verified token (see `docs/adrs/0007`), never trusts client input.
`delete` is a soft delete (`archived_at`, `docs/adrs/0008`).

A new resource adds its own sibling package (`crate::<resource>::
repositories`, mirroring this one) plus a `crate::dyn_repository!(...)`
invocation in its own controllers' `mod.rs` — no changes to
`generic::repositories` or `crud/`.
