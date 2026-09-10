# NFR-0018. Enforce a strict, one-directional module import order

## Status

Implemented

## Attribute

Maintainability.

## Description

Modules shall depend only on layers below them in the order `config` →
`oidc` → `models` → `views` → `repositories` → `crud` → `health` →
`controllers` → `main`; a lower layer shall never import from a higher
one.

## Source

See ADR 0009.

## Verification

CI: `.github/scripts/check_layering.py`, backing the `check-layering`
prek hook, checks every `src/` file's `crate::`-qualified references
against a fixed allow-list on every commit -- see ADR 0009's "2026-09
update". Each module's own doc comment and `src/README.md`'s layer
diagram remain the canonical reference for *why* the order is what it
is; the automated gate is what actually fails a commit that violates it,
closing the gap ADR 0009 originally documented against template-
fastapi's `import-linter` contract.
