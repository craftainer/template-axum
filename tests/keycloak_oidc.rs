//! `oidc::OidcVerifier`'s real (non-`Mode::Mock`) verification path --
//! signature/issuer/audience/expiry against the devcontainer stack's own
//! live Keycloak, including the discovery+JWKS fetch and its cache.
//! `Mode::Mock`'s unsigned-token path has its own unit coverage in
//! `src/oidc/mod.rs`; only the real-provider path needs a live service
//! (see `tests/README.md`'s "All four tiers are built" on why real
//! Keycloak tokens only enter the integration/e2e tiers, never `src/`
//! unit tests).

use std::sync::Arc;

use template_axum::health::checks::OidcHealthCheck;
use template_axum::health::HealthCheck;
use template_axum::oidc::OidcVerifier;
use template_axum::problem_details::AppError;

mod common;
use common::{dev_settings, keycloak_token, oidc_issuer_url};

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
    let state = template_axum::hero::controllers::AppState {
        oidc: Arc::new(OidcVerifier::new(settings.clone())),
        settings,
        health_registry: Arc::new(template_axum::health::HealthRegistry::new()),
        hero_crud: Arc::new(template_axum::crud::CrudService::new(
            template_axum::hero::controllers::DynHeroRepository(Box::new(
                template_axum::hero::repositories::hero_memory::HeroMemoryRepository::new(),
            )),
        )),
        rate_limiter: Arc::new(template_axum::rate_limit::RateLimiter::mock()),
        events: Arc::new(template_axum::events::EventBus::mock()),
    };
    let app = template_axum::generic::controllers::audit::router().with_state(state);

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

#[tokio::test]
async fn oidc_health_check_reports_healthy_against_a_real_keycloak() {
    let check = OidcHealthCheck::new(oidc_issuer_url());
    assert_eq!(check.name(), "oidc");
    let result = check.check().await;
    assert!(result.healthy, "{:?}", result.detail);
    assert!(result.detail.is_none());
}
