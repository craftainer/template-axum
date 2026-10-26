# NFR-0004. Keep the generic CRUD layer free of resource-specific logic

## Status

Implemented

## Attribute

Maintainability.

## Description

`craftainer_crud::crud::CrudService` shall not grow resource-specific methods or
branches; bespoke query/business logic belongs in the owning controller
or that resource's own `Repository` impl. The same rule applies to the
broader generic-vs-resource-specific split this NFR's title now also
covers: `src/generic/` and the `craftainer-crud` crate (`src/README.md`'s "Generic vs. Hero-specific
split") shall never reference `src/hero/` (or any future sibling
resource package).

## Source

See ADR 0001.

## Verification

Manual/code review: `craftainer_crud::crud` (in the `craftainer-crud` crate, ADR 0020)
contains no reference to `Hero` or any concrete type, and the crate has no
dependency on this app, so it cannot name one; `grep -i hero src/generic/`
returns nothing.
`.github/scripts/check_layering.py` (`NFR-0018`) automates the broader
`generic`/`hero` independence check on every commit.
