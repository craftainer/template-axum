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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Mode, Settings};
    use crate::controllers::DynHeroRepository;
    use crate::health::checks::MockHealthCheck;
    use crate::health::{HealthCheck, HealthRegistry};
    use crate::oidc::OidcVerifier;
    use crate::repositories::hero_memory::HeroMemoryRepository;
    use async_trait::async_trait;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use std::sync::Arc;
    use tower::ServiceExt;

    struct AlwaysUnhealthy;
    #[async_trait]
    impl HealthCheck for AlwaysUnhealthy {
        fn name(&self) -> &str {
            "flaky-dependency"
        }
        async fn check(&self) -> crate::health::HealthCheckResult {
            crate::health::HealthCheckResult {
                healthy: false,
                detail: Some("down".to_string()),
            }
        }
    }

    fn state_with_registry(registry: HealthRegistry) -> AppState {
        let settings = Arc::new(Settings {
            app_name: "template-axum".to_string(),
            mode: Mode::Mock,
            allow_mock_mode: true,
            postgres_user: "app".to_string(),
            postgres_password: "app".to_string(),
            postgres_db: "app".to_string(),
            postgres_host: "localhost".to_string(),
            postgres_port: 5432,
            s3_endpoint_url: "http://localhost:9000".to_string(),
            s3_access_key: "rustfsadmin".to_string(),
            s3_secret_key: "rustfsadmin".to_string(),
            redis_url: "redis://localhost:6379/0".to_string(),
            rate_limit_mock_token_per_minute: 10,
            rate_limit_hero_write_per_minute: 20,
            bulk_action_max_matched: 1000,
            oidc_issuer_url: "http://localhost:8080".to_string(),
            oidc_authorization_url: "http://localhost:8080/auth".to_string(),
            oidc_token_url: "http://localhost:8080/token".to_string(),
            oidc_client_id: "api".to_string(),
            oidc_audience: None,
        });
        AppState {
            oidc: Arc::new(OidcVerifier::new(settings.clone())),
            settings,
            health_registry: Arc::new(registry),
            hero_crud: Arc::new(crate::crud::CrudService::new(DynHeroRepository(Box::new(
                HeroMemoryRepository::new(),
            )))),
            rate_limiter: Arc::new(crate::rate_limit::RateLimiter::mock()),
        }
    }

    #[tokio::test]
    async fn live_never_requires_auth_or_touches_dependencies() {
        let app = router().with_state(state_with_registry(HealthRegistry::new()));
        let response = app
            .oneshot(Request::builder().uri("/live").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["status"], "ok");
    }

    #[tokio::test]
    async fn ready_returns_200_when_every_check_is_healthy() {
        let mut registry = HealthRegistry::new();
        registry.register(Box::new(MockHealthCheck::new("database")));
        registry.register(Box::new(MockHealthCheck::new("redis")));
        let app = router().with_state(state_with_registry(registry));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/ready")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["status"], "ok");
        assert_eq!(body["checks"]["database"]["healthy"], true);
    }

    #[tokio::test]
    async fn ready_returns_503_when_any_single_check_is_unhealthy() {
        let mut registry = HealthRegistry::new();
        registry.register(Box::new(MockHealthCheck::new("database")));
        registry.register(Box::new(AlwaysUnhealthy));
        let app = router().with_state(state_with_registry(registry));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/ready")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["status"], "degraded");
        // The one healthy check must still report healthy -- one failure
        // doesn't taint the others (NFR-0008).
        assert_eq!(body["checks"]["database"]["healthy"], true);
        assert_eq!(body["checks"]["flaky-dependency"]["healthy"], false);
    }
}
