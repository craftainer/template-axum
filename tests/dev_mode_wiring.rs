//! `lib.rs`'s `Mode::Dev` halves of `build_state`/`build_health_registry`
//! -- the `Mode::Mock` halves have their own unit coverage in `lib.rs`'s
//! colocated tests; this is the real-Postgres/Redis/S3/MQTT/Keycloak
//! path, against the devcontainer stack's own already-running services.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use template_axum::{build_health_registry, build_router, build_state};

#[tokio::test]
async fn dev_mode_health_registry_reports_every_check_healthy() {
    let settings = common::dev_settings(None);
    let db = sea_orm::Database::connect(settings.database_url())
        .await
        .expect("the devcontainer stack's Postgres must be running for the integration tier");

    let registry = build_health_registry(&settings, Some(db)).await;
    let results = registry.run_all().await;
    let mut names: Vec<&str> = results.iter().map(|(name, _)| name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["database", "oidc", "redis", "s3"]);
    assert!(
        results.iter().all(|(_, result)| result.healthy),
        "every real-backend check should report healthy against the live devcontainer stack: {results:?}"
    );
}

#[tokio::test]
async fn dev_mode_build_state_serves_health_live_and_a_real_hero_round_trip() {
    let settings = common::dev_settings(None);
    let state = build_state(settings).await;
    let app = build_router(state);

    let health = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health/live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);

    let http = reqwest::Client::new();
    let editor_token = common::keycloak_token(&http, "editor").await;
    let viewer_token = common::keycloak_token(&http, "viewer").await;

    let mut create_request = Request::builder()
        .method("POST")
        .uri("/crud/v1/heroes/v2/json")
        .header("Authorization", format!("Bearer {editor_token}"))
        .header("Content-Type", "application/json")
        .body(Body::from(
            serde_json::json!({"name": "Spectra", "powers": ["flight"], "power_level": 5})
                .to_string(),
        ))
        .unwrap();
    create_request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            12345,
        ))));
    let create = app.clone().oneshot(create_request).await.unwrap();
    let create_status = create.status();
    let create_body = axum::body::to_bytes(create.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        create_status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&create_body)
    );
    let created: serde_json::Value = serde_json::from_slice(&create_body).unwrap();
    let id = created["id"].as_i64().unwrap();

    let list = app
        .oneshot(
            Request::builder()
                .uri("/crud/v1/heroes/v2/json")
                .header("Authorization", format!("Bearer {viewer_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let body = common::json_body(list).await;
    let ids: Vec<i64> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|hero| hero["id"].as_i64().unwrap())
        .collect();
    assert!(ids.contains(&id), "created hero should appear in the list");
}
