//! `POST /mock/token` -- mounted only under `Mode::Mock` (FR-0017). Mints a
//! Keycloak-shaped JWT so RBAC is exercisable with zero real OIDC
//! provider; port of `controllers/mock.py`. Generic over any state
//! `S: HasSettings + HasRateLimiter` (see this package's own module
//! doc), not the concrete, Hero-bearing `AppState`.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, State};
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::generic::controllers::{HasRateLimiter, HasSettings};
use crate::oidc::MOCK_SIGNING_KEY;
use crate::problem_details::AppError;

/// Rate-limit scope key (`src/rate_limit.rs`) for this route.
const MOCK_TOKEN_RATE_SCOPE: &str = "mock-token";

#[derive(Deserialize)]
struct MockTokenRequest {
    sub: String,
    #[serde(default)]
    roles: Vec<String>,
}

#[derive(Serialize)]
struct MockTokenResponse {
    access_token: String,
}

async fn mint_token<S>(
    State(state): State<S>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(payload): Json<MockTokenRequest>,
) -> Result<Json<MockTokenResponse>, AppError>
where
    S: HasSettings + HasRateLimiter,
{
    state
        .rate_limiter()
        .check(
            MOCK_TOKEN_RATE_SCOPE,
            addr.ip(),
            state.settings().rate_limit_mock_token_per_minute,
            60,
        )
        .await?;
    let mut resource_access = serde_json::Map::new();
    resource_access.insert(
        state.settings().oidc_client_id.clone(),
        serde_json::json!({ "roles": payload.roles }),
    );
    let claims = serde_json::json!({
        "sub": payload.sub,
        "preferred_username": payload.sub,
        "resource_access": resource_access,
    });
    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(MOCK_SIGNING_KEY),
    )
    .map_err(|err| AppError::Internal(err.to_string()))?;

    Ok(Json(MockTokenResponse {
        access_token: token,
    }))
}

pub fn router<S>() -> Router<S>
where
    S: HasSettings + HasRateLimiter + Clone + Send + Sync + 'static,
{
    Router::new().route("/token", post(mint_token::<S>))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Mode, Settings};
    use crate::oidc::OidcVerifier;
    use crate::rate_limit::RateLimiter;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use std::sync::Arc;
    use tower::ServiceExt;

    // A minimal fake implementing only `HasSettings`/`HasRateLimiter`,
    // not the concrete Hero-bearing `AppState` -- keeps this generic
    // package's tests from depending on `crate::hero` at all.
    #[derive(Clone)]
    struct FakeState {
        settings: Arc<Settings>,
        rate_limiter: Arc<RateLimiter>,
    }

    impl HasSettings for FakeState {
        fn settings(&self) -> &Settings {
            &self.settings
        }
    }

    impl HasRateLimiter for FakeState {
        fn rate_limiter(&self) -> &RateLimiter {
            &self.rate_limiter
        }
    }

    fn mock_defaults() -> Settings {
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
            oidc_issuer_url: "http://localhost:8080".to_string(),
            oidc_authorization_url: "http://localhost:8080/auth".to_string(),
            oidc_token_url: "http://localhost:8080/token".to_string(),
            oidc_client_id: "api".to_string(),
            oidc_audience: None,
        }
    }

    fn app_with(settings: Settings) -> Router {
        let state = FakeState {
            settings: Arc::new(settings),
            rate_limiter: Arc::new(RateLimiter::mock()),
        };
        router().with_state(state)
    }

    fn app() -> Router {
        app_with(mock_defaults())
    }

    /// `mint_token` extracts `ConnectInfo<SocketAddr>` for rate limiting --
    /// `oneshot` bypasses the real `into_make_service_with_connect_info`
    /// main.rs wires up, so tests insert the same extension by hand.
    fn with_connect_info(mut request: Request<Body>) -> Request<Body> {
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
        request
    }

    #[tokio::test]
    async fn mint_token_produces_a_token_whose_claims_round_trip_through_the_verifier() {
        let response = app()
            .oneshot(with_connect_info(
                Request::builder()
                    .method("POST")
                    .uri("/token")
                    .header("Content-Type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"sub": "alice", "roles": ["editor", "maintainer"]})
                            .to_string(),
                    ))
                    .unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let token = body["access_token"].as_str().unwrap();

        let settings = Arc::new(Settings {
            mode: Mode::Mock,
            oidc_client_id: "api".to_string(),
            ..mock_defaults()
        });
        let verifier = OidcVerifier::new(settings);
        let claims = verifier.decode_bearer_token(token).await.unwrap();
        assert_eq!(claims.subject().unwrap(), "alice");
        assert!(claims.require_any_role("api", &["editor"]).is_ok());
        assert!(claims.require_any_role("api", &["maintainer"]).is_ok());
        assert!(claims.require_any_role("api", &["viewer"]).is_err());
    }

    #[tokio::test]
    async fn mint_token_defaults_to_an_empty_role_list() {
        let response = app()
            .oneshot(with_connect_info(
                Request::builder()
                    .method("POST")
                    .uri("/token")
                    .header("Content-Type", "application/json")
                    .body(Body::from(serde_json::json!({"sub": "bob"}).to_string()))
                    .unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn mint_token_returns_429_once_the_per_caller_limit_is_exceeded() {
        let shared_app = app_with(Settings {
            rate_limit_mock_token_per_minute: 1,
            ..mock_defaults()
        });

        let request = || {
            with_connect_info(
                Request::builder()
                    .method("POST")
                    .uri("/token")
                    .header("Content-Type", "application/json")
                    .body(Body::from(serde_json::json!({"sub": "bob"}).to_string()))
                    .unwrap(),
            )
        };

        let first = shared_app.clone().oneshot(request()).await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let second = shared_app.oneshot(request()).await.unwrap();
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    }
}
