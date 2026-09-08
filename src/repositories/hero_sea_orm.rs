//! Postgres-backed `Repository` for Hero, via SeaORM. Used whenever
//! `Mode` is not `Mock` -- see `src/repositories/hero_memory.rs` for the
//! `Mode::Mock` counterpart and `docs/adrs/0006` for the swap mechanism.

use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Set,
};

use crate::models::hero::{self, ActiveModel, Column, Entity};
use crate::repositories::{ListOptions, RepoError, Repository};
use crate::views::hero::{HeroCreate, HeroUpdate};

pub struct HeroSeaOrmRepository {
    db: DatabaseConnection,
}

impl HeroSeaOrmRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn backend_err(err: sea_orm::DbErr) -> RepoError {
    RepoError::Backend(err.to_string())
}

#[async_trait]
impl Repository for HeroSeaOrmRepository {
    type Model = hero::Model;
    type Create = HeroCreate;
    type Update = HeroUpdate;

    async fn list(&self, opts: ListOptions) -> Result<Vec<hero::Model>, RepoError> {
        let mut query = Entity::find();
        if !opts.include_archived {
            query = query.filter(Column::ArchivedAt.is_null());
        }
        query
            .order_by_asc(Column::Id)
            .offset(opts.skip)
            .limit(opts.limit)
            .all(&self.db)
            .await
            .map_err(backend_err)
    }

    async fn get(&self, id: i32, include_archived: bool) -> Result<Option<hero::Model>, RepoError> {
        let mut query = Entity::find_by_id(id);
        if !include_archived {
            query = query.filter(Column::ArchivedAt.is_null());
        }
        query.one(&self.db).await.map_err(backend_err)
    }

    async fn create(&self, owner_id: &str, data: HeroCreate) -> Result<hero::Model, RepoError> {
        let now = Utc::now().naive_utc();
        let active = ActiveModel {
            name: Set(Some(data.name)),
            powers: Set(Some(data.powers)),
            power_level: Set(data.power_level),
            owner_id: Set(owner_id.to_string()),
            archived_at: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        };
        active.insert(&self.db).await.map_err(backend_err)
    }

    async fn update(
        &self,
        id: i32,
        owner_id: &str,
        data: HeroUpdate,
    ) -> Result<Option<hero::Model>, RepoError> {
        let existing = Entity::find_by_id(id)
            .filter(Column::OwnerId.eq(owner_id))
            .filter(Column::ArchivedAt.is_null())
            .one(&self.db)
            .await
            .map_err(backend_err)?;
        let Some(existing) = existing else {
            return Ok(None);
        };

        let mut active: ActiveModel = existing.into();
        if let Some(name) = data.name {
            active.name = Set(Some(name));
        }
        if let Some(powers) = data.powers {
            active.powers = Set(Some(powers));
        }
        if data.power_level.is_some() {
            active.power_level = Set(data.power_level);
        }
        active.updated_at = Set(Utc::now().naive_utc());

        active.update(&self.db).await.map(Some).map_err(backend_err)
    }

    async fn delete(&self, id: i32, owner_id: &str) -> Result<bool, RepoError> {
        let existing = Entity::find_by_id(id)
            .filter(Column::OwnerId.eq(owner_id))
            .filter(Column::ArchivedAt.is_null())
            .one(&self.db)
            .await
            .map_err(backend_err)?;
        let Some(existing) = existing else {
            return Ok(false);
        };

        let mut active: ActiveModel = existing.into();
        active.archived_at = Set(Some(Utc::now().naive_utc()));
        active.updated_at = Set(Utc::now().naive_utc());
        active.update(&self.db).await.map_err(backend_err)?;
        Ok(true)
    }
}
