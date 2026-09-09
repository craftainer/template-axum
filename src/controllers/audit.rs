//! `GET /audit` -- ports the reference's `FR-0016`. Restricted to the
//! `security`/`detective` roles (`FR-0015`'s role matrix already
//! establishes both; this is their first real consumer), returning the
//! caller's own subject and granted roles -- nothing resource-specific,
//! so this router has no `AppState` field of its own beyond `oidc`.

use axum::extract::State;
use axum::{Json, Router};
use serde::Serialize;

use crate::controllers::AppState;
use crate::oidc::AuthClaims;
use crate::problem_details::AppError;

/// The role set allowed to call `GET /audit` (FR-0033) -- distinct from
/// every Hero role set in `controllers::mod`: this route isn't
/// Hero-specific and grants neither read nor write access to Hero
/// records.
pub const AUDIT_ROLES: &[&str] = &["security", "detective"];

#[derive(Debug, Serialize)]
struct AuditResponse {
    subject: String,
    roles: Vec<String>,
}

async fn audit(
    State(state): State<AppState>,
    AuthClaims(claims): AuthClaims,
) -> Result<Json<AuditResponse>, AppError> {
    claims.require_any_role(&state.settings.oidc_client_id, AUDIT_ROLES)?;
    let subject = claims.subject()?.to_string();
    let roles = claims.granted_roles(&state.settings.oidc_client_id);
    Ok(Json(AuditResponse { subject, roles }))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/", axum::routing::get(audit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Mode, Settings};
    use crate::controllers::DynHeroRepository;
    use crate::health::HealthRegistry;
    use crate::oidc::OidcVerifier;
    use crate::repositories::hero_memory::HeroMemoryRepository;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use std::sync::Arc;
    use tower::ServiceExt;

    fn mock_settings() -> Settings {
        Settings {
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
            mqtt_host: "localhost".to_string(),
            mqtt_port: 1883,
            rate_limit_mock_token_per_minute: 10,
            rate_limit_hero_write_per_minute: 20,
            bulk_action_max_matched: 1000,
            oidc_issuer_url: "http://localhost:8080/realms/template-fastapi".to_string(),
            oidc_authorization_url: "http://localhost:8080/auth".to_string(),
            oidc_token_url: "http://localhost:8080/token".to_string(),
            oidc_client_id: "api".to_string(),
            oidc_audience: None,
        }
    }

    fn app() -> Router {
        let settings = Arc::new(mock_settings());
        let state = AppState {
            oidc: Arc::new(OidcVerifier::new(settings.clone())),
            settings,
            health_registry: Arc::new(HealthRegistry::new()),
            hero_crud: Arc::new(crate::crud::CrudService::new(DynHeroRepository(Box::new(
                HeroMemoryRepository::new(),
            )))),
            rate_limiter: Arc::new(crate::rate_limit::RateLimiter::mock()),
            events: Arc::new(crate::events::EventBus::mock()),
        };
        router().with_state(state)
    }

    fn token(sub: &str, roles: &[&str]) -> String {
        jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
            &serde_json::json!({
                "sub": sub,
                "resource_access": { "api": { "roles": roles } }
            }),
            &jsonwebtoken::EncodingKey::from_secret(b"mock-mode-doesnt-verify-signatures"),
        )
        .unwrap()
    }

    fn authed(sub: &str, roles: &[&str]) -> Request<Body> {
        Request::builder()
            .uri("/")
            .header("Authorization", format!("Bearer {}", token(sub, roles)))
            .body(Body::empty())
            .unwrap()
    }

    async fn json_body(response: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn security_and_detective_can_call_audit() {
        for role in ["security", "detective"] {
            let response = app().oneshot(authed("alice", &[role])).await.unwrap();
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "role {role} should be able to call /audit"
            );
        }
    }

    #[tokio::test]
    async fn every_other_role_is_forbidden() {
        for role in ["viewer", "editor", "maintainer"] {
            let response = app().oneshot(authed("alice", &[role])).await.unwrap();
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "role {role} should not be able to call /audit"
            );
        }
    }

    #[tokio::test]
    async fn a_request_with_no_bearer_token_is_unauthorized() {
        let response = app()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn response_reports_the_callers_subject_and_granted_roles() {
        let response = app()
            .oneshot(authed("alice", &["security", "detective"]))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["subject"], "alice");
        let mut roles: Vec<String> = body["roles"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        roles.sort();
        assert_eq!(roles, vec!["detective".to_string(), "security".to_string()]);
    }
}
