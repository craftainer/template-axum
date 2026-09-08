# 0009. Enforce module layering by directory convention plus module-doc contract, not a separate lint tool

## Status

Accepted

## Context

template-fastapi enforces its layer order (`config` → `oidc` → `models`
→ `views` → `repositories` → `crud` → `health` → `controllers` → `main`)
with `import-linter`'s `layers` contract, run in CI. Rust has no
directly equivalent off-the-shelf tool for "module A may never `use`
module B" as a standalone lint; the two realistic options were (a) split
every layer into its own crate in the Cargo workspace, so the layer
order becomes real crate-dependency-graph enforcement the compiler
already checks, or (b) keep one binary crate and enforce the order by
convention plus code review, backed by each module's own doc comment
stating what it may import.

Splitting into per-layer crates was rejected for this phase: the
workspace would grow from one crate to nine, each needing its own
`Cargo.toml`/re-export surface, for a project whose current total is a
few thousand lines -- the ceremony cost outweighs the payoff at this
size, and phase 1's `Cargo.toml` (`[workspace] members = ["."]`) already
committed to a single-crate shape.

## Decision

The layer order is `config` → `oidc` → `models` → `views` →
`repositories` → `crud` → `health` → `controllers` → `main`, matching
this phase's own plan. Enforcement is two-layered:

1. Every module's own top-of-file doc comment states its position in the
   order and what it may import (see `src/repositories/mod.rs`,
   `src/crud/mod.rs`, `src/controllers/mod.rs` for examples) -- a reader
   or reviewer checking one file's `use` statements against its own doc
   comment catches a violation without needing a separate tool.
2. `cargo build`/`clippy` already reject a *genuine* upward dependency
   that would form a cycle (e.g. `models` trying to `use crate::
   controllers::...`) wherever it would require a module declared later
   in `main.rs`'s `mod` list to be visible earlier -- Rust's own module
   resolution doesn't strictly enforce declaration order, so this is a
   partial, not complete, backstop; the doc-comment contract is the real
   enforcement mechanism here, checked by a human (or a future CI grep
   rule) rather than the compiler.

A cheap CI-checkable version of this (a `scripts/check-layering.sh` that
greps each module's `use crate::` lines against an allowed-imports table
keyed by the order above) is left as a documented follow-up rather than
built this phase -- see `NFR-0018`'s port below.

## Consequences

Reading `src/README.md`'s layer diagram plus each module's own doc
comment tells a contributor where new code belongs without needing a
passing/failing tool to confirm it, at the cost of the guarantee being
advisory rather than build-enforced. This is a real, documented gap
against `NFR-0018`'s intent (`docs/nfrs/0018-strict-module-layering.md`
below) -- the honest trade this phase makes given the workspace-split
alternative's cost, not a claim that convention alone is equivalent to
`import-linter`.
