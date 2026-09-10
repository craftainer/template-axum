//! In-memory `Repository` fake for Hero, used under `Mode::Mock` -- port of
//! `repositories/memory.py`'s `InMemoryRepository`. See
//! `docs/adrs/0006-mode-driven-fakes-for-infrastructure-free-testing.md`:
//! the app boots and is fully CRUD-functional with zero real
//! infrastructure.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::Utc;

use crate::models::hero;
use crate::repositories::filtering::{FilterClause, FilterOp, FilterValue, SortClause};
use crate::repositories::{ListOptions, RepoError, Repository};
use crate::views::hero::{HeroCreate, HeroUpdate};

/// Whether `hero` satisfies every clause in `filters` (AND), interpreting
/// each clause directly against the matching `hero::Model` field -- the
/// in-memory counterpart to `hero_sea_orm.rs`'s SeaORM `Condition` mapping.
fn matches_all(hero: &hero::Model, filters: &[FilterClause]) -> bool {
    filters.iter().all(|clause| matches_one(hero, clause))
}

fn matches_one(hero: &hero::Model, clause: &FilterClause) -> bool {
    match clause.field.as_str() {
        "id" => matches_int(hero.id as i64, clause),
        "power_level" => hero
            .power_level
            .is_some_and(|v| matches_int(v as i64, clause)),
        "name" => hero.name.as_deref().is_some_and(|v| matches_str(v, clause)),
        "owner_id" => matches_str(&hero.owner_id, clause),
        "created_at" => matches_datetime(hero.created_at, clause),
        "updated_at" => matches_datetime(hero.updated_at, clause),
        "archived_at" => hero
            .archived_at
            .is_some_and(|v| matches_datetime(v, clause)),
        // An unrecognized field never matches -- `crud_query.rs`'s parser
        // already rejects one before it reaches here, so this is a
        // defense-in-depth default, not a reachable path through the HTTP
        // stack.
        _ => false,
    }
}

fn matches_int(actual: i64, clause: &FilterClause) -> bool {
    if clause.op == FilterOp::In {
        return matches!(&clause.value, FilterValue::List(list) if list.iter().any(|v| matches!(v, FilterValue::Int(target) if *target == actual)));
    }
    let FilterValue::Int(target) = &clause.value else {
        return false;
    };
    compare(actual, clause.op, *target)
}

fn matches_datetime(actual: chrono::NaiveDateTime, clause: &FilterClause) -> bool {
    if clause.op == FilterOp::In {
        return matches!(&clause.value, FilterValue::List(list) if list.iter().any(|v| matches!(v, FilterValue::DateTime(target) if *target == actual)));
    }
    let FilterValue::DateTime(target) = &clause.value else {
        return false;
    };
    compare(actual, clause.op, *target)
}

fn matches_str(actual: &str, clause: &FilterClause) -> bool {
    match clause.op {
        FilterOp::In => {
            matches!(&clause.value, FilterValue::List(list) if list.iter().any(|v| matches!(v, FilterValue::Str(target) if target == actual)))
        }
        FilterOp::Contains => {
            matches!(&clause.value, FilterValue::Str(target) if actual.contains(target.as_str()))
        }
        FilterOp::Icontains => {
            matches!(&clause.value, FilterValue::Str(target) if actual.to_lowercase().contains(&target.to_lowercase()))
        }
        FilterOp::Eq | FilterOp::Ne => {
            let FilterValue::Str(target) = &clause.value else {
                return false;
            };
            if clause.op == FilterOp::Eq {
                actual == target
            } else {
                actual != target
            }
        }
        FilterOp::Lt | FilterOp::Lte | FilterOp::Gt | FilterOp::Gte => false,
    }
}

fn compare<T: PartialOrd>(actual: T, op: FilterOp, target: T) -> bool {
    match op {
        FilterOp::Eq => actual == target,
        FilterOp::Ne => actual != target,
        FilterOp::Lt => actual < target,
        FilterOp::Lte => actual <= target,
        FilterOp::Gt => actual > target,
        FilterOp::Gte => actual >= target,
        FilterOp::In | FilterOp::Contains | FilterOp::Icontains => false,
    }
}

fn field_cmp(a: &hero::Model, b: &hero::Model, field: &str) -> Ordering {
    match field {
        "id" => a.id.cmp(&b.id),
        "power_level" => a.power_level.cmp(&b.power_level),
        "name" => a.name.cmp(&b.name),
        "owner_id" => a.owner_id.cmp(&b.owner_id),
        "created_at" => a.created_at.cmp(&b.created_at),
        "updated_at" => a.updated_at.cmp(&b.updated_at),
        "archived_at" => a.archived_at.cmp(&b.archived_at),
        _ => Ordering::Equal,
    }
}

/// Multi-key stable sort: apply each clause in reverse priority order, so
/// the final pass (the first/primary clause) settles ties the earlier
/// passes left in place -- the standard trick for a multi-key sort built
/// out of single-key stable sorts.
fn apply_sort(items: &mut [hero::Model], sort: &[SortClause]) {
    for clause in sort.iter().rev() {
        items.sort_by(|a, b| {
            let ord = field_cmp(a, b, &clause.field);
            if clause.descending {
                ord.reverse()
            } else {
                ord
            }
        });
    }
}

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
            .filter(|hero| matches_all(hero, &opts.filters))
            .cloned()
            .collect();
        if opts.sort.is_empty() {
            items.sort_by_key(|hero| hero.id);
        } else {
            apply_sort(&mut items, &opts.sort);
        }
        let items = items
            .into_iter()
            .skip(opts.skip as usize)
            .take(opts.limit as usize)
            .collect();
        Ok(items)
    }

    async fn count(
        &self,
        filters: &[FilterClause],
        include_archived: bool,
    ) -> Result<u64, RepoError> {
        let records = self
            .records
            .lock()
            .expect("hero memory repository lock poisoned");
        Ok(records
            .values()
            .filter(|hero| include_archived || hero.archived_at.is_none())
            .filter(|hero| matches_all(hero, filters))
            .count() as u64)
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
        apply_update(existing, data);
        Ok(Some(existing.clone()))
    }

    async fn update_many(
        &self,
        filters: &[FilterClause],
        data: HeroUpdate,
    ) -> Result<Vec<hero::Model>, RepoError> {
        let mut records = self
            .records
            .lock()
            .expect("hero memory repository lock poisoned");
        let mut updated = Vec::new();
        for hero in records.values_mut() {
            if hero.archived_at.is_some() || !matches_all(hero, filters) {
                continue;
            }
            // `HeroUpdate` isn't `Clone` (it's a plain request DTO) -- bulk
            // update applies the same edit to every matched record, so
            // each iteration rebuilds an equivalent `HeroUpdate` rather
            // than requiring `Clone` just for this one caller.
            apply_update(
                hero,
                HeroUpdate {
                    name: data.name.clone(),
                    powers: data.powers.clone(),
                    power_level: data.power_level,
                },
            );
            updated.push(hero.clone());
        }
        Ok(updated)
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

    async fn delete_many(&self, filters: &[FilterClause]) -> Result<Vec<hero::Model>, RepoError> {
        let mut records = self
            .records
            .lock()
            .expect("hero memory repository lock poisoned");
        let now = Utc::now().naive_utc();
        let mut deleted = Vec::new();
        for hero in records.values_mut() {
            if hero.archived_at.is_some() || !matches_all(hero, filters) {
                continue;
            }
            hero.archived_at = Some(now);
            hero.updated_at = now;
            deleted.push(hero.clone());
        }
        Ok(deleted)
    }
}

fn apply_update(existing: &mut hero::Model, data: HeroUpdate) {
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

    fn list_opts(filters: Vec<FilterClause>, sort: Vec<SortClause>) -> ListOptions {
        ListOptions {
            skip: 0,
            limit: 100,
            include_archived: false,
            filters,
            sort,
        }
    }

    async fn seeded_repo() -> (HeroMemoryRepository, hero::Model, hero::Model) {
        let repo = HeroMemoryRepository::new();
        let alpha = repo
            .create(
                "alice",
                HeroCreate {
                    name: "Alpha".into(),
                    powers: vec!["flight".into()],
                    power_level: Some(3),
                },
            )
            .await
            .unwrap();
        let beta = repo
            .create(
                "bob",
                HeroCreate {
                    name: "Beta".into(),
                    powers: vec!["strength".into()],
                    power_level: Some(7),
                },
            )
            .await
            .unwrap();
        (repo, alpha, beta)
    }

    #[test]
    fn default_builds_an_empty_repository() {
        let repo = HeroMemoryRepository::default();
        assert!(repo.records.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn matches_one_filters_by_created_at_and_updated_at_and_archived_at() {
        let (repo, alpha, _beta) = seeded_repo().await;

        let by_created = repo
            .list(list_opts(
                vec![FilterClause {
                    field: "created_at".to_string(),
                    op: FilterOp::Eq,
                    value: FilterValue::DateTime(alpha.created_at),
                }],
                vec![],
            ))
            .await
            .unwrap();
        assert!(by_created.iter().any(|h| h.id == alpha.id));

        let updated = repo
            .update(
                alpha.id,
                "alice",
                HeroUpdate {
                    name: Some("Alpha Prime".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .unwrap();
        let by_updated = repo
            .list(list_opts(
                vec![FilterClause {
                    field: "updated_at".to_string(),
                    op: FilterOp::Eq,
                    value: FilterValue::DateTime(updated.updated_at),
                }],
                vec![],
            ))
            .await
            .unwrap();
        assert!(by_updated.iter().any(|h| h.id == alpha.id));

        repo.delete(alpha.id, "alice").await.unwrap();
        let archived = repo.get(alpha.id, true).await.unwrap().unwrap();
        let by_archived = repo
            .count(
                &[FilterClause {
                    field: "archived_at".to_string(),
                    op: FilterOp::Eq,
                    value: FilterValue::DateTime(archived.archived_at.unwrap()),
                }],
                true,
            )
            .await
            .unwrap();
        assert_eq!(by_archived, 1);
    }

    #[tokio::test]
    async fn matches_one_rejects_an_unrecognized_field() {
        let (repo, _alpha, _beta) = seeded_repo().await;
        let matched = repo
            .list(list_opts(
                vec![FilterClause {
                    field: "nonexistent".to_string(),
                    op: FilterOp::Eq,
                    value: FilterValue::Str("anything".to_string()),
                }],
                vec![],
            ))
            .await
            .unwrap();
        assert!(matched.is_empty());
    }

    #[tokio::test]
    async fn matches_int_supports_in_and_ignores_a_non_int_value() {
        let (repo, alpha, beta) = seeded_repo().await;
        let by_in = repo
            .list(list_opts(
                vec![FilterClause {
                    field: "id".to_string(),
                    op: FilterOp::In,
                    value: FilterValue::List(vec![FilterValue::Int(alpha.id as i64)]),
                }],
                vec![],
            ))
            .await
            .unwrap();
        assert_eq!(by_in.len(), 1);
        assert_eq!(by_in[0].id, alpha.id);

        let by_wrong_type = repo
            .list(list_opts(
                vec![FilterClause {
                    field: "id".to_string(),
                    op: FilterOp::Eq,
                    value: FilterValue::Str("nope".to_string()),
                }],
                vec![],
            ))
            .await
            .unwrap();
        assert!(by_wrong_type.is_empty());
        let _ = beta;
    }

    #[tokio::test]
    async fn matches_datetime_supports_in_and_ignores_a_non_datetime_value() {
        let (repo, alpha, _beta) = seeded_repo().await;
        let by_in = repo
            .list(list_opts(
                vec![FilterClause {
                    field: "created_at".to_string(),
                    op: FilterOp::In,
                    value: FilterValue::List(vec![FilterValue::DateTime(alpha.created_at)]),
                }],
                vec![],
            ))
            .await
            .unwrap();
        assert!(by_in.iter().any(|h| h.id == alpha.id));

        let by_wrong_type = repo
            .list(list_opts(
                vec![FilterClause {
                    field: "created_at".to_string(),
                    op: FilterOp::Eq,
                    value: FilterValue::Str("nope".to_string()),
                }],
                vec![],
            ))
            .await
            .unwrap();
        assert!(by_wrong_type.is_empty());
    }

    #[tokio::test]
    async fn matches_str_supports_in_contains_icontains_and_ne() {
        let (repo, alpha, beta) = seeded_repo().await;

        let by_in = repo
            .list(list_opts(
                vec![FilterClause {
                    field: "name".to_string(),
                    op: FilterOp::In,
                    value: FilterValue::List(vec![FilterValue::Str("Alpha".to_string())]),
                }],
                vec![],
            ))
            .await
            .unwrap();
        assert_eq!(by_in.len(), 1);

        let by_contains = repo
            .list(list_opts(
                vec![FilterClause {
                    field: "name".to_string(),
                    op: FilterOp::Contains,
                    value: FilterValue::Str("lph".to_string()),
                }],
                vec![],
            ))
            .await
            .unwrap();
        assert_eq!(by_contains.len(), 1);

        let by_icontains = repo
            .list(list_opts(
                vec![FilterClause {
                    field: "name".to_string(),
                    op: FilterOp::Icontains,
                    value: FilterValue::Str("ALPH".to_string()),
                }],
                vec![],
            ))
            .await
            .unwrap();
        assert_eq!(by_icontains.len(), 1);

        let by_ne = repo
            .list(list_opts(
                vec![FilterClause {
                    field: "name".to_string(),
                    op: FilterOp::Ne,
                    value: FilterValue::Str("Alpha".to_string()),
                }],
                vec![],
            ))
            .await
            .unwrap();
        assert!(by_ne.iter().any(|h| h.id == beta.id));
        assert!(!by_ne.iter().any(|h| h.id == alpha.id));
    }

    #[tokio::test]
    async fn matches_str_never_matches_ordering_operators() {
        let (repo, _alpha, _beta) = seeded_repo().await;
        let matched = repo
            .list(list_opts(
                vec![FilterClause {
                    field: "name".to_string(),
                    op: FilterOp::Lt,
                    value: FilterValue::Str("Zzz".to_string()),
                }],
                vec![],
            ))
            .await
            .unwrap();
        assert!(matched.is_empty());
    }

    #[tokio::test]
    async fn matches_str_ignores_a_wrong_typed_value_for_eq_or_ne() {
        let (repo, _alpha, _beta) = seeded_repo().await;
        let matched = repo
            .list(list_opts(
                vec![FilterClause {
                    field: "name".to_string(),
                    op: FilterOp::Eq,
                    value: FilterValue::Int(3),
                }],
                vec![],
            ))
            .await
            .unwrap();
        assert!(matched.is_empty());
    }

    #[tokio::test]
    async fn compare_never_matches_contains_or_icontains_on_a_numeric_field() {
        let (repo, _alpha, _beta) = seeded_repo().await;
        let matched = repo
            .list(list_opts(
                vec![FilterClause {
                    field: "power_level".to_string(),
                    op: FilterOp::Contains,
                    value: FilterValue::Int(3),
                }],
                vec![],
            ))
            .await
            .unwrap();
        assert!(matched.is_empty());
    }

    #[tokio::test]
    async fn compare_supports_every_numeric_operator() {
        let (repo, alpha, beta) = seeded_repo().await;
        // Every clause compares against power_level=3 (alpha's own value);
        // beta's is 7.
        for (op, expect_alpha, expect_beta) in [
            (FilterOp::Ne, false, true),
            (FilterOp::Lt, false, false),
            (FilterOp::Lte, true, false),
            (FilterOp::Gt, false, true),
            (FilterOp::Gte, true, true),
        ] {
            let matched = repo
                .list(list_opts(
                    vec![FilterClause {
                        field: "power_level".to_string(),
                        op,
                        value: FilterValue::Int(3),
                    }],
                    vec![],
                ))
                .await
                .unwrap();
            assert_eq!(
                matched.iter().any(|h| h.id == alpha.id),
                expect_alpha,
                "op {op:?} against alpha (power_level=3)"
            );
            assert_eq!(
                matched.iter().any(|h| h.id == beta.id),
                expect_beta,
                "op {op:?} against beta (power_level=7)"
            );
        }
    }

    #[tokio::test]
    async fn field_cmp_and_apply_sort_order_by_every_field_ascending_and_descending() {
        let (repo, alpha, beta) = seeded_repo().await;
        for field in [
            "power_level",
            "name",
            "owner_id",
            "created_at",
            "updated_at",
        ] {
            let ascending = repo
                .list(list_opts(
                    vec![],
                    vec![SortClause {
                        field: field.to_string(),
                        descending: false,
                    }],
                ))
                .await
                .unwrap();
            assert_eq!(ascending[0].id, alpha.id, "ascending sort by {field}");

            let descending = repo
                .list(list_opts(
                    vec![],
                    vec![SortClause {
                        field: field.to_string(),
                        descending: true,
                    }],
                ))
                .await
                .unwrap();
            assert_eq!(descending[0].id, beta.id, "descending sort by {field}");
        }
    }

    #[tokio::test]
    async fn field_cmp_and_apply_sort_order_by_archived_at() {
        let (repo, alpha, beta) = seeded_repo().await;
        repo.delete(alpha.id, "alice").await.unwrap();
        repo.delete(beta.id, "bob").await.unwrap();

        let ascending = repo
            .list(ListOptions {
                include_archived: true,
                ..list_opts(
                    vec![],
                    vec![SortClause {
                        field: "archived_at".to_string(),
                        descending: false,
                    }],
                )
            })
            .await
            .unwrap();
        assert_eq!(ascending[0].id, alpha.id);

        let descending = repo
            .list(ListOptions {
                include_archived: true,
                ..list_opts(
                    vec![],
                    vec![SortClause {
                        field: "archived_at".to_string(),
                        descending: true,
                    }],
                )
            })
            .await
            .unwrap();
        assert_eq!(descending[0].id, beta.id);
    }

    #[tokio::test]
    async fn field_cmp_treats_an_unrecognized_sort_field_as_equal() {
        let (repo, alpha, beta) = seeded_repo().await;
        let items = repo
            .list(list_opts(
                vec![],
                vec![SortClause {
                    field: "nonexistent".to_string(),
                    descending: false,
                }],
            ))
            .await
            .unwrap();
        let mut ids: Vec<i32> = items.iter().map(|h| h.id).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec![alpha.id.min(beta.id), alpha.id.max(beta.id)]);
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
