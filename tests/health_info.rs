//! `GET /health/info` (`docs/adrs/0026`, `NFR-0032`): the running build's
//! version and commit from `APP_VERSION` / `APP_COMMIT`, `null` for a value
//! that is unset or empty. Needs no backing service (`MODE=mock`).
//!
//! One test function, because it changes the process environment, which the
//! other tests in a binary would race with.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use template_axum::config::Settings;
use template_axum::{build_router, build_state};
use tower::ServiceExt;

async fn info(app: &Router) -> Value {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health/info")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn health_info_reports_version_and_commit_and_null_when_unset_or_empty() {
    // SAFETY: the only test in this binary, so nothing reads the
    // environment concurrently.
    unsafe {
        std::env::set_var("MODE", "mock");
        std::env::set_var("ALLOW_MOCK_MODE", "1");
        std::env::remove_var("APP_VERSION");
        std::env::remove_var("APP_COMMIT");
    }
    let settings = Settings::from_env().expect("mock settings");
    let app = build_router(build_state(Arc::new(settings)).await);

    assert_eq!(info(&app).await, json!({"version": null, "commit": null}));

    unsafe {
        std::env::set_var("APP_VERSION", "1.2.3");
        std::env::set_var("APP_COMMIT", "abc1234");
    }
    assert_eq!(
        info(&app).await,
        json!({"version": "1.2.3", "commit": "abc1234"})
    );

    unsafe {
        std::env::set_var("APP_VERSION", "");
        std::env::remove_var("APP_COMMIT");
    }
    assert_eq!(info(&app).await, json!({"version": null, "commit": null}));
}
