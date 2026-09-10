//! The Hero SeaORM entity -- current-shape-only translation of
//! template-fastapi's `heroes` table after its `91807f3de5be` migration
//! (superpower -> powers list) and the later nullable-name/powers,
//! owner_id migrations. See `docs/nfrs/0003-db-model-current-shape-only.md`
//! for why this is one migration, not a replay of that history, and
//! `docs/adrs/0012-soft-delete-via-marker-column.md` for `archived_at`.
//!
//! Deliberately narrower than the Python original: only the
//! `Archivable` mixin is ported (soft delete). `Draftable`/`Schedulable`/
//! `Lockable`, revisions, and events are out of phase-2 scope -- see this
//! repo's own `docs/adrs/0001` for the scope note.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "heroes")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub name: Option<String>,
    pub powers: Option<Vec<String>>,
    pub power_level: Option<i32>,
    #[sea_orm(indexed)]
    pub owner_id: String,
    pub archived_at: Option<DateTime>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

impl crate::generic::models::HasId for Model {
    fn id(&self) -> i32 {
        self.id
    }
}
