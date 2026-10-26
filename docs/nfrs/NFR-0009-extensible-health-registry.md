# NFR-0009. Make adding a health check a two-line change

## Status

Implemented

## Attribute

Maintainability.

## Description

Registering a new external dependency's health check shall require only
implementing `HealthCheck` and calling `HealthRegistry::register` --
no other wiring.

## Source

See ADR 0001.

## Verification

Manual/code review: `craftainer_observability::health::checks`'s real checks and
`MockHealthCheck` all follow the identical `impl HealthCheck for X { fn name...
async fn check... }` shape; `src/lib.rs`'s `build_health_registry` registers
each with one `registry.register(Box::new(...))` call.
