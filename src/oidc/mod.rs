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
use tokio::sync::{Mutex, RwLock};

use crate::config::{Mode, ResourceServer, Settings};
use crate::problem_details::AppError;

pub mod well_known;

const JWKS_CACHE_TTL: Duration = Duration::from_secs(300);
/// The shortest gap between two attempts to refetch the key set that a
/// token asked for (an unknown `kid`, or an expired cache). A signing key
/// rotation is picked up within this long, and a flood of tokens carrying
/// made-up `kid`s costs the identity provider at most one fetch per gap
/// (OWASP A07/A10; the cache cannot be used to amplify requests).
const JWKS_REFETCH_COOLDOWN: Duration = Duration::from_secs(30);
/// How long a cached key set may keep verifying tokens after it expired
/// while the provider cannot be reached. Past this, verification fails as
/// `503` rather than trusting keys the provider may have revoked.
const JWKS_MAX_STALE: Duration = Duration::from_secs(3600);
/// How long one discovery or JWKS request may take in all (connect, send,
/// read the body). Without a bound, an identity provider that accepts the
/// connection and never answers ties up every authenticated request behind
/// its own unbounded fetch (OWASP A10, mishandling exceptional conditions);
/// with it they fail as `503` after this long.
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);
/// How long connecting to the identity provider may take.
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Fixed HS256 key `POST /mock/token` signs with -- never verified against
/// (Mode::Mock trusts claims as-is), so it doesn't need to be a real
/// secret; matches `controllers/mock.py`'s equivalent constant.
pub const MOCK_SIGNING_KEY: &[u8] = b"mock-mode-signing-key-not-a-real-secret";

#[derive(Debug, Deserialize)]
struct OidcDiscoveryDocument {
    jwks_uri: String,
}

/// When a key set was fetched, and the set.
type CachedKeys = (Instant, Arc<jsonwebtoken::jwk::JwkSet>);

struct JwksCache {
    fetched_at: Instant,
    keys: Arc<jsonwebtoken::jwk::JwkSet>,
}

/// Verifies bearer tokens against one OIDC issuer -- holds the cached JWKS
/// (`_get_jwks_client`'s Rust counterpart) and the settings needed to pick
/// mock vs. real verification.
pub struct OidcVerifier {
    settings: Arc<Settings>,
    resource: Arc<ResourceServer>,
    http: reqwest::Client,
    jwks: RwLock<Option<JwksCache>>,
    /// Serialises refetches (single-flight): held for the whole fetch, and
    /// holding when the last attempt finished, successful or not.
    refetch: Mutex<Option<Instant>>,
    refetch_cooldown: Duration,
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
    /// shape (FR-0014). `pub`: `controllers::audit` (FR-0033) reports the
    /// caller's own granted roles back to them, rather than only using
    /// them for an allow/deny decision.
    pub fn granted_roles(&self, client_id: &str) -> Vec<String> {
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

/// The client for discovery and JWKS fetches: bounded in time and following
/// no redirects (the provider's endpoints are fixed configuration; a
/// redirect would let a compromised or misconfigured endpoint send the
/// fetch, and the keys it trusts, elsewhere).
fn http_client(timeout: Duration, connect_timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(connect_timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("the OIDC HTTP client builds with a TLS backend present")
}

impl OidcVerifier {
    /// A verifier for the issuer, audience and discovery URL `settings`
    /// alone implies (`ResourceServer::from_settings`); the wiring uses
    /// `with_resource_server` with what the environment says instead.
    pub fn new(settings: Arc<Settings>) -> Self {
        let resource = Arc::new(ResourceServer::from_settings(&settings));
        Self::with_resource_server(settings, resource)
    }

    pub fn with_resource_server(settings: Arc<Settings>, resource: Arc<ResourceServer>) -> Self {
        Self {
            settings,
            resource,
            http: http_client(HTTP_TIMEOUT, HTTP_CONNECT_TIMEOUT),
            jwks: RwLock::new(None),
            refetch: Mutex::new(None),
            refetch_cooldown: JWKS_REFETCH_COOLDOWN,
        }
    }

    /// The issuer/audience/public-URL configuration this verifier checks
    /// against, shared with the `/.well-known/` documents.
    pub fn resource_server(&self) -> &Arc<ResourceServer> {
        &self.resource
    }

    /// The cached key set, as of now.
    async fn cached_jwks(&self) -> Option<CachedKeys> {
        let cache = self.jwks.read().await;
        cache
            .as_ref()
            .map(|cache| (cache.fetched_at, Arc::clone(&cache.keys)))
    }

    /// The key set to look `kid` up in.
    ///
    /// A fresh cache that holds `kid` answers at once. Otherwise (cold
    /// start, expired cache, or a `kid` the cache does not know, which is
    /// what a key rotation looks like) the key set is refetched, with:
    ///
    /// - single flight: one fetch at a time, and a request that queued
    ///   behind it uses its outcome instead of fetching again;
    /// - a cooldown: no new fetch within `refetch_cooldown` of the last
    ///   attempt, so attacker-chosen `kid`s cannot make this service hammer
    ///   the identity provider (the cold start is exempt: with nothing
    ///   cached there is nothing to fall back on);
    /// - stale-if-error: when no fetch happens or it fails, the cached
    ///   keys keep serving for up to [`JWKS_MAX_STALE`]; an unknown `kid`
    ///   then simply is not found (`401`), and with nothing usable cached
    ///   the answer is `503`.
    async fn jwks_for(&self, kid: &str) -> Result<Arc<jsonwebtoken::jwk::JwkSet>, AppError> {
        let entered = Instant::now();
        let cold = match self.cached_jwks().await {
            Some((fetched_at, keys)) if fetched_at.elapsed() < JWKS_CACHE_TTL => {
                if keys.find(kid).is_some() {
                    return Ok(keys);
                }
                false
            }
            Some(_) => false,
            None => true,
        };

        let mut last_attempt = self.refetch.lock().await;
        let latest = self.cached_jwks().await;
        let usable = |latest: Option<CachedKeys>| match latest {
            Some((fetched_at, keys)) if fetched_at.elapsed() < JWKS_MAX_STALE => Ok(keys),
            _ => Err(AppError::ServiceUnavailable(
                "Authentication service unavailable".to_string(),
            )),
        };

        // An attempt finished while this request waited for the lock.
        if last_attempt.is_some_and(|at| at >= entered) {
            return usable(latest);
        }
        if !cold && last_attempt.is_some_and(|at| at.elapsed() < self.refetch_cooldown) {
            return usable(latest);
        }

        let fetched = self.fetch_jwks().await;
        *last_attempt = Some(Instant::now());
        match fetched {
            Ok(keys) => {
                let keys = Arc::new(keys);
                *self.jwks.write().await = Some(JwksCache {
                    fetched_at: Instant::now(),
                    keys: Arc::clone(&keys),
                });
                Ok(keys)
            }
            Err(err) => {
                tracing::warn!("JWKS refetch failed; keeping the cached keys");
                usable(latest).or(Err(err))
            }
        }
    }

    /// Fetch the discovery document, then the key set it points at.
    async fn fetch_jwks(&self) -> Result<jsonwebtoken::jwk::JwkSet, AppError> {
        let discovery_url = &self.resource.discovery_url;
        let unavailable =
            || AppError::ServiceUnavailable("Authentication service unavailable".to_string());
        let discovery: OidcDiscoveryDocument = self
            .http
            .get(discovery_url)
            .send()
            .await
            .map_err(|_| unavailable())?
            .error_for_status()
            .map_err(|_| unavailable())?
            .json()
            .await
            .map_err(|_| unavailable())?;
        self.http
            .get(&discovery.jwks_uri)
            .send()
            .await
            .map_err(|_| unavailable())?
            .error_for_status()
            .map_err(|_| unavailable())?
            .json()
            .await
            .map_err(|_| unavailable())
    }

    /// Decode + verify a bearer token. `Mode::Mock` trusts the claims
    /// as-is with no signature check (FR-0017); otherwise verifies
    /// signature, expiry, the exact issuer and the audience against the
    /// cached JWKS (FR-0013). The audience is always checked (RFC 9700
    /// section 2.3) and `exp`, `iss` and `aud` must be present.
    pub async fn decode_bearer_token(&self, token: &str) -> Result<Claims, AppError> {
        let invalid = || AppError::InvalidToken("Invalid or expired token".to_string());

        if self.settings.mode == Mode::Mock {
            return decode_without_verification(token)
                .map(Claims)
                .map_err(|_| invalid());
        }

        let header = jsonwebtoken::decode_header(token).map_err(|_| invalid())?;
        let kid = header.kid.as_deref().ok_or_else(invalid)?;
        let jwks = self.jwks_for(kid).await?;
        let jwk = jwks.find(kid).ok_or_else(invalid)?;
        let decoding_key = jsonwebtoken::DecodingKey::from_jwk(jwk).map_err(|_| invalid())?;

        let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
        validation.set_issuer(&[self.resource.issuer.as_str()]);
        validation.set_audience(&[self.resource.audience.as_str()]);
        validation.set_required_spec_claims(&["exp", "iss", "aud"]);
        // A token that is not valid yet is refused too (RFC 7519 section 4.1.5);
        // the default leeway absorbs clock skew.
        validation.validate_nbf = true;

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
        // The scheme is case-insensitive (RFC 9110 section 11.1, RFC 6750).
        let token = header
            .split_once(' ')
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("Bearer"))
            .map(|(_, token)| token)
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
    AppError::Unauthorized("Not authenticated".to_string())
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
        assert!(matches!(err, AppError::InvalidToken(_)));
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
    #[tokio::test]
    async fn auth_claims_accepts_the_bearer_scheme_in_any_case() {
        let state = FakeState(OidcVerifier::new(Arc::new(mock_settings())));
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
            &serde_json::json!({"sub": "erin"}),
            &jsonwebtoken::EncodingKey::from_secret(b"anything"),
        )
        .unwrap();
        for scheme in ["bearer", "BEARER", "BeArEr"] {
            let request = Request::builder()
                .header("Authorization", format!("{scheme} {token}"))
                .body(())
                .unwrap();
            let (mut parts, ()) = request.into_parts();
            let AuthClaims(claims) = AuthClaims::from_request_parts(&mut parts, &state)
                .await
                .unwrap();
            assert_eq!(claims.subject().unwrap(), "erin", "{scheme}");
        }
    }

    // -- discovery/JWKS fetch: bounded in time, no redirects --

    /// A verifier outside mock mode whose discovery document is fetched from
    /// `issuer` (an unroutable-port stand-in the tests control), with the
    /// given timeouts.
    fn verifier_for(issuer: &str, timeout: Duration) -> OidcVerifier {
        let settings = Arc::new(Settings {
            mode: Mode::Dev,
            oidc_issuer_url: issuer.to_string(),
            ..mock_settings()
        });
        let mut verifier = OidcVerifier::new(settings);
        verifier.http = http_client(timeout, timeout);
        verifier
    }

    /// A syntactically valid RS256 token with a `kid`, so verification gets
    /// as far as fetching the key set.
    fn token_with_kid() -> String {
        use base64::Engine;
        let b64 = |json: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json);
        format!(
            "{}.{}.c2ln",
            b64(r#"{"alg":"RS256","typ":"JWT","kid":"k1"}"#),
            b64(r#"{"sub":"alice"}"#)
        )
    }

    #[tokio::test]
    async fn a_provider_that_never_answers_fails_as_unavailable_within_the_timeout() {
        // Accepts connections and then says nothing, like a stalled IdP.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let stall = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                held.push(socket);
            }
        });

        let verifier = verifier_for(
            &format!("http://127.0.0.1:{port}/realms/r"),
            Duration::from_millis(300),
        );
        let started = Instant::now();
        let err = verifier
            .decode_bearer_token(&token_with_kid())
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::ServiceUnavailable(_)), "{err:?}");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "the fetch must not outlive its timeout"
        );
        stall.abort();
    }

    #[tokio::test]
    async fn a_redirecting_discovery_endpoint_is_not_followed() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        // The "target" the redirect points at must never be contacted.
        let target = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_port = target.local_addr().unwrap().port();
        let contacted = tokio::spawn(async move { target.accept().await.is_ok() });

        let redirector = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = redirector.local_addr().unwrap().port();
        let serve = tokio::spawn(async move {
            while let Ok((mut socket, _)) = redirector.accept().await {
                let mut buffer = [0u8; 1024];
                let _ = socket.read(&mut buffer).await;
                let reply = format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{target_port}/x\r\n\
                     Content-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = socket.write_all(reply.as_bytes()).await;
            }
        });

        let verifier = verifier_for(
            &format!("http://127.0.0.1:{port}/realms/r"),
            Duration::from_secs(2),
        );
        let err = verifier
            .decode_bearer_token(&token_with_kid())
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::ServiceUnavailable(_)), "{err:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !contacted.is_finished(),
            "the redirect target must not have been contacted"
        );
        serve.abort();
        contacted.abort();
    }

    // -- JWKS refetch: single flight, cooldown, rotation, stale-if-error --
    //
    // A stand-in identity provider (discovery + JWKS over loopback) counts
    // how many times its key set is fetched. The two RSA keys below are
    // throwaway fixtures generated for these tests (PKCS#1 DER, base64) and
    // protect nothing.

    const KEY_A_DER: &str = "MIIEowIBAAKCAQEAjdwd7rsgJL7XAqqK8f6Sw2keq1Oy0knSiaIXdwSFcQszpewn1JYfKHRhPkDDfFw7FWu7etuessVLUH+ps7SBx2JIiujLrSlnHzghZuzwHJ1MkBgfD8GgucfzapaqWHKLlGqMN/QpCTLrVIbce9+xQPV2cpHOEOSFtZlyltjJ5fAN+FymAyzyc2UBjWUt87m2pVUr7r+2IjRALQm/n6R8dB7BAREt53vVOICYm7RMGSgsCNE/xKS/odInJ+12tAvmct1ROA09aDXokoloZvHzoYzBMjgz1behixOwSXtvIpq5ZGh2Ln9xuzHgT0ABGVqVG9Sd/o47zoytQWx/lvomkQIDAQABAoIBAArWQmke3W1BMO6wtY2YC4EnEiM3cSU0Y4OFIPLEIyeIGgl2WXKZh0bRYS5exzBEYXUY9kkD5wM0JDVBk2a5huKC5RTLhOe1Dn4pjQ+uUQd2ksc9OH49JfibSEKfwOKh2XzciNqrackcnTKnXMePxpw2RDarYckTlA1T7BeO9xl8GI4UMDzeV8wx+CulIKM/+2lfsEZOr19EFCaE9noJzZIXc6zJaf5ao150oaYc3CMxnZ2nIfHyjrnJhoOyHHQWQvWqprrprTiH2nRRGNNmd71emZToNGieA8IOgWMkNB9lKWXMffeE7FSRqENrhPi1v1rzD7QSO8q5iOrZmv3sa+sCgYEAwijulhEiVCfwJ5PCdzI14d9Yiaek6GStGfN3hQP57FPrfDOgUkGQE4fTNNNEmh9nNIUAZVDXvRrG7jtMF7Slf0a1GJnxGyid+v8O/5BUqVkPQAVfxwGvmHLRWkUMJutpRq8nMaYxUF9CudzHd5BRAeMiTrW2PFQaUFiXCVw1TPcCgYEAuwrVqw5dhdnZg8IW3/NDAtog2rq4PBA9KdMlHDWJzm/3qLSIxi8Uco5karmBPy2XGSWkB5/tzZy8t4uSj2/BQ9E8bJdxFdaCobX3ENZyLW02WtMd81gWb50kRblKEaas3pODMNhv4tlJ5SvXSxBgbAsJ3ji23tTpKhazDpsibrcCgYEAs6z9XZJtmkN7UqTCiVvF0UjGvlyNlyHTrvhH2qhbWAAxI7cC3hweIHTJV1FLYvU8pLWQMDIn87Ab98qYGDfsD2R3olSTcvabe/MdBhOixTTNkAlpINjGjuVyJ39OR5YCzo64TpHlgOMm9KkDTfoXfQ+caApwolOSJlhVLSxPhvECgYBZgcvSOeOjZCblGIJd1UmiGcRTJ7t2csbyNuqxXyV9NrOrdgll3a3JTD7dm7wwAZ8nF44xX+Uq2fCtXQtbvL0duQlTZf1l1+tXe36p/hVRi8vRFupkQykpbGepPgwl+j8LnEfRniGcyyLDmoFCmKmrynSEdVt9syGJVKeHO9H0bQKBgEnaFaGZmHHc9zPpPk5wur83hQBRKWL3yNpWuvCS1KM755GYwhQvP3cYpKMsUkIMCrvBvRS8052RG4AOuEzveBBhwYItoMwX5Mb+1BsYprNkKQnS+hM4PBFZd3A2WCGXX/4x/ZgwSs1X/P6DISRHOyfSZYw5N8PFi1CKG4juf/4c";
    const KEY_A_N: &str = "jdwd7rsgJL7XAqqK8f6Sw2keq1Oy0knSiaIXdwSFcQszpewn1JYfKHRhPkDDfFw7FWu7etuessVLUH-ps7SBx2JIiujLrSlnHzghZuzwHJ1MkBgfD8GgucfzapaqWHKLlGqMN_QpCTLrVIbce9-xQPV2cpHOEOSFtZlyltjJ5fAN-FymAyzyc2UBjWUt87m2pVUr7r-2IjRALQm_n6R8dB7BAREt53vVOICYm7RMGSgsCNE_xKS_odInJ-12tAvmct1ROA09aDXokoloZvHzoYzBMjgz1behixOwSXtvIpq5ZGh2Ln9xuzHgT0ABGVqVG9Sd_o47zoytQWx_lvomkQ";
    const KEY_B_DER: &str = "MIIEpAIBAAKCAQEAr3tEVOWjq7r8iWi5+rTiCURMvzGfO35eRSRfToWxWrt65/9xC/P2/4vlnSSRkZeAvzJH4dHXMoW4ehzA1CKdNUPM54bsvQlFmDUWAxo/HLsnNUzg3mjm75iYPCi1xJcfy+JBAqDBHnCnHsam6pYv23/gpivyJ1MByDw51xxFaqtfRSsSr6n41LmiX0J34waYBMbdsiuG5LL83Hbozsz6nIu83K6JiZf+vj+TYfztO0Qy/1/w3KpkULRIY7Y9DLDGpGDu7IdoG3zGeUnXdzYijRuvLlSvPyK+nV6a+Zo4HWmyUUR5l+jwh8R52fkV3Ubpn70LQP1c/hM57RdVyRKVGQIDAQABAoIBAALrEf528K88DXPUCua7UBh/GychUkB1KChuc8rMoJ/NhxxZVEYSfTCwAQYpW+Xfzv9u/MDJwnJHxML8HC1S84oVmfBePyzgHPkMZfSwgKzvxFXHtOQh+rgwkZSLiUVPDZNwjGv4pCmHTlIt08wKKhrUgDYHPFBQ2jJcJQqZoOJmls4B1wjWh2OMAi/doGto0nLoD9RVSOuE0fVCzgYm/UrUQSczrxj/tJmsWZQPrCf8qc7UnU693mUOXD+5obTEG5j2KjuA0mJCAdDDHp/9Fco7yrMDK1Tf4XsuYLlx0bigrWOtsQmPF/lraaB6lEbpSz5DdNgq0QFbmkgprwYfnWECgYEA9smw9WpCNrehjAoxDulmeA6sGP0jxktSv4SFn5fN3g7AHXiUnvJPztn+c3/cfJdZwSnpnHqMv4ciYuedTIXeXoecYkXA8wVP2lrWDWznOWM8b8ai3HozDPjV6rKUUieHDMhMlBnBAp8Ldsc/Oaexx5K0lVIR/mrSRvaUrForn7kCgYEAtggrzF5gc+g1mVeMe9iJlFMeRAMxNaZEjrawIBcE4RxcusuUMakmhBG3mLIuAzQBCs6UoJR1SLp6IVyIaSMRJ3je7cwjpZoXaamrOGIqjhGwSRxifyryIqssXrHyN3rEuT2q1NhLGKBfvj8o5xrCV58d3nWXNYvMyvgpp1XLkGECgYAbQ1zRP9Pgx5iJJ/cTdqVjTfulJxH0rg2OKs24O8T6axa+KrwAczRt3ZdGmzF4ch/wPvkX2FIENqGN25h9A6z1NHYKRTpznnqb3L0Y7pb8uAHNN1x2julk3XiEXQYVrVg2CUqsccvoxjk1/fz2GZiCO6rHdDGYsaRZQpRaFfwLwQKBgQCdih7Vxcw2I/7XUmTb/kC7pN/pvtWE9/wXg1aUvzdS1+gNJgmAnUMfQ/IgbfzSetx4DOemvjVvFyyB2QKZ82ZyQfK13VnwhZbDtLrhllv8DqUB8VXxdyqOBJTm85yMlWzSYOOLSBJeItYZ4bG1qXwOtz/zetO93NsFwpbSIreFoQKBgQCjO4RbtrYR1bmtffvLXSKnufXXN1B33q5Mln33W41VYhrlHyKdlp/nTbyzTxB3ZO1xzyg7uTVfBPGfu0/IC71cNE+IV0+uQhmfKxq7ELXp+7INjTEbmhPo1Q2F7dB/14ZOPBoDxlyTG64sWqtEUCfp3ieEDmXWk9ADMIJ7X+e3Tg==";
    const KEY_B_N: &str = "r3tEVOWjq7r8iWi5-rTiCURMvzGfO35eRSRfToWxWrt65_9xC_P2_4vlnSSRkZeAvzJH4dHXMoW4ehzA1CKdNUPM54bsvQlFmDUWAxo_HLsnNUzg3mjm75iYPCi1xJcfy-JBAqDBHnCnHsam6pYv23_gpivyJ1MByDw51xxFaqtfRSsSr6n41LmiX0J34waYBMbdsiuG5LL83Hbozsz6nIu83K6JiZf-vj-TYfztO0Qy_1_w3KpkULRIY7Y9DLDGpGDu7IdoG3zGeUnXdzYijRuvLlSvPyK-nV6a-Zo4HWmyUUR5l-jwh8R52fkV3Ubpn70LQP1c_hM57RdVyRKVGQ";

    struct FakeIdp {
        jwks_hits: std::sync::atomic::AtomicUsize,
        jwks: std::sync::Mutex<Value>,
        failing: std::sync::atomic::AtomicBool,
        delay: Duration,
        port: u16,
    }

    impl FakeIdp {
        fn hits(&self) -> usize {
            self.jwks_hits.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn serve_keys(&self, keys: &[(&str, &str)]) {
            let keys: Vec<Value> = keys
                .iter()
                .map(|(kid, n)| {
                    serde_json::json!({
                        "kty": "RSA", "kid": kid, "use": "sig", "alg": "RS256",
                        "n": n, "e": "AQAB"
                    })
                })
                .collect();
            *self.jwks.lock().unwrap() = serde_json::json!({ "keys": keys });
        }

        fn fail(&self, failing: bool) {
            self.failing
                .store(failing, std::sync::atomic::Ordering::SeqCst);
        }

        fn issuer(&self) -> String {
            format!("http://127.0.0.1:{}/realms/r", self.port)
        }
    }

    async fn spawn_idp(
        keys: &[(&str, &str)],
        delay: Duration,
    ) -> (Arc<FakeIdp>, tokio::task::JoinHandle<()>) {
        use axum::extract::State;
        use axum::routing::get;
        use std::sync::atomic::Ordering;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let idp = Arc::new(FakeIdp {
            jwks_hits: Default::default(),
            jwks: std::sync::Mutex::new(Value::Null),
            failing: Default::default(),
            delay,
            port: listener.local_addr().unwrap().port(),
        });
        idp.serve_keys(keys);

        async fn discovery(State(idp): State<Arc<FakeIdp>>) -> axum::Json<Value> {
            axum::Json(serde_json::json!({
                "jwks_uri": format!("http://127.0.0.1:{}/jwks", idp.port)
            }))
        }
        async fn jwks(
            State(idp): State<Arc<FakeIdp>>,
        ) -> Result<axum::Json<Value>, axum::http::StatusCode> {
            idp.jwks_hits.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(idp.delay).await;
            if idp.failing.load(Ordering::SeqCst) {
                return Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
            }
            Ok(axum::Json(idp.jwks.lock().unwrap().clone()))
        }

        let app = axum::Router::new()
            .route("/realms/r/.well-known/openid-configuration", get(discovery))
            .route("/jwks", get(jwks))
            .with_state(Arc::clone(&idp));
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (idp, handle)
    }

    /// A verifier for `idp` whose refetch cooldown is `cooldown`.
    fn verifier_with_cooldown(idp: &FakeIdp, cooldown: Duration) -> OidcVerifier {
        let mut verifier = verifier_for(&idp.issuer(), Duration::from_secs(5));
        verifier.refetch_cooldown = cooldown;
        verifier
    }

    /// An RS256 token for `idp`, signed with `der` and carrying `kid`.
    fn signed_token(idp: &FakeIdp, der: &str, kid: &str) -> String {
        use base64::Engine;
        let der = base64::engine::general_purpose::STANDARD.decode(der).unwrap();
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        header.kid = Some(kid.to_string());
        let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600;
        jsonwebtoken::encode(
            &header,
            &serde_json::json!({
                "sub": "alice", "iss": idp.issuer(), "aud": "api", "exp": exp
            }),
            &jsonwebtoken::EncodingKey::from_rsa_der(&der),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn a_valid_token_verifies_against_the_fetched_keys_with_one_fetch() {
        let (idp, server) = spawn_idp(&[("a", KEY_A_N)], Duration::ZERO).await;
        let verifier = verifier_with_cooldown(&idp, Duration::from_secs(30));
        let token = signed_token(&idp, KEY_A_DER, "a");
        for _ in 0..3 {
            let claims = verifier.decode_bearer_token(&token).await.unwrap();
            assert_eq!(claims.subject().unwrap(), "alice");
        }
        assert_eq!(idp.hits(), 1, "later tokens use the cache");
        server.abort();
    }

    #[tokio::test]
    async fn concurrent_requests_with_an_unknown_kid_share_one_fetch() {
        // The delay keeps the first fetch in flight while the rest arrive.
        let (idp, server) = spawn_idp(&[("a", KEY_A_N)], Duration::from_millis(300)).await;
        let verifier = Arc::new(verifier_with_cooldown(&idp, Duration::from_secs(30)));
        let unknown = signed_token(&idp, KEY_B_DER, "b");

        let results = futures::future::join_all((0..10).map(|_| {
            let verifier = Arc::clone(&verifier);
            let unknown = unknown.clone();
            async move { verifier.decode_bearer_token(&unknown).await }
        }))
        .await;
        for result in results {
            assert!(matches!(result, Err(AppError::InvalidToken(_))), "{result:?}");
        }
        assert_eq!(idp.hits(), 1, "ten concurrent requests, one fetch");

        // Warm now; still-unknown kids ride the cooldown, not the provider.
        for _ in 0..5 {
            assert!(verifier.decode_bearer_token(&unknown).await.is_err());
        }
        assert_eq!(idp.hits(), 1);
        server.abort();
    }

    #[tokio::test]
    async fn an_unknown_kid_refetches_at_most_once_per_cooldown() {
        let (idp, server) = spawn_idp(&[("a", KEY_A_N)], Duration::ZERO).await;
        let verifier = verifier_with_cooldown(&idp, Duration::from_millis(400));
        let known = signed_token(&idp, KEY_A_DER, "a");
        let unknown = signed_token(&idp, KEY_B_DER, "b");

        verifier.decode_bearer_token(&known).await.unwrap();
        assert_eq!(idp.hits(), 1);

        // Inside the cooldown (counted from the last attempt): no fetch.
        for _ in 0..5 {
            let err = verifier.decode_bearer_token(&unknown).await.unwrap_err();
            assert!(matches!(err, AppError::InvalidToken(_)), "{err:?}");
        }
        assert_eq!(idp.hits(), 1, "the cooldown holds attacker-chosen kids off");

        // After it: exactly one more, and the next one is held off again.
        tokio::time::sleep(Duration::from_millis(500)).await;
        for _ in 0..5 {
            assert!(verifier.decode_bearer_token(&unknown).await.is_err());
        }
        assert_eq!(idp.hits(), 2);
        // Known keys never needed a fetch.
        verifier.decode_bearer_token(&known).await.unwrap();
        assert_eq!(idp.hits(), 2);
        server.abort();
    }

    #[tokio::test]
    async fn a_rotated_signing_key_is_picked_up_by_the_refetch() {
        let (idp, server) = spawn_idp(&[("a", KEY_A_N)], Duration::ZERO).await;
        let verifier = verifier_with_cooldown(&idp, Duration::from_millis(100));
        verifier
            .decode_bearer_token(&signed_token(&idp, KEY_A_DER, "a"))
            .await
            .unwrap();

        // The provider rotates: "b" signs from now on and "a" is retired.
        idp.serve_keys(&[("b", KEY_B_N)]);
        tokio::time::sleep(Duration::from_millis(150)).await;
        let claims = verifier
            .decode_bearer_token(&signed_token(&idp, KEY_B_DER, "b"))
            .await
            .unwrap();
        assert_eq!(claims.subject().unwrap(), "alice");
        assert_eq!(idp.hits(), 2);

        // The refetch replaced the set: the retired key no longer verifies.
        let err = verifier
            .decode_bearer_token(&signed_token(&idp, KEY_A_DER, "a"))
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::InvalidToken(_)), "{err:?}");
        server.abort();
    }

    #[tokio::test]
    async fn a_failed_refetch_keeps_serving_the_cached_keys() {
        let (idp, server) = spawn_idp(&[("a", KEY_A_N)], Duration::ZERO).await;
        let verifier = verifier_with_cooldown(&idp, Duration::from_millis(100));
        let known = signed_token(&idp, KEY_A_DER, "a");
        verifier.decode_bearer_token(&known).await.unwrap();

        idp.fail(true);
        tokio::time::sleep(Duration::from_millis(150)).await;
        // The unknown kid triggers a refetch, which fails: still a plain 401
        // for that token, not a 503, and the cached key keeps working.
        let err = verifier
            .decode_bearer_token(&signed_token(&idp, KEY_B_DER, "b"))
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::InvalidToken(_)), "{err:?}");
        assert_eq!(idp.hits(), 2, "the failed attempt was made");
        verifier.decode_bearer_token(&known).await.unwrap();

        // Recovery: once the provider is back and the cooldown has passed,
        // the next unknown kid is looked up again.
        idp.fail(false);
        idp.serve_keys(&[("a", KEY_A_N), ("b", KEY_B_N)]);
        tokio::time::sleep(Duration::from_millis(150)).await;
        verifier
            .decode_bearer_token(&signed_token(&idp, KEY_B_DER, "b"))
            .await
            .unwrap();
        server.abort();
    }

    #[tokio::test]
    async fn a_provider_failure_with_nothing_cached_is_unavailable() {
        let (idp, server) = spawn_idp(&[("a", KEY_A_N)], Duration::ZERO).await;
        idp.fail(true);
        let verifier = verifier_with_cooldown(&idp, Duration::from_secs(30));
        let err = verifier
            .decode_bearer_token(&signed_token(&idp, KEY_A_DER, "a"))
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::ServiceUnavailable(_)), "{err:?}");

        // Nothing to fall back on, so the cooldown does not apply: the next
        // request retries and succeeds once the provider is back.
        idp.fail(false);
        verifier
            .decode_bearer_token(&signed_token(&idp, KEY_A_DER, "a"))
            .await
            .unwrap();
        server.abort();
    }
}
