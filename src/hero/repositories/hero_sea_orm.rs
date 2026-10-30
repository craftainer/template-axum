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

use crate::hero::models::hero::{self, ActiveModel, Column, Entity};
use crate::hero::views::hero::{HeroCreate, HeroUpdate};
use craftainer_crud::repositories::filtering::{FilterClause, FilterOp, FilterValue, SortClause};
use craftainer_crud::repositories::{ListOptions, RepoError, Repository};

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
        FilterValue::Float(v) => (*v).into(),
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

/// Archive visibility: `only_archived` (which implies `include_archived`)
/// keeps just the soft-deleted rows; otherwise live rows, plus archived ones
/// when `include_archived`.
fn archive_scope(
    query: sea_orm::Select<Entity>,
    include_archived: bool,
    only_archived: bool,
) -> sea_orm::Select<Entity> {
    if only_archived {
        query.filter(Column::ArchivedAt.is_not_null())
    } else if include_archived {
        query
    } else {
        query.filter(Column::ArchivedAt.is_null())
    }
}

fn apply_filters<Q: QueryFilter>(mut query: Q, filters: &[FilterClause]) -> Q {
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
        let mut query = archive_scope(Entity::find(), opts.include_archived, opts.only_archived);
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
        only_archived: bool,
        // Hero has no publish window (docs/adrs/0020): nothing to filter.
        _include_unpublished: bool,
    ) -> Result<u64, RepoError> {
        let mut query = archive_scope(Entity::find(), include_archived, only_archived);
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
        // One `UPDATE ... WHERE id AND owner AND live RETURNING *` (docs/adrs/
        // 0028): ownership and visibility are decided by the statement that
        // writes, so a concurrent delete cannot slip between a read and a write.
        let rows = Entity::update_many()
            .set(update_values(data, Utc::now().naive_utc()))
            .filter(Column::Id.eq(id))
            .filter(Column::OwnerId.eq(owner_id))
            .filter(Column::ArchivedAt.is_null())
            .exec_with_returning(&self.db)
            .await
            .map_err(backend_err)?;
        Ok(rows.into_iter().next())
    }

    async fn update_many(
        &self,
        filters: &[FilterClause],
        data: HeroUpdate,
    ) -> Result<Vec<hero::Model>, RepoError> {
        let query = Entity::update_many()
            .set(update_values(data, Utc::now().naive_utc()))
            .filter(Column::ArchivedAt.is_null());
        let rows = apply_filters(query, filters)
            .exec_with_returning(&self.db)
            .await
            .map_err(backend_err)?;
        Ok(by_id(rows))
    }

    async fn delete(&self, id: i32, owner_id: &str) -> Result<bool, RepoError> {
        let rows = Entity::update_many()
            .set(archive_values(Utc::now().naive_utc()))
            .filter(Column::Id.eq(id))
            .filter(Column::OwnerId.eq(owner_id))
            .filter(Column::ArchivedAt.is_null())
            .exec_with_returning(&self.db)
            .await
            .map_err(backend_err)?;
        Ok(!rows.is_empty())
    }

    async fn delete_many(
        &self,
        filters: &[FilterClause],
        // Hero has no publish window (docs/adrs/0020): every live row is in it.
        _include_unpublished: bool,
    ) -> Result<Vec<hero::Model>, RepoError> {
        let query = Entity::update_many()
            .set(archive_values(Utc::now().naive_utc()))
            .filter(Column::ArchivedAt.is_null());
        let rows = apply_filters(query, filters)
            .exec_with_returning(&self.db)
            .await
            .map_err(backend_err)?;
        Ok(by_id(rows))
    }
}

/// The `SET` list of an update: only the fields the request carries, plus
/// `updated_at` (the ETag input, `docs/adrs/0023`).
fn update_values(data: HeroUpdate, now: chrono::NaiveDateTime) -> ActiveModel {
    let mut active = ActiveModel {
        updated_at: Set(now),
        ..Default::default()
    };
    if let Some(name) = data.name {
        active.name = Set(Some(name));
    }
    if let Some(powers) = data.powers {
        active.powers = Set(Some(powers));
    }
    if data.power_level.is_some() {
        active.power_level = Set(data.power_level);
    }
    active
}

/// The `SET` list of a soft delete (`docs/adrs/0008`).
fn archive_values(now: chrono::NaiveDateTime) -> ActiveModel {
    ActiveModel {
        archived_at: Set(Some(now)),
        updated_at: Set(now),
        ..Default::default()
    }
}

/// `RETURNING` order is unspecified; the per-row loops this replaces
/// returned rows in primary-key order in practice, so keep that.
fn by_id(mut rows: Vec<hero::Model>) -> Vec<hero::Model> {
    rows.sort_by_key(|row| row.id);
    rows
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
        assert_eq!(scalar(&FilterValue::Float(1.5)), 1.5f64.into());
        let now = Utc::now().naive_utc();
        assert_eq!(scalar(&FilterValue::DateTime(now)), now.into());
    }

    #[test]
    fn archive_scope_renders_the_three_visibility_modes() {
        let sql = |include, only| {
            archive_scope(Entity::find(), include, only)
                .build(DbBackend::Postgres)
                .to_string()
        };
        assert!(sql(false, false).contains("IS NULL"));
        assert!(!sql(true, false).contains("WHERE"));
        assert!(sql(false, true).contains("IS NOT NULL"));
        assert!(sql(true, true).contains("IS NOT NULL"));
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
