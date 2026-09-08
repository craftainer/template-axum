# repositories/

Storage-agnostic CRUD access, backing `crud/`. See
`docs/adrs/0001-mvc-layering-with-a-generic-crud-interface.md` for why
this exists as a trait with two implementations (SeaORM-backed,
in-memory) per resource, and for the Rust-specific deviation from
template-fastapi's fully dynamic `SQLAlchemyRepository`.

- `mod.rs` — the `Repository` trait (associated types `Model`/
  `Create`/`Update`; methods `list`/`count`/`get`/`create`/`update`/
  `update_many`/`delete`/`delete_many`), `ListOptions`, `RepoError`, and
  the `dyn_repository!` macro (generates a concrete, boxed-trait-object
  wrapper per resource — see its doc comment for why a generic blanket
  `impl` doesn't work here).
- `filtering.rs` — `FilterOp`/`FilterClause`/`FilterValue`/
  `SortClause`, the storage-agnostic filter/sort vocabulary each
  `Repository` impl interprets itself (`docs/adrs/0013`).
- `hero_sea_orm.rs` — `HeroSeaOrmRepository`, the real Postgres-backed
  implementation. Maps a `FilterClause`/`SortClause`'s field name onto
  a SeaORM `Column` via its own small `column_for` table.
- `hero_memory.rs` — `HeroMemoryRepository`, the `Mode::Mock`
  in-memory fake (`BTreeMap`-backed, auto-incrementing id). Interprets
  filter/sort clauses directly against `hero::Model` fields.

A new resource adds one `impl Repository for XSeaOrmRepository` and,
if it needs `Mode::Mock` support, one `impl Repository for
XMemoryRepository` plus a `crate::dyn_repository!(...)` invocation in
`controllers/mod.rs` — no changes to `mod.rs` itself or to `crud/`.

Every mutating method takes `owner_id`; `create` always stamps it from
the verified token (see `docs/adrs/0007`), never trusts client input.
`delete` is a soft delete (`archived_at`, `docs/adrs/0008`) when the
resource has that column.
