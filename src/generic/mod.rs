//! The app-owned, resource-agnostic routers (`audit`, `mock`) and their
//! accessor traits. The generic CRUD layer itself (models, views,
//! repositories, `CrudService`, query parsing, bulk/stats/events helpers)
//! lives in the `craftainer-crud` crate since docs/adrs/0020; the health
//! router and checks live in `craftainer-observability`. Never imports from
//! `crate::hero`; see `controllers/README.md`.

pub mod controllers;
