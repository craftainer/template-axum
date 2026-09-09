//! Provider-agnostic OIDC bearer-token validation (FR-0013) plus Keycloak
//! client-role RBAC (FR-0014/FR-0015) -- port of `oidc.py`. Discovery +
//! JWKS verification code itself assumes nothing beyond "any Authorization
//! Code + PKCE provider exposing `.well-known/openid-configuration`" (ADR
//! 0003); only `Claims::roles`' `resource_access.<client>.roles` lookup is
//! Keycloak-specific (`docs/nfrs/0014-oidc-claim-shape-portability.md`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::config::{Mode, Settings};
use crate::problem_details::AppError;

const JWKS_CACHE_TTL: Duration = Duration::from_secs(300);
/// Fixed HS256 key `POST /mock/token` signs with -- never verified against
/// (Mode::Mock trusts claims as-is), so it doesn't need to be a real
/// secret; matches `controllers/mock.py`'s equivalent constant.
pub const MOCK_SIGNING_KEY: &[u8] = b"mock-mode-signing-key-not-a-real-secret";

#[derive(Debug, Deserialize)]
struct OidcDiscoveryDocument {
    jwks_uri: String,
}

struct JwksCache {
    fetched_at: Instant,
    keys: jsonwebtoken::jwk::JwkSet,
}

/// Verifies bearer tokens against one OIDC issuer -- holds the cached JWKS
/// (`_get_jwks_client`'s Rust counterpart) and the settings needed to pick
/// mock vs. real verification.
pub struct OidcVerifier {
    settings: Arc<Settings>,
    http: reqwest::Client,
    jwks: RwLock<Option<JwksCache>>,
}

/// The decoded token's claims -- kept as a raw JSON object (like the
/// Python original's plain `dict`) since NFR-0014 forbids assuming any
/// shape beyond `sub`.
#[derive(Debug, Clone)]
pub struct Claims(pub Value);

impl Claims {
    pub fn subject(&self) -> Result<&str, AppError> {
        self.0
            .get("sub")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::Unauthorized("token has no sub claim".to_string()))
    }

    /// `resource_access.<client>.roles` -- Keycloak's client-role claim
    /// shape (FR-0014).
    fn granted_roles(&self, client_id: &str) -> Vec<String> {
        self.0
            .get("resource_access")
            .and_then(|ra| ra.get(client_id))
            .and_then(|client| client.get("roles"))
            .and_then(Value::as_array)
            .map(|roles| {
                roles
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Returns `Ok(())` if the token carries at least one of `roles`, else
    /// `403` (FR-0014/FR-0015).
    pub fn require_any_role(&self, client_id: &str, roles: &[&str]) -> Result<(), AppError> {
        let granted = self.granted_roles(client_id);
        if roles.iter().any(|role| granted.iter().any(|g| g == role)) {
            Ok(())
        } else {
            tracing::warn!(subject = ?self.subject().ok(), required = ?roles, "insufficient role");
            Err(AppError::Forbidden("Insufficient role".to_string()))
        }
    }
}

impl OidcVerifier {
    pub fn new(settings: Arc<Settings>) -> Self {
        Self {
            settings,
            http: reqwest::Client::new(),
            jwks: RwLock::new(None),
        }
    }

    async fn jwks(&self) -> Result<jsonwebtoken::jwk::JwkSet, AppError> {
        {
            let cache = self.jwks.read().await;
            if let Some(cache) = cache.as_ref() {
                if cache.fetched_at.elapsed() < JWKS_CACHE_TTL {
                    return Ok(cache.keys.clone());
                }
            }
        }

        let discovery_url = format!(
            "{}/.well-known/openid-configuration",
            self.settings.oidc_issuer_url.trim_end_matches('/')
        );
        let unavailable =
            || AppError::ServiceUnavailable("Authentication service unavailable".to_string());
        let discovery: OidcDiscoveryDocument = self
            .http
            .get(&discovery_url)
            .send()
            .await
            .map_err(|_| unavailable())?
            .error_for_status()
            .map_err(|_| unavailable())?
            .json()
            .await
            .map_err(|_| unavailable())?;
        let keys: jsonwebtoken::jwk::JwkSet = self
            .http
            .get(&discovery.jwks_uri)
            .send()
            .await
            .map_err(|_| unavailable())?
            .error_for_status()
            .map_err(|_| unavailable())?
            .json()
            .await
            .map_err(|_| unavailable())?;

        *self.jwks.write().await = Some(JwksCache {
            fetched_at: Instant::now(),
            keys: keys.clone(),
        });
        Ok(keys)
    }

    /// Decode + verify a bearer token. `Mode::Mock` trusts the claims
    /// as-is with no signature check (FR-0017); otherwise verifies
    /// signature/issuer/audience(if configured)/expiry against the cached
    /// JWKS (FR-0013).
    pub async fn decode_bearer_token(&self, token: &str) -> Result<Claims, AppError> {
        let invalid = || AppError::Unauthorized("Invalid or expired token".to_string());

        if self.settings.mode == Mode::Mock {
            return decode_without_verification(token)
                .map(Claims)
                .map_err(|_| invalid());
        }

        let header = jsonwebtoken::decode_header(token).map_err(|_| invalid())?;
        let kid = header.kid.as_deref().ok_or_else(invalid)?;
        let jwks = self.jwks().await?;
        let jwk = jwks.find(kid).ok_or_else(invalid)?;
        let decoding_key = jsonwebtoken::DecodingKey::from_jwk(jwk).map_err(|_| invalid())?;

        let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
        validation.set_issuer(&[self.settings.oidc_issuer_url.as_str()]);
        if let Some(audience) = &self.settings.oidc_audience {
            validation.set_audience(&[audience.as_str()]);
        } else {
            validation.validate_aud = false;
        }

        let data = jsonwebtoken::decode::<Value>(token, &decoding_key, &validation)
            .map_err(|_| invalid())?;
        Ok(Claims(data.claims))
    }
}

fn decode_without_verification(token: &str) -> Result<Value, ()> {
    use base64::Engine;
    let payload = token.split('.').nth(1).ok_or(())?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| ())?;
    serde_json::from_slice(&bytes).map_err(|_| ())
}

/// Shared state every OIDC-aware extractor/handler needs.
pub trait HasOidcVerifier {
    fn oidc_verifier(&self) -> &OidcVerifier;
}

/// Extracts+verifies the bearer token on any route that declares this as a
/// handler parameter -- a route with no `Claims`/`AuthClaims` parameter
/// stays public (`docs/nfrs/0012-routes-public-by-default.md`), matching
/// `Depends(get_current_claims)`'s opt-in shape.
#[derive(Debug)]
pub struct AuthClaims(pub Claims);

impl<S> FromRequestParts<S> for AuthClaims
where
    S: HasOidcVerifier + Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| unauthorized(&parts.uri))?;
        let token = header
            .strip_prefix("Bearer ")
            .ok_or_else(|| unauthorized(&parts.uri))?;

        match state.oidc_verifier().decode_bearer_token(token).await {
            Ok(claims) => Ok(AuthClaims(claims)),
            Err(err) => {
                tracing::warn!(path = %parts.uri.path(), "rejected bearer token");
                Err(err)
            }
        }
    }
}

fn unauthorized(uri: &axum::http::Uri) -> AppError {
    tracing::warn!(path = %uri.path(), "missing bearer token");
    AppError::Unauthorized("Invalid or expired token".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;

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

    fn claims_with_roles(client_id: &str, roles: &[&str]) -> Claims {
        Claims(serde_json::json!({
            "sub": "alice",
            "resource_access": {
                client_id: { "roles": roles }
            }
        }))
    }

    // -- Claims::subject --

    #[test]
    fn subject_returns_the_sub_claim() {
        let claims = claims_with_roles("api", &[]);
        assert_eq!(claims.subject().unwrap(), "alice");
    }

    #[test]
    fn subject_errors_when_sub_is_missing() {
        let claims = Claims(serde_json::json!({}));
        assert!(claims.subject().is_err());
    }

    // -- Claims::require_any_role -- the RBAC matrix (FR-0014/FR-0015).

    #[test]
    fn require_any_role_allows_a_granted_role() {
        let claims = claims_with_roles("api", &["editor"]);
        assert!(claims
            .require_any_role("api", &["editor", "maintainer"])
            .is_ok());
    }

    #[test]
    fn require_any_role_rejects_when_no_role_matches() {
        let claims = claims_with_roles("api", &["viewer"]);
        let err = claims
            .require_any_role("api", &["editor", "maintainer"])
            .unwrap_err();
        assert!(matches!(err, AppError::Forbidden(_)));
    }

    #[test]
    fn require_any_role_rejects_when_no_roles_granted_at_all() {
        let claims = Claims(serde_json::json!({"sub": "alice"}));
        assert!(claims.require_any_role("api", &["viewer"]).is_err());
    }

    #[test]
    fn require_any_role_is_scoped_to_the_configured_client_id() {
        // A role granted under a *different* client's resource_access entry
        // must not satisfy this app's own role check.
        let claims = claims_with_roles("some-other-client", &["maintainer"]);
        assert!(claims.require_any_role("api", &["maintainer"]).is_err());
    }

    #[test]
    fn hero_read_role_matrix_matches_fr_0015() {
        // viewer/editor/maintainer/detective can read; nothing else can.
        for role in ["viewer", "editor", "maintainer", "detective"] {
            let claims = claims_with_roles("api", &[role]);
            assert!(
                claims
                    .require_any_role("api", &["viewer", "editor", "maintainer", "detective"])
                    .is_ok(),
                "{role} should have hero read access"
            );
        }
        let claims = claims_with_roles("api", &["security"]);
        assert!(claims
            .require_any_role("api", &["viewer", "editor", "maintainer", "detective"])
            .is_err());
    }

    #[test]
    fn hero_write_role_matrix_excludes_viewer_and_detective() {
        for role in ["editor", "maintainer"] {
            let claims = claims_with_roles("api", &[role]);
            assert!(claims
                .require_any_role("api", &["editor", "maintainer"])
                .is_ok());
        }
        for role in ["viewer", "detective", "security"] {
            let claims = claims_with_roles("api", &[role]);
            assert!(claims
                .require_any_role("api", &["editor", "maintainer"])
                .is_err());
        }
    }

    #[test]
    fn hero_delete_role_matrix_is_maintainer_only() {
        let claims = claims_with_roles("api", &["maintainer"]);
        assert!(claims.require_any_role("api", &["maintainer"]).is_ok());
        for role in ["editor", "viewer", "detective"] {
            let claims = claims_with_roles("api", &[role]);
            assert!(claims.require_any_role("api", &["maintainer"]).is_err());
        }
    }

    // -- decode_without_verification (Mode::Mock path) --

    #[test]
    fn decode_without_verification_reads_the_payload_segment() {
        use base64::Engine;
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::json!({"sub": "bob"}).to_string());
        let token = format!("header.{payload}.signature");
        let claims = decode_without_verification(&token).unwrap();
        assert_eq!(claims["sub"], "bob");
    }

    #[test]
    fn decode_without_verification_rejects_a_token_with_no_payload_segment() {
        assert!(decode_without_verification("just-one-segment").is_err());
    }

    #[test]
    fn decode_without_verification_rejects_invalid_base64() {
        assert!(decode_without_verification("header.not-valid-base64!!!.sig").is_err());
    }

    #[test]
    fn decode_without_verification_rejects_non_json_payload() {
        use base64::Engine;
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("not json");
        let token = format!("header.{payload}.signature");
        assert!(decode_without_verification(&token).is_err());
    }

    // -- OidcVerifier::decode_bearer_token under Mode::Mock (FR-0017): no
    // network reachable in a unit test, so only the mock path is exercised
    // here -- the real-JWKS path is covered against a live Keycloak in
    // tests/integration (see tests/integration/README.md).

    #[tokio::test]
    async fn mock_mode_trusts_an_unsigned_tokens_claims() {
        let verifier = OidcVerifier::new(Arc::new(mock_settings()));
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
            &serde_json::json!({"sub": "carol", "resource_access": {"api": {"roles": ["editor"]}}}),
            &jsonwebtoken::EncodingKey::from_secret(b"any-key-mock-mode-never-checks-this"),
        )
        .unwrap();
        let claims = verifier.decode_bearer_token(&token).await.unwrap();
        assert_eq!(claims.subject().unwrap(), "carol");
        assert!(claims.require_any_role("api", &["editor"]).is_ok());
    }

    #[tokio::test]
    async fn mock_mode_rejects_a_malformed_token() {
        let verifier = OidcVerifier::new(Arc::new(mock_settings()));
        let err = verifier.decode_bearer_token("not-a-jwt").await.unwrap_err();
        assert!(matches!(err, AppError::Unauthorized(_)));
    }

    // -- AuthClaims extractor --

    struct FakeState(OidcVerifier);
    impl HasOidcVerifier for FakeState {
        fn oidc_verifier(&self) -> &OidcVerifier {
            &self.0
        }
    }

    #[tokio::test]
    async fn auth_claims_rejects_a_request_with_no_authorization_header() {
        let state = FakeState(OidcVerifier::new(Arc::new(mock_settings())));
        let request = Request::builder().body(()).unwrap();
        let (mut parts, ()) = request.into_parts();
        let err = AuthClaims::from_request_parts(&mut parts, &state)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Unauthorized(_)));
    }

    #[tokio::test]
    async fn auth_claims_rejects_a_non_bearer_authorization_header() {
        let state = FakeState(OidcVerifier::new(Arc::new(mock_settings())));
        let request = Request::builder()
            .header("Authorization", "Basic dXNlcjpwYXNz")
            .body(())
            .unwrap();
        let (mut parts, ()) = request.into_parts();
        let err = AuthClaims::from_request_parts(&mut parts, &state)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Unauthorized(_)));
    }

    #[tokio::test]
    async fn auth_claims_accepts_a_valid_mock_mode_bearer_token() {
        let state = FakeState(OidcVerifier::new(Arc::new(mock_settings())));
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
            &serde_json::json!({"sub": "dave"}),
            &jsonwebtoken::EncodingKey::from_secret(b"anything"),
        )
        .unwrap();
        let request = Request::builder()
            .header("Authorization", format!("Bearer {token}"))
            .body(())
            .unwrap();
        let (mut parts, ()) = request.into_parts();
        let AuthClaims(claims) = AuthClaims::from_request_parts(&mut parts, &state)
            .await
            .unwrap();
        assert_eq!(claims.subject().unwrap(), "dave");
    }
}
