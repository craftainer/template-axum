# NFR-0004. Keep the generic CRUD layer free of resource-specific logic

## Status

Implemented

## Attribute

Maintainability.

## Description

`crud::CrudService` shall not grow resource-specific methods or
branches; bespoke query/business logic belongs in the owning controller
or that resource's own `Repository` impl. The same rule applies to the
broader generic-vs-resource-specific split this NFR's title now also
covers: `src/generic/` (`src/README.md`'s "Generic vs. Hero-specific
split") shall never reference `src/hero/` (or any future sibling
resource package).

## Source

See ADR 0001.

## Verification

Manual/code review: `src/crud/mod.rs` contains no reference to `Hero` or
any concrete type; `grep -i hero src/crud/mod.rs` returns nothing.
`.github/scripts/check_layering.py` (`NFR-0018`) automates the broader
`generic`/`hero` independence check on every commit.
