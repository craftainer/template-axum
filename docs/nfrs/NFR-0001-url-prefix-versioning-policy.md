# NFR-0001. Mount every resource under an explicit version prefix

## Status

Implemented

## Attribute

Maintainability / API stability.

## Description

Every resource shall be mounted under an explicit `/vN` router-version
and `/vM` model-version segment; no bare unversioned alias shall exist.

## Source

See ADR 0002.

## Verification

Manual: `main.rs` nests Hero's router at `/crud/v1/heroes/v2/json` only
-- no `.route("/heroes", ...)` or similar unversioned alias exists
anywhere in `src/controllers/`.
