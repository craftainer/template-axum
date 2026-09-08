# NFR-0004. Keep the generic CRUD layer free of resource-specific logic

## Status

Implemented

## Attribute

Maintainability.

## Description

`crud::CrudService` shall not grow resource-specific methods or
branches; bespoke query/business logic belongs in the owning controller
or that resource's own `Repository` impl.

## Source

See ADR 0001.

## Verification

Manual/code review: `src/crud/mod.rs` contains no reference to `Hero` or
any concrete type; `grep -i hero src/crud/mod.rs` returns nothing.
