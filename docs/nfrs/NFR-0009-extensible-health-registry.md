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

Manual/code review: `src/health/checks.rs`'s four real checks and one
mock check all follow the identical `impl HealthCheck for X { fn name...
async fn check... }` shape; `main.rs::build_health_registry` registers
each with one `registry.register(Box::new(...))` call.
