# NFR-0019. Never edit an applied migration

## Status

Implemented

## Attribute

Maintainability / data safety.

## Description

Once a migration has been applied to any environment, it shall never be
hand-edited or deleted; a further schema change is a new migration file.

## Source

See ADR 0002, `docs/nfrs/0003`.

## Verification

Process convention, enforced by review: `src/migration/` currently holds
one migration (`m20260907_000001_create_heroes.rs`); any future schema
change adds a new `mYYYYMMDD_HHMMSS_*.rs` file rather than editing this
one.
