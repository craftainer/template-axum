//! Creates the `heroes` table in its current shape (id, name, powers list,
//! power_level, owner_id, archived_at, created_at, updated_at) -- see
//! `src/models/hero.rs`'s module doc for the template-fastapi migration
//! history this collapses into one revision.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Heroes::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Heroes::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Heroes::Name).string().null())
                    .col(
                        ColumnDef::new(Heroes::Powers)
                            .array(ColumnType::Text)
                            .null(),
                    )
                    .col(ColumnDef::new(Heroes::PowerLevel).integer().null())
                    .col(ColumnDef::new(Heroes::OwnerId).string().not_null())
                    .col(ColumnDef::new(Heroes::ArchivedAt).timestamp().null())
                    .col(ColumnDef::new(Heroes::CreatedAt).timestamp().not_null())
                    .col(ColumnDef::new(Heroes::UpdatedAt).timestamp().not_null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("ix_heroes_owner_id")
                    .table(Heroes::Table)
                    .col(Heroes::OwnerId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Heroes::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Heroes {
    Table,
    Id,
    Name,
    Powers,
    PowerLevel,
    OwnerId,
    ArchivedAt,
    CreatedAt,
    UpdatedAt,
}
