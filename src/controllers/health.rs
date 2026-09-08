//! `GET /health/live` and `GET /health/ready` -- port of `controllers/health.py`.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use std::collections::BTreeMap;

use crate::controllers::AppState;
use crate::health::HealthCheckResult;

#[derive(Serialize)]
struct LiveResponse {
    status: &'static str,
}

/// Dependency-free liveness probe (FR-0010) -- never touches
/// `HealthRegistry`, so a downstream outage can't fail liveness.
async fn live() -> Json<LiveResponse> {
    Json(LiveResponse { status: "ok" })
}

#[derive(Serialize)]
struct ReadyResponse {
    status: &'static str,
    checks: BTreeMap<String, HealthCheckResult>,
}

/// Runs every registered check concurrently; 200 if all healthy, 503
/// otherwise (FR-0011).
async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    let results = state.health_registry.run_all().await;
    let healthy = results.iter().all(|(_, result)| result.healthy);
    let checks: BTreeMap<String, HealthCheckResult> = results.into_iter().collect();
    let status = if healthy {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let body = ReadyResponse {
        status: if healthy { "ok" } else { "degraded" },
        checks,
    };
    (status, Json(body))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/live", get(live))
        .route("/ready", get(ready))
}
