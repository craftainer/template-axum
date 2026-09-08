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

Manual/code review, per ADR 0009's documented gap: each module's own doc
comment states its allowed imports; `src/README.md`'s layer diagram is
the canonical reference. A `scripts/check-layering.sh` CI gate (grepping
`use crate::` against the allowed order) is a documented follow-up, not
yet built -- this NFR is currently verified by convention/review, not by
an automated gate, unlike template-fastapi's `import-linter` contract.
