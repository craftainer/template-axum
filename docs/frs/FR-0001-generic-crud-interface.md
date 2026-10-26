# FR-0001. Provide a generic CRUD interface that a new resource can reuse without new CRUD code

## Status

Implemented

## Description

The system shall provide a storage-agnostic `Repository` trait and a
`CrudService<R>` built from it such that adding a new database-backed
resource requires only a SeaORM entity, DTOs, and one `Repository` `impl`
per backend -- no new list/get/create/update/delete logic.

## Source

This template's own purpose: making the *next* resource cheap to add.
See ADR 0001.

## Acceptance criteria

- `craftainer_crud::crud`'s `CrudService<R>` (the `craftainer-crud` crate,
  ADR 0020) contains no reference to `Hero` or
  any other concrete resource type.
- Hero's controller (`src/hero/controllers/heroes.rs`) is the only Hero-aware
  code outside `models`/`views`/`repositories`.
- A second resource could be added by implementing `Repository` for its
  own model/DTOs and writing a thin router, per `craftainer_crud::repositories::Repository`'s
  module doc.
