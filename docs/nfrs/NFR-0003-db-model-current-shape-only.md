# NFR-0003. Keep the persisted schema at its current shape only

## Status

Implemented

## Attribute

Maintainability.

## Description

The database schema shall represent only the current API version's
shape; a deprecated API version (when one exists) is a DTO/router-layer
conversion concern, never a second table or a legacy column kept in
sync.

## Source

See ADR 0002, ADR 0008.

## Verification

Manual: `src/migration/m20260907_000001_create_heroes.rs` creates one
`heroes` table in its final shape (no `superpower` legacy column, no
migration replay) -- confirmed via `psql \d heroes` against a real
Postgres instance in this phase's smoke test.
