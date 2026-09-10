//! `oidc::OidcVerifier`'s real (non-`Mode::Mock`) verification path --
//! signature/issuer/audience/expiry against the devcontainer stack's own
//! live Keycloak, including the discovery+JWKS fetch and its cache.
//! `Mode::Mock`'s unsigned-token path has its own unit coverage in
//! `src/oidc/mod.rs`; only the real-provider path needs a live service
//! (see `tests/README.md`'s "All four tiers are built" on why real
//! Keycloak tokens only enter the integration/e2e tiers, never `src/`
//! unit tests).

use std::sync::Arc;

use template_axum::config::{Mode, Settings};
use template_axum::oidc::OidcVerifier;
use template_axum::problem_details::AppError;

fn oidc_issuer_url() -> String {
    std::env::var("OIDC_ISSUER_URL")
        .unwrap_or_else(|_| "http://keycloak:8080/realms/template-axum".to_string())
}

fn oidc_token_url() -> String {
    std::env::var("OIDC_TOKEN_URL")
        .unwrap_or_else(|_| format!("{}/protocol/openid-connect/token", oidc_issuer_url()))
}

fn dev_settings(oidc_audience: Option<String>) -> Arc<Settings> {
    Arc::new(Settings {
        app_name: "template-axum".to_string(),
        mode: Mode::Dev,
        allow_mock_mode: false,
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
        rate_limit_mock_token_per_minute: 10_000,
        rate_limit_hero_write_per_minute: 10_000,
        bulk_action_max_matched: 1000,
        oidc_issuer_url: oidc_issuer_url(),
        oidc_authorization_url: format!("{}/protocol/openid-connect/auth", oidc_issuer_url()),
        oidc_token_url: oidc_token_url(),
        oidc_client_id: "api".to_string(),
        oidc_audience,
    })
}

/// Real Keycloak Resource Owner Password Credentials grant -- the `api`
/// client is public with `directAccessGrantsEnabled` (`realm-export.json`),
/// and every test user's password equals its username
/// (`.devcontainer/stack/keycloak/README.md`).
async fn keycloak_token(client: &reqwest::Client, username: &str) -> String {
    let response = client
        .post(oidc_token_url())
        .form(&[
            ("grant_type", "password"),
            ("client_id", "api"),
            ("username", username),
            ("password", username),
        ])
        .send()
        .await
        .expect("Keycloak token request failed -- is the devcontainer stack running?");
    assert_eq!(
        response.status(),
        200,
        "Keycloak should issue a token for test user {username}"
    );
    let body: serde_json::Value = response.json().await.unwrap();
    body["access_token"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn decodes_and_caches_a_real_keycloak_token() {
    let http = reqwest::Client::new();
    let token = keycloak_token(&http, "viewer").await;
    let verifier = OidcVerifier::new(dev_settings(None));

    // First call fetches discovery + JWKS; second reuses the cache
    // (jwks()'s `fetched_at.elapsed() < JWKS_CACHE_TTL` branch).
    let claims = verifier
        .decode_bearer_token(&token)
        .await
        .expect("a real Keycloak token must verify");
    let subject = claims.subject().unwrap().to_string();
    assert!(!subject.is_empty());

    let claims_again = verifier
        .decode_bearer_token(&token)
        .await
        .expect("the cached JWKS must still verify the same token");
    assert_eq!(claims_again.subject().unwrap(), subject);
}

#[tokio::test]
async fn rejects_a_malformed_token() {
    let verifier = OidcVerifier::new(dev_settings(None));
    let err = verifier
        .decode_bearer_token("not-a-jwt")
        .await
        .expect_err("a malformed token must be rejected");
    assert!(matches!(err, AppError::Unauthorized(_)));
}

#[tokio::test]
async fn rejects_a_token_with_an_unrecognized_kid() {
    // A well-formed but unsigned/foreign JWT: valid base64url JSON header
    // and payload, so `decode_header` succeeds but no cached JWK matches
    // its `kid`.
    let header = base64_url(br#"{"alg":"RS256","kid":"not-a-real-kid","typ":"JWT"}"#);
    let payload = base64_url(br#"{"sub":"nobody"}"#);
    let token = format!("{header}.{payload}.sig");

    let verifier = OidcVerifier::new(dev_settings(None));
    let err = verifier
        .decode_bearer_token(&token)
        .await
        .expect_err("an unrecognized kid must be rejected");
    assert!(matches!(err, AppError::Unauthorized(_)));
}

#[tokio::test]
async fn rejects_a_real_token_against_the_wrong_audience() {
    let http = reqwest::Client::new();
    let token = keycloak_token(&http, "viewer").await;
    let verifier = OidcVerifier::new(dev_settings(Some("not-the-real-audience".to_string())));
    let err = verifier
        .decode_bearer_token(&token)
        .await
        .expect_err("a token for a different audience must be rejected");
    assert!(matches!(err, AppError::Unauthorized(_)));
}

fn base64_url(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[tokio::test]
async fn reports_service_unavailable_when_discovery_is_unreachable() {
    let mut settings = (*dev_settings(None)).clone();
    settings.oidc_issuer_url = "http://127.0.0.1:1/realms/nowhere".to_string();
    let verifier = OidcVerifier::new(Arc::new(settings));

    // Well-formed enough to pass `decode_header` (so the failure comes
    // from the discovery fetch, not an earlier parse error).
    let header = base64_url(br#"{"alg":"RS256","kid":"whatever","typ":"JWT"}"#);
    let payload = base64_url(br#"{"sub":"nobody"}"#);
    let token = format!("{header}.{payload}.sig");

    let err = verifier
        .decode_bearer_token(&token)
        .await
        .expect_err("an unreachable issuer must fail closed");
    assert!(
        matches!(err, AppError::ServiceUnavailable(_)),
        "expected ServiceUnavailable, got {err:?}"
    );
}

#[tokio::test]
async fn the_auth_claims_extractor_rejects_a_bad_bearer_token_over_http() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    let settings = dev_settings(None);
    let state = template_axum::controllers::AppState {
        oidc: Arc::new(OidcVerifier::new(settings.clone())),
        settings,
        health_registry: Arc::new(template_axum::health::HealthRegistry::new()),
        hero_crud: Arc::new(template_axum::crud::CrudService::new(
            template_axum::controllers::DynHeroRepository(Box::new(
                template_axum::repositories::hero_memory::HeroMemoryRepository::new(),
            )),
        )),
        rate_limiter: Arc::new(template_axum::rate_limit::RateLimiter::mock()),
        events: Arc::new(template_axum::events::EventBus::mock()),
    };
    let app = template_axum::controllers::audit::router().with_state(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/")
                .header("Authorization", "Bearer not-a-real-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
