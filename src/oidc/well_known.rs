//! Unauthenticated `/.well-known/` documents this service publishes about
//! itself, and the caller resolver the CRUD manifest uses (`docs/adrs/0030`,
//! FR-0036) -- port of template-fastapi's `well_known.py`.
//!
//! - `oauth-protected-resource` (RFC 9728): tells an OAuth client which
//!   authorization server issues tokens for this API, how to present them
//!   and which scopes exist. Always served. The same URL is what
//!   `problem_details::add_resource_metadata` puts in a 401's
//!   `WWW-Authenticate`.
//! - `security.txt` (RFC 9116): served only when `SECURITY_TXT_CONTACT` is
//!   set, since an instance must choose its own reporting channel.
//!   `config::SecurityTxt` validates the fields (a contact needs an
//!   `Expires` within a year) before this module ever runs.
//!
//! The RFC 9727 `api-catalog` is `craftainer_crud::manifest`'s, mounted in
//! `lib.rs`. These routes live in `oidc/` (not a module of their own)
//! because `.github/scripts/check_layering.py` knows no other home for a
//! module that depends on `config` alone.

use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::State;
use axum::http::header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use craftainer_crud::error::CrudError;
use craftainer_crud::manifest::{Caller, CallerResolver};
use serde_json::{json, Value};

use super::OidcVerifier;
use crate::config::ResourceServer;
use crate::problem_details::AppError;

const CACHE: &str = "public, max-age=3600";

/// The RFC 9728 document, built from configuration alone.
pub fn protected_resource_metadata(resource: &ResourceServer) -> Value {
    json!({
        "resource": resource.public_base_url,
        "authorization_servers": [resource.issuer],
        "scopes_supported": resource.scopes_supported,
        "bearer_methods_supported": ["header"],
        "resource_name": resource.name,
    })
}

async fn oauth_protected_resource(State(resource): State<Arc<ResourceServer>>) -> Response {
    (
        [(CACHE_CONTROL, HeaderValue::from_static(CACHE))],
        Json(protected_resource_metadata(&resource)),
    )
        .into_response()
}

async fn security_txt(State(resource): State<Arc<ResourceServer>>) -> Response {
    // Only mounted when configured (see `router`).
    let body = resource
        .security_txt
        .as_ref()
        .map(crate::config::SecurityTxt::render)
        .unwrap_or_default();
    (
        [
            (CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8")),
            (CACHE_CONTROL, HeaderValue::from_static(CACHE)),
        ],
        body,
    )
        .into_response()
}

/// Router for the documents above; the paths are absolute, so merge it at
/// the root.
pub fn router<S>(resource: Arc<ResourceServer>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let mut router = Router::new().route(
        "/.well-known/oauth-protected-resource",
        get(oauth_protected_resource),
    );
    if resource.security_txt.is_some() {
        router = router.route("/.well-known/security.txt", get(security_txt));
    }
    router.with_state(resource)
}

/// Who is calling the CRUD manifest: the bearer token's roles under the
/// configured client, or `Caller::Anonymous` for a request without a
/// valid token (the manifest then lists public resources only). A token
/// check that fails because the identity provider is down is an error of
/// its own, not an anonymous caller.
pub struct ManifestCallers(pub Arc<OidcVerifier>);

#[async_trait]
impl CallerResolver for ManifestCallers {
    async fn resolve(&self, headers: &HeaderMap) -> Result<Caller, CrudError> {
        let token = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "));
        let Some(token) = token else {
            return Ok(Caller::Anonymous);
        };
        match self.0.decode_bearer_token(token).await {
            Ok(claims) => Ok(Caller::Authenticated {
                roles: claims.granted_roles(&self.0.settings.oidc_client_id),
            }),
            Err(AppError::ServiceUnavailable(msg)) => Err(CrudError::ServiceUnavailable(msg)),
            Err(_) => Ok(Caller::Anonymous),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Mode, Settings};
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn settings() -> Settings {
        Settings {
            app_name: "template-axum".to_string(),
            mode: Mode::Mock,
            allow_mock_mode: true,
            postgres_user: "app".to_string(),
            postgres_password: "app".to_string(),
            postgres_db: "app".to_string(),
            postgres_host: "localhost".to_string(),
            postgres_port: 5432,
            db: Default::default(),
            s3_endpoint_url: "http://localhost:9000".to_string(),
            s3_access_key: "rustfsadmin".to_string(),
            s3_secret_key: "rustfsadmin".to_string(),
            redis_url: "redis://localhost:6379/0".to_string(),
            mqtt_host: "localhost".to_string(),
            mqtt_port: 1883,
            rate_limit_mock_token_per_minute: 10,
            rate_limit_hero_write_per_minute: 20,
            bulk_action_max_matched: 1000,
            oidc_issuer_url: "http://localhost:8080/realms/template-axum".to_string(),
            oidc_authorization_url: "http://localhost:8080/auth".to_string(),
            oidc_token_url: "http://localhost:8080/token".to_string(),
            oidc_client_id: "api".to_string(),
            oidc_audience: None,
        }
    }

    async fn get_path(router: &Router, path: &str) -> Response {
        router
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn body_text(response: Response) -> String {
        String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn serves_the_rfc_9728_document_built_from_configuration() {
        let mut resource = ResourceServer::from_settings(&settings());
        resource.public_base_url = "https://api.example.com".to_string();
        resource.scopes_supported = vec!["openid".to_string(), "profile".to_string()];
        let app: Router = router(Arc::new(resource));

        let response = get_path(&app, "/.well-known/oauth-protected-resource").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers().get(CACHE_CONTROL).unwrap(), CACHE);
        assert_eq!(
            response.headers().get(CONTENT_TYPE).unwrap(),
            "application/json"
        );
        let doc: Value = serde_json::from_str(&body_text(response).await).unwrap();
        assert_eq!(
            doc,
            json!({
                "resource": "https://api.example.com",
                "authorization_servers": ["http://localhost:8080/realms/template-axum"],
                "scopes_supported": ["openid", "profile"],
                "bearer_methods_supported": ["header"],
                "resource_name": "template-axum",
            })
        );
    }

    #[tokio::test]
    async fn security_txt_is_a_404_until_a_contact_is_configured() {
        let app: Router = router(Arc::new(ResourceServer::from_settings(&settings())));
        let response = get_path(&app, "/.well-known/security.txt").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn serves_security_txt_once_configured() {
        let mut resource = ResourceServer::from_settings(&settings());
        resource.security_txt = Some(crate::config::SecurityTxt {
            contact: "mailto:security@example.com".to_string(),
            expires: chrono::Utc::now() + chrono::Duration::days(30),
            policy: None,
            preferred_languages: Some("en".to_string()),
            canonical: None,
        });
        let app: Router = router(Arc::new(resource));
        let response = get_path(&app, "/.well-known/security.txt").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(CONTENT_TYPE).unwrap(),
            "text/plain; charset=utf-8"
        );
        assert_eq!(response.headers().get(CACHE_CONTROL).unwrap(), CACHE);
        let body = body_text(response).await;
        assert!(body.starts_with("Contact: mailto:security@example.com\nExpires: "));
        assert!(body.ends_with("Preferred-Languages: en\n"));
    }

    fn bearer(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        headers
    }

    #[tokio::test]
    async fn the_manifest_caller_is_anonymous_without_a_valid_token() {
        let callers = ManifestCallers(Arc::new(OidcVerifier::new(Arc::new(settings()))));
        assert_eq!(
            callers.resolve(&HeaderMap::new()).await.unwrap(),
            Caller::Anonymous
        );
        let mut basic = HeaderMap::new();
        basic.insert(AUTHORIZATION, HeaderValue::from_static("Basic dTpw"));
        assert_eq!(callers.resolve(&basic).await.unwrap(), Caller::Anonymous);
        // Mock mode accepts any token with a decodable payload, so a
        // malformed one is the rejected case here.
        assert_eq!(
            callers.resolve(&bearer("not-a-jwt")).await.unwrap(),
            Caller::Anonymous
        );
    }

    #[tokio::test]
    async fn the_manifest_caller_carries_the_tokens_roles_for_the_configured_client() {
        use base64::Engine;
        let callers = ManifestCallers(Arc::new(OidcVerifier::new(Arc::new(settings()))));
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            json!({"sub": "a", "resource_access": {"api": {"roles": ["viewer"]}}}).to_string(),
        );
        let caller = callers
            .resolve(&bearer(&format!("h.{payload}.s")))
            .await
            .unwrap();
        assert_eq!(
            caller,
            Caller::Authenticated {
                roles: vec!["viewer".to_string()]
            }
        );
    }
}
