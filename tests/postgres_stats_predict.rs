//! Integration tier: `/stats` and `/predict` over HTTP, backed by the
//! devcontainer stack's real Postgres.
//!
//! Two gaps this closes, both called out in `docs/plans/`:
//!
//! - `/stats`' aggregate reads (`count`, the capped history `list`, the
//!   archived/visible difference behind `lifecycle`) were only ever run
//!   against the in-memory repository.
//! - `/predict`'s *happy path* had no end-to-end coverage at all. Every
//!   record the unit-test harness can create is stamped with the real
//!   "now", so multiple heroes always land in one day bucket and the route
//!   could only ever be observed returning its below-two-buckets `422`.
//!   `common::seed_hero_at` (test-only -- see its own doc comment) writes
//!   rows with a chosen `created_at`, which is what makes a real
//!   multi-bucket forecast reachable through HTTP.

mod common;

use axum::http::StatusCode;
use chrono::{Duration, Utc};
use common::{authed_request, json_body, seed_hero_at, state_for, IsolatedDb};
use template_axum::events::EventBus;
use tower::ServiceExt;

fn app(db: sea_orm::DatabaseConnection) -> axum::Router {
    template_axum::controllers::heroes::router().with_state(state_for(db, EventBus::mock()))
}

#[tokio::test]
async fn stats_aggregates_are_computed_by_postgres_over_real_rows() {
    let db = IsolatedDb::new().await;
    let now = Utc::now().naive_utc();
    seed_hero_at(&db.connection, "alice", "A", Some(2), now).await;
    seed_hero_at(&db.connection, "alice", "B", Some(4), now).await;
    seed_hero_at(&db.connection, "alice", "C", None, now).await;

    let response = app(db.connection.clone())
        .oneshot(authed_request(
            "GET",
            "/stats",
            "alice",
            &["viewer"],
            serde_json::Value::Null,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;

    assert_eq!(body["total"], 3);
    let power_level = body["numeric"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["field"] == "power_level")
        .unwrap();
    // The null-power_level row is excluded from the numeric aggregate, the
    // same way SQL's own AVG()/SUM() ignore NULLs.
    assert_eq!(power_level["count"], 2);
    assert_eq!(power_level["total"], 6.0);
    assert_eq!(power_level["average"], 3.0);
    assert_eq!(power_level["minimum"], 2.0);
    assert_eq!(power_level["maximum"], 4.0);
    assert_eq!(body["lifecycle"]["archived"], 0);

    db.cleanup().await;
}

#[tokio::test]
async fn stats_lifecycle_counts_archived_rows_separately_from_the_visible_total() {
    let db = IsolatedDb::new().await;
    let now = Utc::now().naive_utc();
    let doomed = seed_hero_at(&db.connection, "alice", "A", Some(1), now).await;
    seed_hero_at(&db.connection, "alice", "B", Some(2), now).await;

    let deleted = app(db.connection.clone())
        .oneshot(authed_request(
            "DELETE",
            &format!("/?id={doomed}"),
            "alice",
            &["maintainer"],
            serde_json::Value::Null,
        ))
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);

    let visible = json_body(
        app(db.connection.clone())
            .oneshot(authed_request(
                "GET",
                "/stats",
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(visible["total"], 1);
    assert_eq!(visible["lifecycle"]["archived"], 1);

    let including_archived = json_body(
        app(db.connection.clone())
            .oneshot(authed_request(
                "GET",
                "/stats?include_archived=true",
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(including_archived["total"], 2);

    db.cleanup().await;
}

#[tokio::test]
async fn stats_time_series_buckets_rows_by_their_real_created_at() {
    let db = IsolatedDb::new().await;
    let now = Utc::now().naive_utc();
    seed_hero_at(
        &db.connection,
        "alice",
        "A",
        Some(1),
        now - Duration::days(2),
    )
    .await;
    seed_hero_at(
        &db.connection,
        "alice",
        "B",
        Some(1),
        now - Duration::days(1),
    )
    .await;
    seed_hero_at(
        &db.connection,
        "alice",
        "C",
        Some(1),
        now - Duration::days(1),
    )
    .await;

    let body = json_body(
        app(db.connection.clone())
            .oneshot(authed_request(
                "GET",
                "/stats?bucket=day",
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap(),
    )
    .await;
    let series = body["time_series"].as_array().unwrap();
    assert_eq!(series.len(), 2, "two distinct day buckets");
    assert_eq!(series[0]["count"], 1);
    assert_eq!(series[1]["count"], 2);

    db.cleanup().await;
}

#[tokio::test]
async fn predict_returns_a_real_multi_bucket_record_count_forecast() {
    let db = IsolatedDb::new().await;
    let now = Utc::now().naive_utc();
    // A clean upward trend: 1 record three days ago, 2 two days ago, 3
    // yesterday -- enough distinct day buckets for the OLS fit, and a
    // slope obvious enough to assert on.
    seed_hero_at(
        &db.connection,
        "alice",
        "A",
        Some(1),
        now - Duration::days(3),
    )
    .await;
    for (offset, name) in [(2, "B"), (2, "C")] {
        seed_hero_at(
            &db.connection,
            "alice",
            name,
            Some(1),
            now - Duration::days(offset),
        )
        .await;
    }
    for name in ["D", "E", "F"] {
        seed_hero_at(
            &db.connection,
            "alice",
            name,
            Some(1),
            now - Duration::days(1),
        )
        .await;
    }

    let response = app(db.connection.clone())
        .oneshot(authed_request(
            "GET",
            "/predict?bucket=day&periods=2",
            "alice",
            &["viewer"],
            serde_json::Value::Null,
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "three day buckets of history is enough to forecast"
    );
    let body = json_body(response).await;

    assert_eq!(body["method"], "linear_regression");
    assert!(body["field"].is_null(), "no ?field= means record count");
    assert_eq!(body["bucket"], "day");
    assert_eq!(body["last_known"]["value"], 3.0);

    let predictions = body["predictions"].as_array().unwrap();
    assert_eq!(predictions.len(), 2);
    let first = predictions[0]["value"].as_f64().unwrap();
    let second = predictions[1]["value"].as_f64().unwrap();
    assert!(
        first > 3.0 && second > first,
        "an upward trend must project upward, got {first} then {second}"
    );
    // Each prediction advances exactly one day past the last known bucket.
    assert_ne!(
        predictions[0]["bucket_start"],
        body["last_known"]["bucket_start"]
    );

    db.cleanup().await;
}

#[tokio::test]
async fn predict_forecasts_a_numeric_fields_per_bucket_sum_when_asked() {
    let db = IsolatedDb::new().await;
    let now = Utc::now().naive_utc();
    seed_hero_at(
        &db.connection,
        "alice",
        "A",
        Some(10),
        now - Duration::days(2),
    )
    .await;
    seed_hero_at(
        &db.connection,
        "alice",
        "B",
        Some(20),
        now - Duration::days(1),
    )
    .await;
    seed_hero_at(&db.connection, "alice", "C", Some(30), now).await;

    let body = json_body(
        app(db.connection.clone())
            .oneshot(authed_request(
                "GET",
                "/predict?bucket=day&periods=1&field=power_level",
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap(),
    )
    .await;

    assert_eq!(body["field"], "power_level");
    assert_eq!(body["last_known"]["value"], 30.0);
    let projected = body["predictions"][0]["value"].as_f64().unwrap();
    assert!(
        (projected - 40.0).abs() < 0.001,
        "a perfectly linear 10/20/30 series must project 40, got {projected}"
    );

    db.cleanup().await;
}

#[tokio::test]
async fn predict_ignores_archived_rows_when_building_its_history() {
    let db = IsolatedDb::new().await;
    let now = Utc::now().naive_utc();
    seed_hero_at(
        &db.connection,
        "alice",
        "A",
        Some(1),
        now - Duration::days(1),
    )
    .await;
    let archived = seed_hero_at(&db.connection, "alice", "B", Some(1), now).await;

    app(db.connection.clone())
        .oneshot(authed_request(
            "DELETE",
            &format!("/?id={archived}"),
            "alice",
            &["maintainer"],
            serde_json::Value::Null,
        ))
        .await
        .unwrap();

    // One visible row is left, so only one day bucket of history remains
    // and the forecast is refused -- which is exactly how we observe that
    // the archived row was excluded.
    let response = app(db.connection.clone())
        .oneshot(authed_request(
            "GET",
            "/predict?bucket=day&periods=1",
            "alice",
            &["viewer"],
            serde_json::Value::Null,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    db.cleanup().await;
}

#[tokio::test]
async fn filter_and_sort_query_parameters_reach_postgres_through_the_router() {
    let db = IsolatedDb::new().await;
    let now = Utc::now().naive_utc();
    seed_hero_at(&db.connection, "alice", "Umbra", Some(3), now).await;
    seed_hero_at(&db.connection, "alice", "Spectra", Some(9), now).await;

    let body = json_body(
        app(db.connection.clone())
            .oneshot(authed_request(
                "GET",
                "/?sort=-power_level",
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap(),
    )
    .await;
    let heroes = body.as_array().unwrap();
    assert_eq!(heroes[0]["name"], "Spectra");
    assert_eq!(heroes[1]["name"], "Umbra");

    let filtered = json_body(
        app(db.connection.clone())
            .oneshot(authed_request(
                "GET",
                "/?power_level__min=5",
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(filtered.as_array().unwrap().len(), 1);

    db.cleanup().await;
}

#[tokio::test]
async fn bulk_update_and_delete_round_trip_through_postgres() {
    let db = IsolatedDb::new().await;
    let now = Utc::now().naive_utc();
    seed_hero_at(&db.connection, "alice", "A", Some(5), now).await;
    seed_hero_at(&db.connection, "alice", "B", Some(5), now).await;
    seed_hero_at(&db.connection, "mallory", "C", Some(5), now).await;

    let updated = json_body(
        app(db.connection.clone())
            .oneshot(authed_request(
                "PATCH",
                "/?power_level=5",
                "alice",
                &["editor"],
                serde_json::json!({"power_level": 10}),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(updated["matched"], 2, "another owner's row is out of scope");

    let deleted = json_body(
        app(db.connection.clone())
            .oneshot(authed_request(
                "DELETE",
                "/?power_level=10",
                "alice",
                &["maintainer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(deleted["matched"], 2);

    let remaining = json_body(
        app(db.connection.clone())
            .oneshot(authed_request(
                "GET",
                "/",
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(remaining.as_array().unwrap().len(), 1);

    db.cleanup().await;
}
