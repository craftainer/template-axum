//! SeaORM migrations -- current-shape-only translation of template-fastapi's
//! Alembic history (`docs/nfrs/0003-db-model-current-shape-only.md`): one
//! migration landing the `heroes` table's final shape, not a replay of
//! every intermediate Alembic revision. Applied automatically at startup,
//! off the request path (FR-0020) -- see `main.rs`'s `run_migrations`.

pub mod m20260907_000001_create_heroes;

use sea_orm_migration::prelude::*;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(m20260907_000001_create_heroes::Migration)]
    }
}
