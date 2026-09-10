//! Hero's storage backends: one `Repository` impl per backend (SeaORM,
//! in-memory). See `crate::generic::repositories` for the trait/
//! `dyn_repository!` machinery these implement.

pub mod hero_memory;
pub mod hero_sea_orm;
