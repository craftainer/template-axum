//! In-memory `Repository` fake for Hero, used under `Mode::Mock` -- port of
//! `repositories/memory.py`'s `InMemoryRepository`. See
//! `docs/adrs/0006-mode-driven-fakes-for-infrastructure-free-testing.md`:
//! the app boots and is fully CRUD-functional with zero real
//! infrastructure.

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::Utc;

use crate::models::hero;
use crate::repositories::{ListOptions, RepoError, Repository};
use crate::views::hero::{HeroCreate, HeroUpdate};

pub struct HeroMemoryRepository {
    records: Mutex<BTreeMap<i32, hero::Model>>,
    next_id: Mutex<i32>,
}

impl HeroMemoryRepository {
    pub fn new() -> Self {
        Self {
            records: Mutex::new(BTreeMap::new()),
            next_id: Mutex::new(1),
        }
    }
}

impl Default for HeroMemoryRepository {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Repository for HeroMemoryRepository {
    type Model = hero::Model;
    type Create = HeroCreate;
    type Update = HeroUpdate;

    async fn list(&self, opts: ListOptions) -> Result<Vec<hero::Model>, RepoError> {
        let records = self
            .records
            .lock()
            .expect("hero memory repository lock poisoned");
        let mut items: Vec<hero::Model> = records
            .values()
            .filter(|hero| opts.include_archived || hero.archived_at.is_none())
            .cloned()
            .collect();
        items.sort_by_key(|hero| hero.id);
        let items = items
            .into_iter()
            .skip(opts.skip as usize)
            .take(opts.limit as usize)
            .collect();
        Ok(items)
    }

    async fn get(&self, id: i32, include_archived: bool) -> Result<Option<hero::Model>, RepoError> {
        let records = self
            .records
            .lock()
            .expect("hero memory repository lock poisoned");
        Ok(records
            .get(&id)
            .filter(|hero| include_archived || hero.archived_at.is_none())
            .cloned())
    }

    async fn create(&self, owner_id: &str, data: HeroCreate) -> Result<hero::Model, RepoError> {
        let now = Utc::now().naive_utc();
        let mut next_id = self
            .next_id
            .lock()
            .expect("hero memory repository lock poisoned");
        let id = *next_id;
        *next_id += 1;
        let model = hero::Model {
            id,
            name: Some(data.name),
            powers: Some(data.powers),
            power_level: data.power_level,
            owner_id: owner_id.to_string(),
            archived_at: None,
            created_at: now,
            updated_at: now,
        };
        self.records
            .lock()
            .expect("hero memory repository lock poisoned")
            .insert(id, model.clone());
        Ok(model)
    }

    async fn update(
        &self,
        id: i32,
        owner_id: &str,
        data: HeroUpdate,
    ) -> Result<Option<hero::Model>, RepoError> {
        let mut records = self
            .records
            .lock()
            .expect("hero memory repository lock poisoned");
        let Some(existing) = records.get_mut(&id) else {
            return Ok(None);
        };
        if existing.owner_id != owner_id || existing.archived_at.is_some() {
            return Ok(None);
        }
        if let Some(name) = data.name {
            existing.name = Some(name);
        }
        if let Some(powers) = data.powers {
            existing.powers = Some(powers);
        }
        if data.power_level.is_some() {
            existing.power_level = data.power_level;
        }
        existing.updated_at = Utc::now().naive_utc();
        Ok(Some(existing.clone()))
    }

    async fn delete(&self, id: i32, owner_id: &str) -> Result<bool, RepoError> {
        let mut records = self
            .records
            .lock()
            .expect("hero memory repository lock poisoned");
        let Some(existing) = records.get_mut(&id) else {
            return Ok(false);
        };
        if existing.owner_id != owner_id || existing.archived_at.is_some() {
            return Ok(false);
        }
        existing.archived_at = Some(Utc::now().naive_utc());
        existing.updated_at = Utc::now().naive_utc();
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_then_get_round_trips() {
        let repo = HeroMemoryRepository::new();
        let created = repo
            .create(
                "alice",
                HeroCreate {
                    name: "Spectra".into(),
                    powers: vec!["flight".into()],
                    power_level: Some(5),
                },
            )
            .await
            .unwrap();
        let fetched = repo.get(created.id, false).await.unwrap();
        assert_eq!(fetched.unwrap().name.as_deref(), Some("Spectra"));
    }

    #[tokio::test]
    async fn delete_is_soft_and_excluded_by_default() {
        let repo = HeroMemoryRepository::new();
        let created = repo
            .create(
                "alice",
                HeroCreate {
                    name: "Spectra".into(),
                    powers: vec!["flight".into()],
                    power_level: None,
                },
            )
            .await
            .unwrap();
        assert!(repo.delete(created.id, "alice").await.unwrap());
        assert!(repo.get(created.id, false).await.unwrap().is_none());
        assert!(repo.get(created.id, true).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn write_ops_are_owner_scoped() {
        let repo = HeroMemoryRepository::new();
        let created = repo
            .create(
                "alice",
                HeroCreate {
                    name: "Spectra".into(),
                    powers: vec!["flight".into()],
                    power_level: None,
                },
            )
            .await
            .unwrap();
        assert!(!repo.delete(created.id, "mallory").await.unwrap());
        assert!(repo
            .update(
                created.id,
                "mallory",
                HeroUpdate {
                    name: Some("Hacked".into()),
                    ..Default::default()
                }
            )
            .await
            .unwrap()
            .is_none());
    }
}
