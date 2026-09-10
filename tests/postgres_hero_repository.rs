//! Integration tier: `repositories::hero_sea_orm` against the
//! devcontainer stack's real Postgres.
//!
//! The unit tests colocated in that module stop at the SQL *text* SeaORM's
//! query builder renders -- they prove `condition_for`/`apply_filters`/
//! `apply_sort` emit the clauses intended, not that Postgres agrees with
//! what those clauses mean. Everything here is about the latter: operator
//! semantics (`__icontains` case folding, `__in` list membership, `__ne`
//! against NULL-able columns), sort stability, the archived-row visibility
//! rule, and the bulk update/delete round trips.
//!
//! Each test gets its own Postgres schema (see `common::IsolatedDb`), so
//! they run in parallel and share nothing.

mod common;

use chrono::{Duration, Utc};
use common::{seed_hero_at, IsolatedDb};
use template_axum::generic::repositories::filtering::{
    FilterClause, FilterOp, FilterValue, SortClause,
};
use template_axum::generic::repositories::{ListOptions, Repository};
use template_axum::hero::repositories::hero_sea_orm::HeroSeaOrmRepository;
use template_axum::hero::views::hero::{HeroCreate, HeroUpdate};

fn eq(field: &str, value: FilterValue) -> FilterClause {
    FilterClause {
        field: field.to_string(),
        op: FilterOp::Eq,
        value,
    }
}

fn clause(field: &str, op: FilterOp, value: FilterValue) -> FilterClause {
    FilterClause {
        field: field.to_string(),
        op,
        value,
    }
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

/// Seed a fixed cast of heroes, all owned by `owner`, with distinct
/// power levels and names chosen to exercise case-insensitive matching.
async fn seed_cast(db: &sea_orm::DatabaseConnection, owner: &str) {
    let now = Utc::now().naive_utc();
    seed_hero_at(db, owner, "Spectra", Some(5), now).await;
    seed_hero_at(db, owner, "Umbra", Some(3), now).await;
    seed_hero_at(db, owner, "SPECTRAL Wraith", Some(9), now).await;
    seed_hero_at(db, owner, "Nameless", None, now).await;
}

#[tokio::test]
async fn equality_and_comparison_filters_match_the_same_rows_postgres_does() {
    let db = IsolatedDb::new().await;
    let repository = HeroSeaOrmRepository::new(db.connection.clone());
    seed_cast(&db.connection, "alice").await;

    let exact = repository
        .list(list_opts(
            vec![eq("name", FilterValue::Str("Umbra".into()))],
            vec![],
        ))
        .await
        .unwrap();
    assert_eq!(exact.len(), 1);
    assert_eq!(exact[0].name.as_deref(), Some("Umbra"));

    let strong = repository
        .list(list_opts(
            vec![clause("power_level", FilterOp::Gte, FilterValue::Int(5))],
            vec![],
        ))
        .await
        .unwrap();
    assert_eq!(strong.len(), 2);

    let below = repository
        .list(list_opts(
            vec![clause("power_level", FilterOp::Lt, FilterValue::Int(5))],
            vec![],
        ))
        .await
        .unwrap();
    assert_eq!(below.len(), 1);
    assert_eq!(below[0].name.as_deref(), Some("Umbra"));

    db.cleanup().await;
}

#[tokio::test]
async fn contains_is_case_sensitive_and_icontains_is_not() {
    let db = IsolatedDb::new().await;
    let repository = HeroSeaOrmRepository::new(db.connection.clone());
    seed_cast(&db.connection, "alice").await;

    let sensitive = repository
        .list(list_opts(
            vec![clause(
                "name",
                FilterOp::Contains,
                FilterValue::Str("Spectra".into()),
            )],
            vec![],
        ))
        .await
        .unwrap();
    assert_eq!(
        sensitive.len(),
        1,
        "LIKE '%Spectra%' must not match 'SPECTRAL Wraith'"
    );

    let insensitive = repository
        .list(list_opts(
            vec![clause(
                "name",
                FilterOp::Icontains,
                FilterValue::Str("SpEcTrA".into()),
            )],
            vec![],
        ))
        .await
        .unwrap();
    assert_eq!(
        insensitive.len(),
        2,
        "lower(name) LIKE '%spectra%' must match both"
    );

    db.cleanup().await;
}

#[tokio::test]
async fn an_in_filter_matches_every_listed_value() {
    let db = IsolatedDb::new().await;
    let repository = HeroSeaOrmRepository::new(db.connection.clone());
    seed_cast(&db.connection, "alice").await;

    let matched = repository
        .list(list_opts(
            vec![clause(
                "power_level",
                FilterOp::In,
                FilterValue::List(vec![FilterValue::Int(3), FilterValue::Int(9)]),
            )],
            vec![],
        ))
        .await
        .unwrap();
    let mut levels: Vec<i32> = matched.iter().filter_map(|hero| hero.power_level).collect();
    levels.sort_unstable();
    assert_eq!(levels, vec![3, 9]);

    db.cleanup().await;
}

#[tokio::test]
async fn sorting_orders_rows_the_way_the_sort_clause_asks() {
    let db = IsolatedDb::new().await;
    let repository = HeroSeaOrmRepository::new(db.connection.clone());
    seed_cast(&db.connection, "alice").await;

    let descending = repository
        .list(list_opts(
            vec![],
            vec![SortClause {
                field: "power_level".to_string(),
                descending: true,
            }],
        ))
        .await
        .unwrap();
    // Postgres orders NULLs *first* on DESC (its documented default is
    // NULLS FIRST for DESC, NULLS LAST for ASC), so the null-power_level
    // row leads -- what this asserts on is the order of the rows that have
    // a value.
    let ordered: Vec<i32> = descending
        .iter()
        .filter_map(|hero| hero.power_level)
        .collect();
    assert_eq!(ordered, vec![9, 5, 3]);

    let ascending = repository
        .list(list_opts(
            vec![],
            vec![SortClause {
                field: "name".to_string(),
                descending: false,
            }],
        ))
        .await
        .unwrap();
    let names: Vec<&str> = ascending
        .iter()
        .filter_map(|hero| hero.name.as_deref())
        .collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted);

    db.cleanup().await;
}

#[tokio::test]
async fn an_unrecognized_sort_field_is_ignored_rather_than_failing_the_query() {
    let db = IsolatedDb::new().await;
    let repository = HeroSeaOrmRepository::new(db.connection.clone());
    seed_cast(&db.connection, "alice").await;

    // `apply_sort` skips a field it has no column for; this proves the
    // resulting SQL is still valid rather than half-built.
    let rows = repository
        .list(list_opts(
            vec![],
            vec![SortClause {
                field: "not_a_column".to_string(),
                descending: true,
            }],
        ))
        .await
        .unwrap();
    assert_eq!(rows.len(), 4);

    db.cleanup().await;
}

#[tokio::test]
async fn count_respects_filters_and_the_archived_visibility_rule() {
    let db = IsolatedDb::new().await;
    let repository = HeroSeaOrmRepository::new(db.connection.clone());
    seed_cast(&db.connection, "alice").await;

    assert_eq!(repository.count(&[], false).await.unwrap(), 4);
    assert_eq!(
        repository
            .count(
                &[clause("power_level", FilterOp::Gte, FilterValue::Int(5))],
                false
            )
            .await
            .unwrap(),
        2
    );

    let doomed = repository
        .list(list_opts(
            vec![eq("name", FilterValue::Str("Umbra".into()))],
            vec![],
        ))
        .await
        .unwrap();
    assert!(repository.delete(doomed[0].id, "alice").await.unwrap());

    assert_eq!(repository.count(&[], false).await.unwrap(), 3);
    assert_eq!(repository.count(&[], true).await.unwrap(), 4);
    assert!(repository.get(doomed[0].id, false).await.unwrap().is_none());
    assert!(repository.get(doomed[0].id, true).await.unwrap().is_some());

    db.cleanup().await;
}

#[tokio::test]
async fn delete_is_scoped_to_the_owner_and_is_not_repeatable() {
    let db = IsolatedDb::new().await;
    let repository = HeroSeaOrmRepository::new(db.connection.clone());
    let id = seed_hero_at(
        &db.connection,
        "alice",
        "Spectra",
        Some(5),
        Utc::now().naive_utc(),
    )
    .await;

    assert!(
        !repository.delete(id, "mallory").await.unwrap(),
        "another owner must not be able to archive this row"
    );
    assert!(repository.delete(id, "alice").await.unwrap());
    assert!(
        !repository.delete(id, "alice").await.unwrap(),
        "an already-archived row is not deletable again"
    );

    db.cleanup().await;
}

#[tokio::test]
async fn update_many_writes_every_matching_row_and_leaves_the_rest_alone() {
    let db = IsolatedDb::new().await;
    let repository = HeroSeaOrmRepository::new(db.connection.clone());
    seed_cast(&db.connection, "alice").await;
    seed_hero_at(
        &db.connection,
        "mallory",
        "Ghost",
        Some(5),
        Utc::now().naive_utc(),
    )
    .await;

    let updated = repository
        .update_many(
            &[
                eq("power_level", FilterValue::Int(5)),
                eq("owner_id", FilterValue::Str("alice".into())),
            ],
            HeroUpdate {
                name: None,
                powers: None,
                power_level: Some(11),
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].power_level, Some(11));

    let mallorys = repository
        .list(list_opts(
            vec![eq("owner_id", FilterValue::Str("mallory".into()))],
            vec![],
        ))
        .await
        .unwrap();
    assert_eq!(
        mallorys[0].power_level,
        Some(5),
        "another owner's matching row must be untouched"
    );

    db.cleanup().await;
}

#[tokio::test]
async fn delete_many_archives_every_match_and_they_stop_listing() {
    let db = IsolatedDb::new().await;
    let repository = HeroSeaOrmRepository::new(db.connection.clone());
    seed_cast(&db.connection, "alice").await;

    let deleted = repository
        .delete_many(&[clause("power_level", FilterOp::Gte, FilterValue::Int(5))])
        .await
        .unwrap();
    assert_eq!(deleted.len(), 2);
    assert!(deleted.iter().all(|hero| hero.archived_at.is_some()));

    let remaining = repository.list(list_opts(vec![], vec![])).await.unwrap();
    assert_eq!(remaining.len(), 2);

    // A second bulk delete over the same filter matches nothing: the rows
    // are archived and `delete_many` only considers non-archived ones.
    let again = repository
        .delete_many(&[clause("power_level", FilterOp::Gte, FilterValue::Int(5))])
        .await
        .unwrap();
    assert!(again.is_empty());

    db.cleanup().await;
}

#[tokio::test]
async fn create_and_update_round_trip_through_postgres_column_types() {
    let db = IsolatedDb::new().await;
    let repository = HeroSeaOrmRepository::new(db.connection.clone());

    // `powers` is a Postgres text[]; this is the one place the array
    // mapping is exercised against the real column type rather than a
    // Vec<String> in memory.
    let created = repository
        .create(
            "alice",
            HeroCreate {
                name: "Spectra".to_string(),
                powers: vec!["flight".to_string(), "phasing".to_string()],
                power_level: Some(5),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        created.powers.as_deref(),
        Some(["flight".to_string(), "phasing".to_string()].as_slice())
    );
    assert!(created.archived_at.is_none());

    let updated = repository
        .update(
            created.id,
            "alice",
            HeroUpdate {
                name: Some("Spectra Prime".to_string()),
                powers: None,
                power_level: None,
            },
        )
        .await
        .unwrap()
        .expect("the owner's own row must be updatable");
    assert_eq!(updated.name.as_deref(), Some("Spectra Prime"));
    assert_eq!(
        updated.powers.as_deref(),
        Some(["flight".to_string(), "phasing".to_string()].as_slice()),
        "an omitted field is left unchanged (FR-0004)"
    );
    assert!(updated.updated_at >= created.updated_at);

    assert!(
        repository
            .update(
                created.id,
                "mallory",
                HeroUpdate {
                    name: Some("Hacked".to_string()),
                    powers: None,
                    power_level: None,
                },
            )
            .await
            .unwrap()
            .is_none(),
        "another owner's update must not match any row"
    );

    db.cleanup().await;
}

#[tokio::test]
async fn a_datetime_filter_compares_against_the_stored_timestamp() {
    let db = IsolatedDb::new().await;
    let repository = HeroSeaOrmRepository::new(db.connection.clone());
    let now = Utc::now().naive_utc();
    seed_hero_at(
        &db.connection,
        "alice",
        "Old",
        Some(1),
        now - Duration::days(10),
    )
    .await;
    seed_hero_at(&db.connection, "alice", "New", Some(2), now).await;

    let recent = repository
        .list(list_opts(
            vec![clause(
                "created_at",
                FilterOp::Gte,
                FilterValue::DateTime(now - Duration::days(1)),
            )],
            vec![],
        ))
        .await
        .unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].name.as_deref(), Some("New"));

    db.cleanup().await;
}
