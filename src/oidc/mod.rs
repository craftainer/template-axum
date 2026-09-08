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
