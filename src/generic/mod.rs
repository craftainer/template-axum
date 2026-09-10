//! The generic, resource-agnostic half of the app's MVC layers -- see
//! `src/README.md`'s "Generic vs. Hero-specific split". Mirrors
//! template-fastapi's `src/crud/` package (named `generic` here, not
//! `crud`, since this crate's own `crate::crud` module -- the
//! `CrudService` abstraction, `src/crud/README.md` -- already owns that
//! name and predates this split). Never imports from `crate::hero` (or
//! any future resource package); see each submodule's own `README.md`.

pub mod controllers;
pub mod models;
pub mod repositories;
pub mod views;
