//! Postgres-backed `Repository` for Hero, via SeaORM. Used whenever
//! `Mode` is not `Mock` -- see `src/hero/repositories/hero_memory.rs` for
//! the `Mode::Mock` counterpart and `docs/adrs/0006` for the swap
//! mechanism.

use async_trait::async_trait;
use chrono::Utc;
use sea_orm::sea_query::{Condition, Expr, ExprTrait, Func, SimpleExpr};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect, Set,
};

use crate::generic::repositories::filtering::{FilterClause, FilterOp, FilterValue, SortClause};
use crate::generic::repositories::{ListOptions, RepoError, Repository};
use crate::hero::models::hero::{self, ActiveModel, Column, Entity};
use crate::hero::views::hero::{HeroCreate, HeroUpdate};

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

/// Map one field name to its SeaORM `Column` -- the one place `FilterClause`/
/// `SortClause`'s storage-agnostic field names become Hero-specific, mirroring
/// `filtering.py`'s "each concrete repository interprets these itself".
fn column_for(field: &str) -> Option<Column> {
    match field {
        "id" => Some(Column::Id),
        "name" => Some(Column::Name),
        "power_level" => Some(Column::PowerLevel),
        "owner_id" => Some(Column::OwnerId),
        "archived_at" => Some(Column::ArchivedAt),
        "created_at" => Some(Column::CreatedAt),
        "updated_at" => Some(Column::UpdatedAt),
        _ => None,
    }
}

/// Build one `SimpleExpr` for a `FilterClause` -- an unrecognized field
/// (already rejected by `crud_query.rs`'s parser before reaching here)
/// falls back to an always-false expression rather than panicking or
/// silently matching everything.
fn condition_for(clause: &FilterClause) -> SimpleExpr {
    let Some(column) = column_for(&clause.field) else {
        return Expr::val(1).eq(0);
    };
    match (&clause.op, &clause.value) {
        (FilterOp::Eq, value) => column.eq(scalar(value)),
        (FilterOp::Ne, value) => column.ne(scalar(value)),
        (FilterOp::Lt, value) => column.lt(scalar(value)),
        (FilterOp::Lte, value) => column.lte(scalar(value)),
        (FilterOp::Gt, value) => column.gt(scalar(value)),
        (FilterOp::Gte, value) => column.gte(scalar(value)),
        (FilterOp::In, FilterValue::List(values)) => {
            column.is_in(values.iter().map(scalar).collect::<Vec<_>>())
        }
        (FilterOp::In, other) => column.is_in(vec![scalar(other)]),
        (FilterOp::Contains, value) => column.contains(scalar_str(value)),
        (FilterOp::Icontains, value) => {
            SimpleExpr::from(Func::lower(Expr::col(column))).like(format!(
                "%{}%",
                scalar_str(value).to_lowercase().replace(['%', '_'], "")
            ))
        }
    }
}

fn scalar(value: &FilterValue) -> sea_orm::Value {
    match value {
        FilterValue::Str(v) => v.clone().into(),
        FilterValue::Int(v) => (*v as i32).into(),
        FilterValue::DateTime(v) => (*v).into(),
        FilterValue::Bool(v) => (*v).into(),
        // Only reachable for a malformed FilterOp::In clause whose value
        // isn't a List -- crud_query.rs's parser never produces this.
        FilterValue::List(_) => sea_orm::Value::Int(None),
    }
}

fn scalar_str(value: &FilterValue) -> String {
    match value {
        FilterValue::Str(v) => v.clone(),
        _ => String::new(),
    }
}

fn apply_filters(
    mut query: sea_orm::Select<Entity>,
    filters: &[FilterClause],
) -> sea_orm::Select<Entity> {
    for clause in filters {
        query = query.filter(Condition::all().add(condition_for(clause)));
    }
    query
}

fn apply_sort(mut query: sea_orm::Select<Entity>, sort: &[SortClause]) -> sea_orm::Select<Entity> {
    for clause in sort {
        let Some(column) = column_for(&clause.field) else {
            continue;
        };
        query = if clause.descending {
            query.order_by_desc(column)
        } else {
            query.order_by_asc(column)
        };
    }
    query
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
        query = apply_filters(query, &opts.filters);
        query = if opts.sort.is_empty() {
            query.order_by_asc(Column::Id)
        } else {
            apply_sort(query, &opts.sort)
        };
        query
            .offset(opts.skip)
            .limit(opts.limit)
            .all(&self.db)
            .await
            .map_err(backend_err)
    }

    async fn count(
        &self,
        filters: &[FilterClause],
        include_archived: bool,
    ) -> Result<u64, RepoError> {
        let mut query = Entity::find();
        if !include_archived {
            query = query.filter(Column::ArchivedAt.is_null());
        }
        query = apply_filters(query, filters);
        query.count(&self.db).await.map_err(backend_err)
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

    async fn update_many(
        &self,
        filters: &[FilterClause],
        data: HeroUpdate,
    ) -> Result<Vec<hero::Model>, RepoError> {
        let mut query = Entity::find().filter(Column::ArchivedAt.is_null());
        query = apply_filters(query, filters);
        let existing = query.all(&self.db).await.map_err(backend_err)?;

        // HeroUpdate isn't Clone (a plain request DTO); destructure once so
        // each row's ActiveModel gets its own clone of the owned fields.
        let HeroUpdate {
            name,
            powers,
            power_level,
        } = data;
        let now = Utc::now().naive_utc();
        let mut updated = Vec::with_capacity(existing.len());
        for row in existing {
            let mut active: ActiveModel = row.into();
            if let Some(name) = name.clone() {
                active.name = Set(Some(name));
            }
            if let Some(powers) = powers.clone() {
                active.powers = Set(Some(powers));
            }
            if power_level.is_some() {
                active.power_level = Set(power_level);
            }
            active.updated_at = Set(now);
            updated.push(active.update(&self.db).await.map_err(backend_err)?);
        }
        Ok(updated)
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

    async fn delete_many(&self, filters: &[FilterClause]) -> Result<Vec<hero::Model>, RepoError> {
        let mut query = Entity::find().filter(Column::ArchivedAt.is_null());
        query = apply_filters(query, filters);
        let existing = query.all(&self.db).await.map_err(backend_err)?;

        let now = Utc::now().naive_utc();
        let mut deleted = Vec::with_capacity(existing.len());
        for row in existing {
            let mut active: ActiveModel = row.into();
            active.archived_at = Set(Some(now));
            active.updated_at = Set(now);
            deleted.push(active.update(&self.db).await.map_err(backend_err)?);
        }
        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{DbBackend, QueryTrait};

    // No live Postgres needed for any of this: `column_for`/`condition_for`/
    // `apply_filters`/`apply_sort` are pure functions over SeaORM's query
    // builder, and QueryTrait::build() renders the SQL text without ever
    // opening a connection -- what tests/README.md's "don't reach a real
    // Postgres from a src/ unit test" rule reserves for the not-yet-built
    // integration tier is the actual round-trip against a live database,
    // not the query-construction logic itself.

    #[test]
    fn column_for_maps_every_filterable_hero_field() {
        for field in [
            "id",
            "name",
            "power_level",
            "owner_id",
            "archived_at",
            "created_at",
            "updated_at",
        ] {
            assert!(
                column_for(field).is_some(),
                "{field} should map to a Column"
            );
        }
        assert!(column_for("nope").is_none());
    }

    #[test]
    fn apply_filters_renders_an_eq_clause_into_the_where_clause() {
        let filters = vec![FilterClause {
            field: "name".to_string(),
            op: FilterOp::Eq,
            value: FilterValue::Str("Spectra".to_string()),
        }];
        let sql = apply_filters(Entity::find(), &filters)
            .build(DbBackend::Postgres)
            .to_string();
        assert!(sql.contains("WHERE"));
        assert!(sql.contains("name"));
    }

    #[test]
    fn backend_err_wraps_the_dberr_display_text() {
        let RepoError::Backend(msg) = backend_err(sea_orm::DbErr::ConvertFromU64("i32"));
        assert!(msg.contains("i32"));
    }

    #[test]
    fn apply_filters_renders_a_ne_clause() {
        let filters = vec![FilterClause {
            field: "name".to_string(),
            op: FilterOp::Ne,
            value: FilterValue::Str("Spectra".to_string()),
        }];
        let sql = apply_filters(Entity::find(), &filters)
            .build(DbBackend::Postgres)
            .to_string();
        assert!(sql.contains("<>"));
    }

    #[test]
    fn apply_filters_renders_a_lte_clause() {
        let filters = vec![FilterClause {
            field: "power_level".to_string(),
            op: FilterOp::Lte,
            value: FilterValue::Int(5),
        }];
        let sql = apply_filters(Entity::find(), &filters)
            .build(DbBackend::Postgres)
            .to_string();
        assert!(sql.contains("<="));
    }

    #[test]
    fn apply_filters_renders_a_gt_clause() {
        let filters = vec![FilterClause {
            field: "power_level".to_string(),
            op: FilterOp::Gt,
            value: FilterValue::Int(5),
        }];
        let sql = apply_filters(Entity::find(), &filters)
            .build(DbBackend::Postgres)
            .to_string();
        assert!(sql.contains(" > "));
    }

    #[test]
    fn apply_filters_renders_an_in_clause() {
        let filters = vec![FilterClause {
            field: "id".to_string(),
            op: FilterOp::In,
            value: FilterValue::List(vec![FilterValue::Int(1), FilterValue::Int(2)]),
        }];
        let sql = apply_filters(Entity::find(), &filters)
            .build(DbBackend::Postgres)
            .to_string();
        assert!(sql.contains("IN"));
    }

    #[test]
    fn apply_filters_renders_a_contains_clause() {
        let filters = vec![FilterClause {
            field: "name".to_string(),
            op: FilterOp::Contains,
            value: FilterValue::Str("Spec".to_string()),
        }];
        let sql = apply_filters(Entity::find(), &filters)
            .build(DbBackend::Postgres)
            .to_string();
        assert!(sql.contains("LIKE"));
    }

    #[test]
    fn apply_filters_renders_an_icontains_clause_lowercasing_both_sides() {
        let filters = vec![FilterClause {
            field: "name".to_string(),
            op: FilterOp::Icontains,
            value: FilterValue::Str("SPEC".to_string()),
        }];
        let sql = apply_filters(Entity::find(), &filters)
            .build(DbBackend::Postgres)
            .to_string();
        assert!(sql.to_lowercase().contains("lower"));
        assert!(sql.contains("%spec%"));
    }

    #[test]
    fn apply_filters_on_an_unrecognized_field_renders_an_always_false_clause() {
        let filters = vec![FilterClause {
            field: "nope".to_string(),
            op: FilterOp::Eq,
            value: FilterValue::Str("x".to_string()),
        }];
        let sql = apply_filters(Entity::find(), &filters)
            .build(DbBackend::Postgres)
            .to_string();
        assert!(sql.contains("WHERE"));
    }

    #[test]
    fn apply_sort_renders_asc_and_desc_order_by_clauses() {
        let sort = vec![
            SortClause {
                field: "name".to_string(),
                descending: false,
            },
            SortClause {
                field: "id".to_string(),
                descending: true,
            },
        ];
        let sql = apply_sort(Entity::find(), &sort)
            .build(DbBackend::Postgres)
            .to_string();
        assert!(sql.contains("ORDER BY"));
        assert!(sql.contains("ASC"));
        assert!(sql.contains("DESC"));
    }

    #[test]
    fn apply_sort_skips_an_unrecognized_field() {
        let sort = vec![SortClause {
            field: "nope".to_string(),
            descending: false,
        }];
        // Should not panic, and should produce no ORDER BY at all.
        let sql = apply_sort(Entity::find(), &sort)
            .build(DbBackend::Postgres)
            .to_string();
        assert!(!sql.contains("ORDER BY"));
    }

    #[test]
    fn scalar_maps_every_filtervalue_variant() {
        assert_eq!(scalar(&FilterValue::Str("x".to_string())), "x".into());
        assert_eq!(scalar(&FilterValue::Int(5)), 5i32.into());
        assert_eq!(scalar(&FilterValue::Bool(true)), true.into());
        let now = Utc::now().naive_utc();
        assert_eq!(scalar(&FilterValue::DateTime(now)), now.into());
    }

    #[test]
    fn scalar_str_defaults_to_empty_for_a_non_string_value() {
        assert_eq!(scalar_str(&FilterValue::Int(1)), "");
        assert_eq!(scalar_str(&FilterValue::Str("x".to_string())), "x");
    }

    #[test]
    fn scalar_defaults_to_a_null_int_for_a_malformed_list_value() {
        // Only reachable for a malformed FilterOp::In clause whose value
        // isn't itself a List -- crud_query.rs's parser never produces
        // this (see `scalar`'s own doc comment).
        assert_eq!(
            scalar(&FilterValue::List(vec![FilterValue::Int(1)])),
            sea_orm::Value::Int(None)
        );
    }

    #[test]
    fn apply_filters_renders_an_in_clause_for_a_malformed_non_list_value() {
        // Same malformed-input case as above, exercised through
        // `condition_for`'s `(FilterOp::In, other)` fallback arm.
        let filters = vec![FilterClause {
            field: "id".to_string(),
            op: FilterOp::In,
            value: FilterValue::Int(1),
        }];
        let sql = apply_filters(Entity::find(), &filters)
            .build(DbBackend::Postgres)
            .to_string();
        assert!(sql.contains("IN"));
    }
}
