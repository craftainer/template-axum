//! `/heroes` -- Hero HTML form CRUD with progressive enhancement
//! (`FR-0034`, ports the reference's `FR-0009`). `GET /heroes/form` works
//! as a plain no-JS `<form>` against server-rendered HTML;
//! `GET /heroes/components.js` is vanilla JS that, once loaded,
//! intercepts those same forms and submits them with `fetch` instead (same
//! endpoints, same cookie), so create/update/delete don't need a full page
//! navigation once JS is available -- but every capability keeps working
//! with JS disabled, through this router's own handlers, which reuse
//! `views::hero_form`'s conversion into the exact same
//! `HeroCreate`/`HeroUpdate`/`CrudService` path the JSON router uses.
//!
//! **Auth (`docs/adrs/0031`):** this app's only auth mechanism is a bearer
//! token (`oidc::AuthClaims`); it has no OIDC login flow of its own. A plain
//! HTML navigation can't attach an `Authorization` header, and a token in the
//! URL (`?token=`) is what RFC 6750 section 2.3 says SHOULD NOT be used: it
//! lands in history, proxy and access logs and in the server's own request
//! span. So this router's `WebAuthClaims` extractor takes the token from
//!
//! 1. `Authorization: Bearer` (RFC 6750 section 2.1; API clients and
//!    tests), or
//! 2. the `hero_session` cookie, which `POST /heroes/form/session` sets after
//!    verifying a token the user pasted into a form (RFC 6750 section 2.2: a
//!    form-encoded body, never the URL). The cookie is `HttpOnly` (script
//!    cannot read it), `SameSite=Strict`, scoped to `/heroes`, `Secure`
//!    whenever `PUBLIC_BASE_URL` is https, and lives one hour at most; the
//!    token inside is verified again on every request, so it cannot outlive
//!    its own `exp`.
//!
//! A cookie is ambient authority, so unsafe requests authenticated by it
//! (and the session routes themselves) are refused when the browser says
//! the request is cross-site (`Sec-Fetch-Site`), on top of `SameSite`
//! (OWASP CSRF cheat sheet, fetch-metadata defence). No token is ever
//! rendered into a page or a URL. This is a deliberate, narrow scope
//! decision for demonstrating progressive enhancement over the existing
//! resource -- not a general browser-auth mechanism (that is the BFF's
//! job), and not something any other router in this app does.

use axum::extract::{Path, State};
use axum::http::header::{AUTHORIZATION, CACHE_CONTROL, COOKIE, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};

use crate::hero::controllers::{
    announce, enforce_write_limit, AppState, HERO_DELETE_ROLES, HERO_READ_ROLES, HERO_WRITE_ROLES,
};
use crate::hero::models::hero;
use crate::hero::views::hero_form::HeroFormFields;
use crate::oidc::{Claims, HasOidcVerifier};
use crate::problem_details::AppError;
use craftainer_crud::controllers::crud_actions;
use craftainer_crud::events::EventAction;

const COMPONENTS_JS: &str = include_str!("heroes_web_components.js");

/// The page every successful form post redirects back to.
const PAGE: &str = "/heroes/form";

/// The session cookie's name.
const SESSION_COOKIE: &str = "hero_session";

/// How long the session cookie lives, in seconds.
const SESSION_MAX_AGE_SECONDS: u64 = 3600;

/// Longest token the session form accepts (a JWT with a few roles is well
/// under 4 KiB; a cookie larger than this is refused by browsers anyway).
const MAX_TOKEN_BYTES: usize = 4096;

const REJECTED: &str = "That token was rejected -- paste a current one.";

// -- auth: bearer header, or the session cookie --

struct WebAuthClaims {
    claims: Claims,
}

/// The token of an `Authorization: Bearer` header (the scheme is
/// case-insensitive, RFC 9110 section 11.1).
fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("Bearer")
        .then(|| token.trim().to_string())
}

/// The value of the session cookie, if the request carries one.
fn cookie_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == SESSION_COOKIE)
        .map(|(_, value)| value.to_string())
}

/// Whether the browser did not mark the request cross-site. An absent
/// `Sec-Fetch-Site` (an old browser, or a client that is not a browser and
/// so carries no ambient cookie) passes; `SameSite=Strict` still applies.
fn not_cross_site(headers: &HeaderMap) -> bool {
    headers
        .get("sec-fetch-site")
        .is_none_or(|value| matches!(value.to_str(), Ok("same-origin" | "none")))
}

fn cross_site_refused() -> Response {
    AppError::Forbidden("Cross-site request refused".to_string()).into_response()
}

/// Whether `byte` is in a JWT's alphabet (base64url and `.`) or the padding
/// and standard-base64 characters: what the session cookie can carry
/// verbatim, with no quoting.
fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'~' | b'+' | b'/' | b'=')
}

/// A token worth putting in the cookie: non-empty, bounded, cookie-safe.
fn is_cookie_safe(token: &str) -> bool {
    !token.is_empty() && token.len() <= MAX_TOKEN_BYTES && token.bytes().all(is_token_byte)
}

impl<S> axum::extract::FromRequestParts<S> for WebAuthClaims
where
    S: HasOidcVerifier + Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let (token, from_cookie) = match bearer_token(&parts.headers) {
            Some(token) => (token, false),
            None => match cookie_token(&parts.headers) {
                Some(token) => (token, true),
                None => return Err(bootstrap_page(StatusCode::OK, None)),
            },
        };
        let unsafe_method = !matches!(parts.method, Method::GET | Method::HEAD | Method::OPTIONS);
        if from_cookie && unsafe_method && !not_cross_site(&parts.headers) {
            return Err(cross_site_refused());
        }

        match state.oidc_verifier().decode_bearer_token(&token).await {
            Ok(claims) => Ok(WebAuthClaims { claims }),
            Err(_) => Err(bootstrap_page(StatusCode::UNAUTHORIZED, Some(REJECTED))),
        }
    }
}

// -- rendering --

fn escape_html(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// An HTML page the browser and any shared cache must not keep: it lists
/// other owners' records and is only valid for the signed-in session.
fn page(status: StatusCode, html: String) -> Response {
    (status, [(CACHE_CONTROL, "no-store")], Html(html)).into_response()
}

fn bootstrap_page(status: StatusCode, message: Option<&str>) -> Response {
    let message = message
        .map(|m| format!("<p class=\"error\">{}</p>", escape_html(m)))
        .unwrap_or_default();
    page(
        status,
        format!(
            r#"<!doctype html>
<html><head><title>Heroes</title></head>
<body>
<h1>Heroes</h1>
{message}
<p>Paste a bearer token to continue (under <code>MODE=mock</code>, mint
one via <code>POST /mock/token</code>). It is sent in the request body and
kept in an HttpOnly cookie for this browser session, never in a URL.</p>
<form method="POST" action="/heroes/form/session">
  <label>Token <input type="password" name="token" size="80" autocomplete="off" required></label>
  <button type="submit">Continue</button>
</form>
</body></html>"#
        ),
    )
}

fn hero_row_html(hero: &hero::Model) -> String {
    let id = hero.id;
    let name = escape_html(hero.name.as_deref().unwrap_or(""));
    let powers = escape_html(&hero.powers.clone().unwrap_or_default().join(", "));
    let power_level = hero.power_level.map(|p| p.to_string()).unwrap_or_default();
    format!(
        r#"<li class="hero" data-hero-id="{id}">
  <form class="hero-form" method="POST" action="/heroes/form/{id}/update">
    <input type="text" name="name" value="{name}" required>
    <input type="text" name="powers" value="{powers}" required>
    <input type="number" name="power_level" value="{power_level}">
    <button type="submit">Update</button>
  </form>
  <form class="hero-delete-form" method="POST" action="/heroes/form/{id}/delete">
    <button type="submit">Delete</button>
  </form>
</li>"#
    )
}

fn heroes_page(heroes: &[hero::Model]) -> Response {
    let rows: String = heroes
        .iter()
        .map(hero_row_html)
        .collect::<Vec<_>>()
        .join("\n");
    page(
        StatusCode::OK,
        format!(
            r#"<!doctype html>
<html><head><title>Heroes</title><script src="/heroes/components.js" defer></script></head>
<body>
<h1>Heroes</h1>
<ul class="hero-list">
{rows}
</ul>
<h2>Create a hero</h2>
<form class="hero-form" method="POST" action="/heroes/form">
  <input type="text" name="name" placeholder="Name" required>
  <input type="text" name="powers" placeholder="Powers, comma-separated" required>
  <input type="number" name="power_level" placeholder="Power level">
  <button type="submit">Create</button>
</form>
<form class="session-form" method="POST" action="/heroes/form/logout">
  <button type="submit">Sign out</button>
</form>
</body></html>"#
        ),
    )
}

// -- session --

#[derive(Debug, serde::Deserialize)]
struct SessionForm {
    token: String,
}

/// A `Set-Cookie` value for the session cookie. `Secure` follows the public
/// origin: https in any real deployment (production requires it), plain http
/// only on a developer's machine, where browsers would drop a `Secure`
/// cookie.
fn session_cookie(state: &AppState, token: &str, max_age: u64) -> HeaderValue {
    let secure = if state
        .oidc
        .resource_server()
        .public_base_url
        .starts_with("https://")
    {
        "; Secure"
    } else {
        ""
    };
    HeaderValue::from_str(&format!(
        "{SESSION_COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/heroes; Max-Age={max_age}{secure}"
    ))
    .expect("a cookie-safe token and fixed attributes are a valid header value")
}

fn redirect_with_cookie(cookie: HeaderValue) -> Response {
    let mut response = Redirect::to(PAGE).into_response();
    response.headers_mut().insert(SET_COOKIE, cookie);
    response
}

/// `POST /heroes/form/session` -- verify the pasted token and keep it in the
/// session cookie.
async fn start_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<SessionForm>,
) -> Response {
    if !not_cross_site(&headers) {
        return cross_site_refused();
    }
    let token = form.token.trim();
    // Checked before verifying: only a token the cookie can carry verbatim
    // is worth the verification round trip.
    if !is_cookie_safe(token) || state.oidc.decode_bearer_token(token).await.is_err() {
        return bootstrap_page(StatusCode::UNAUTHORIZED, Some(REJECTED));
    }
    redirect_with_cookie(session_cookie(&state, token, SESSION_MAX_AGE_SECONDS))
}

/// `POST /heroes/form/logout` -- clear the session cookie.
async fn end_session(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !not_cross_site(&headers) {
        return cross_site_refused();
    }
    redirect_with_cookie(session_cookie(&state, "", 0))
}

// -- handlers --

async fn form_page(
    State(state): State<AppState>,
    claims: Result<WebAuthClaims, Response>,
) -> Response {
    let WebAuthClaims { claims } = match claims {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    if let Err(err) = claims.require_any_role(&state.settings.oidc_client_id, HERO_READ_ROLES) {
        return err.into_response();
    }
    let heroes = match state
        .hero_crud
        .list(0, 100, false, false, vec![], vec![])
        .await
    {
        Ok(heroes) => heroes,
        Err(err) => return AppError::from(err).into_response(),
    };
    heroes_page(&heroes)
}

async fn create(
    State(state): State<AppState>,
    claims: Result<WebAuthClaims, Response>,
    Form(fields): Form<HeroFormFields>,
) -> Response {
    let WebAuthClaims { claims } = match claims {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    if let Err(err) = claims.require_any_role(&state.settings.oidc_client_id, HERO_WRITE_ROLES) {
        return err.into_response();
    }
    if let Err(err) = enforce_write_limit(&state, &claims).await {
        return err.into_response();
    }
    let create = match fields.into_hero_create() {
        Ok(create) => create,
        Err(errors) => return AppError::UnprocessableEntity(errors).into_response(),
    };
    let owner_id = match claims.subject() {
        Ok(subject) => subject.to_string(),
        Err(err) => return err.into_response(),
    };
    let hero = match state.hero_crud.create(&owner_id, create).await {
        Ok(hero) => hero,
        Err(err) => return AppError::from(err).into_response(),
    };
    announce(&state, EventAction::Create, vec![hero.id]).await;
    Redirect::to(PAGE).into_response()
}

async fn update(
    State(state): State<AppState>,
    Path(id): Path<i32>,
    claims: Result<WebAuthClaims, Response>,
    Form(fields): Form<HeroFormFields>,
) -> Response {
    let WebAuthClaims { claims } = match claims {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    if let Err(err) = claims.require_any_role(&state.settings.oidc_client_id, HERO_WRITE_ROLES) {
        return err.into_response();
    }
    if let Err(err) = enforce_write_limit(&state, &claims).await {
        return err.into_response();
    }
    let update = match fields.into_hero_update() {
        Ok(update) => update,
        Err(errors) => return AppError::UnprocessableEntity(errors).into_response(),
    };
    let owner_id = match claims.subject() {
        Ok(subject) => subject.to_string(),
        Err(err) => return err.into_response(),
    };
    match crud_actions::resolve_update_by_id(&state.hero_crud, id, &owner_id, update).await {
        Ok(hero) => {
            announce(&state, EventAction::Update, vec![hero.id]).await;
            Redirect::to(PAGE).into_response()
        }
        Err(err) => err.into_response(),
    }
}

async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i32>,
    claims: Result<WebAuthClaims, Response>,
) -> Response {
    let WebAuthClaims { claims } = match claims {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    if let Err(err) = claims.require_any_role(&state.settings.oidc_client_id, HERO_DELETE_ROLES) {
        return err.into_response();
    }
    if let Err(err) = enforce_write_limit(&state, &claims).await {
        return err.into_response();
    }
    let owner_id = match claims.subject() {
        Ok(subject) => subject.to_string(),
        Err(err) => return err.into_response(),
    };
    match crud_actions::resolve_delete(
        &state.hero_crud,
        Some(id),
        &owner_id,
        vec![],
        state.settings.bulk_action_max_matched,
        // by-id delete only (`Some(id)`): the bulk-only flag is moot
        true,
    )
    .await
    {
        Ok(_) => {
            announce(&state, EventAction::Delete, vec![id]).await;
            Redirect::to(PAGE).into_response()
        }
        Err(err) => err.into_response(),
    }
}

async fn components_js() -> impl IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/javascript; charset=utf-8",
        )],
        COMPONENTS_JS,
    )
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/form", get(form_page).post(create))
        .route("/form/session", post(start_session))
        .route("/form/logout", post(end_session))
        .route("/form/{id}/update", post(update))
        .route("/form/{id}/delete", post(delete))
        .route("/components.js", get(components_js))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Mode, Settings};
    use crate::hero::controllers::DynHeroRepository;
    use crate::hero::repositories::hero_memory::HeroMemoryRepository;
    use crate::oidc::OidcVerifier;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use craftainer_observability::health::HealthRegistry;
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

    fn app() -> Router {
        let settings = Arc::new(mock_settings());
        let state = AppState {
            oidc: Arc::new(OidcVerifier::new(settings.clone())),
            settings,
            health_registry: Arc::new(HealthRegistry::new()),
            hero_crud: Arc::new(craftainer_crud::crud::CrudService::new(DynHeroRepository(
                Box::new(HeroMemoryRepository::new()),
            ))),
            rate_limiter: Arc::new(crate::rate_limit::RateLimiter::mock()),
            hero_stats: craftainer_crud::stats_cache::CachedStats::uncached("heroes"),
            shutdown: tokio_util::sync::CancellationToken::new(),
            events: Arc::new(craftainer_crud::events::EventBus::mock()),
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

    async fn body_text(response: Response) -> String {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    /// Always fails `list`/`create` -- the in-memory repository these
    /// tests otherwise use never returns `Err`, so `form_page`/`create`'s
    /// own error-mapping branches are otherwise unreachable. Every other
    /// method is unused by the two tests that exercise this fake.
    #[derive(Default)]
    struct FaultyRepository;

    #[async_trait::async_trait]
    impl craftainer_crud::repositories::Repository for FaultyRepository {
        type Model = hero::Model;
        type Create = crate::hero::views::hero::HeroCreate;
        type Update = crate::hero::views::hero::HeroUpdate;

        async fn list(
            &self,
            _opts: craftainer_crud::repositories::ListOptions,
        ) -> Result<Vec<Self::Model>, craftainer_crud::repositories::RepoError> {
            Err(craftainer_crud::repositories::RepoError::Backend(
                "boom".to_string(),
            ))
        }

        async fn count(
            &self,
            _filters: &[craftainer_crud::repositories::filtering::FilterClause],
            _include_archived: bool,
            _only_archived: bool,
            _include_unpublished: bool,
        ) -> Result<u64, craftainer_crud::repositories::RepoError> {
            unimplemented!()
        }

        async fn get(
            &self,
            _id: i32,
            _include_archived: bool,
        ) -> Result<Option<Self::Model>, craftainer_crud::repositories::RepoError> {
            unimplemented!()
        }

        async fn create(
            &self,
            _owner_id: &str,
            _data: Self::Create,
        ) -> Result<Self::Model, craftainer_crud::repositories::RepoError> {
            Err(craftainer_crud::repositories::RepoError::Backend(
                "boom".to_string(),
            ))
        }

        async fn update(
            &self,
            _id: i32,
            _owner_id: &str,
            _data: Self::Update,
        ) -> Result<Option<Self::Model>, craftainer_crud::repositories::RepoError> {
            unimplemented!()
        }

        async fn update_many(
            &self,
            _filters: &[craftainer_crud::repositories::filtering::FilterClause],
            _data: Self::Update,
        ) -> Result<Vec<Self::Model>, craftainer_crud::repositories::RepoError> {
            unimplemented!()
        }

        async fn delete(
            &self,
            _id: i32,
            _owner_id: &str,
        ) -> Result<bool, craftainer_crud::repositories::RepoError> {
            unimplemented!()
        }

        async fn delete_many(
            &self,
            _filters: &[craftainer_crud::repositories::filtering::FilterClause],
            _include_unpublished: bool,
        ) -> Result<Vec<Self::Model>, craftainer_crud::repositories::RepoError> {
            unimplemented!()
        }
    }

    fn app_with_faulty_repository() -> Router {
        let settings = Arc::new(mock_settings());
        let state = AppState {
            oidc: Arc::new(OidcVerifier::new(settings.clone())),
            settings,
            health_registry: Arc::new(HealthRegistry::new()),
            hero_crud: Arc::new(craftainer_crud::crud::CrudService::new(DynHeroRepository(
                Box::new(FaultyRepository),
            ))),
            rate_limiter: Arc::new(crate::rate_limit::RateLimiter::mock()),
            hero_stats: craftainer_crud::stats_cache::CachedStats::uncached("heroes"),
            shutdown: tokio_util::sync::CancellationToken::new(),
            events: Arc::new(craftainer_crud::events::EventBus::mock()),
        };
        router().with_state(state)
    }

    #[tokio::test]
    async fn form_page_maps_a_repository_failure_to_an_app_error_response() {
        let response = app_with_faulty_repository()
            .oneshot(
                Request::builder()
                    .uri("/form")
                    .header("Authorization", format!("Bearer {}", token("alice", &["viewer"])))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn create_maps_a_repository_failure_to_an_app_error_response() {
        let response = app_with_faulty_repository()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/form")
                    .header("Authorization", format!("Bearer {}", token("alice", &["editor"])))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Spectra&powers=flight&power_level=5"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn form_page_with_no_token_renders_the_bootstrap_page() {
        let response = app()
            .oneshot(Request::builder().uri("/form").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_text(response).await;
        assert!(body.contains("Paste a bearer token"));
    }

    #[tokio::test]
    async fn form_page_with_an_invalid_token_shows_a_rejection_message() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/form")
                    .header("Authorization", "Bearer not-a-real-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = body_text(response).await;
        assert!(body.contains("was rejected"));
    }

    #[tokio::test]
    async fn form_page_with_a_valid_token_lists_heroes_and_a_create_form() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/form")
                    .header("Authorization", format!("Bearer {}", token("alice", &["viewer"])))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_text(response).await;
        assert!(body.contains("Create a hero"));
    }

    #[tokio::test]
    async fn create_via_form_post_redirects_and_the_hero_then_appears_in_the_list() {
        let shared_app = app();
        let create_token = token("alice", &["editor"]);
        let create = shared_app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/form")
                    .header("Authorization", format!("Bearer {create_token}"))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Spectra&powers=flight&power_level=5"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::SEE_OTHER);

        let list = shared_app
            .oneshot(
                Request::builder()
                    .uri("/form")
                    .header("Authorization", format!("Bearer {}", token("alice", &["viewer"])))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_text(list).await;
        assert!(body.contains("Spectra"));
        assert!(body.contains("flight"));
    }

    #[tokio::test]
    async fn create_with_an_invalid_payload_returns_422_problem_details() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/form")
                    .header("Authorization", format!("Bearer {}", token("alice", &["editor"])))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=&powers=&power_level="))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn a_role_with_no_write_grant_is_forbidden_from_creating() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/form")
                    .header("Authorization", format!("Bearer {}", token("alice", &["viewer"])))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Spectra&powers=flight&power_level=5"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn update_and_delete_via_form_post_round_trip() {
        let shared_app = app();
        let owner_token = token("alice", &["editor", "maintainer"]);
        let create = shared_app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/form")
                    .header("Authorization", format!("Bearer {owner_token}"))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Spectra&powers=flight&power_level=5"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::SEE_OTHER);

        let list = shared_app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/form")
                    .header("Authorization", format!("Bearer {owner_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_text(list).await;
        let id: i32 = body
            .split("data-hero-id=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .parse()
            .unwrap();

        let update = shared_app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/form/{id}/update"))
                    .header("Authorization", format!("Bearer {owner_token}"))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Renamed&powers=stealth&power_level=9"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(update.status(), StatusCode::SEE_OTHER);

        let after_update = shared_app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/form")
                    .header("Authorization", format!("Bearer {owner_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_text(after_update).await;
        assert!(body.contains("Renamed"));
        assert!(body.contains("stealth"));

        let delete = shared_app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/form/{id}/delete"))
                    .header("Authorization", format!("Bearer {owner_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(delete.status(), StatusCode::SEE_OTHER);

        let after_delete = shared_app
            .oneshot(
                Request::builder()
                    .uri("/form")
                    .header("Authorization", format!("Bearer {owner_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_text(after_delete).await;
        assert!(!body.contains("Renamed"));
    }

    fn token_with_no_sub(roles: &[&str]) -> String {
        jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
            &serde_json::json!({
                "resource_access": { "api": { "roles": roles } }
            }),
            &jsonwebtoken::EncodingKey::from_secret(b"mock-mode-doesnt-verify-signatures"),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn form_page_is_forbidden_without_a_read_role() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/form")
                    .header("Authorization", format!("Bearer {}", token("alice", &["security"])))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn create_without_a_token_shows_the_bootstrap_page() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/form")
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Spectra&powers=flight&power_level=5"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn create_rejects_a_token_with_no_subject() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/form")
                    .header("Authorization", format!("Bearer {}", token_with_no_sub(&["editor"])))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Spectra&powers=flight&power_level=5"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn update_without_a_token_shows_the_bootstrap_page() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/form/1/update")
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Renamed&powers=stealth&power_level=9"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn update_is_forbidden_without_a_write_role() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/form/1/update")
                    .header("Authorization", format!("Bearer {}", token("alice", &["viewer"])))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Renamed&powers=stealth&power_level=9"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn update_rejects_an_invalid_payload_with_422() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/form/1/update")
                    .header("Authorization", format!("Bearer {}", token("alice", &["editor"])))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "name=Renamed&powers=stealth&power_level=not-a-number",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn update_rejects_a_token_with_no_subject() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/form/1/update")
                    .header("Authorization", format!("Bearer {}", token_with_no_sub(&["editor"])))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Renamed&powers=stealth&power_level=9"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn update_of_a_nonexistent_id_returns_404() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/form/999999/update")
                    .header("Authorization", format!("Bearer {}", token("alice", &["editor"])))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Renamed&powers=stealth&power_level=9"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_without_a_token_shows_the_bootstrap_page() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/form/1/delete")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn delete_is_forbidden_without_the_maintainer_role() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/form/1/delete")
                    .header("Authorization", format!("Bearer {}", token("alice", &["editor"])))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn delete_rejects_a_token_with_no_subject() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/form/1/delete")
                    .header("Authorization", format!("Bearer {}", token_with_no_sub(&["maintainer"])))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn delete_of_a_nonexistent_id_returns_404() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/form/999999/delete")
                    .header("Authorization", format!("Bearer {}", token("alice", &["maintainer"])))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    fn app_with_write_limit(limit: u32) -> Router {
        let settings = Arc::new(Settings {
            rate_limit_hero_write_per_minute: limit,
            ..mock_settings()
        });
        let state = AppState {
            oidc: Arc::new(OidcVerifier::new(settings.clone())),
            settings,
            health_registry: Arc::new(HealthRegistry::new()),
            hero_crud: Arc::new(craftainer_crud::crud::CrudService::new(DynHeroRepository(
                Box::new(HeroMemoryRepository::new()),
            ))),
            rate_limiter: Arc::new(crate::rate_limit::RateLimiter::mock()),
            hero_stats: craftainer_crud::stats_cache::CachedStats::uncached("heroes"),
            shutdown: tokio_util::sync::CancellationToken::new(),
            events: Arc::new(craftainer_crud::events::EventBus::mock()),
        };
        router().with_state(state)
    }

    #[tokio::test]
    async fn create_returns_429_once_the_per_caller_limit_is_exceeded() {
        let shared_app = app_with_write_limit(1);
        let create_token = token("alice", &["editor"]);
        for _ in 0..1 {
            shared_app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/form")
                        .header("Authorization", format!("Bearer {create_token}"))
                        .header("Content-Type", "application/x-www-form-urlencoded")
                        .body(Body::from("name=Spectra&powers=flight&power_level=5"))
                        .unwrap(),
                )
                .await
                .unwrap();
        }
        let response = shared_app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/form")
                    .header("Authorization", format!("Bearer {create_token}"))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Spectra&powers=flight&power_level=5"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn update_returns_429_once_the_per_caller_limit_is_exceeded() {
        let shared_app = app_with_write_limit(1);
        let owner_token = token("alice", &["editor"]);
        // Spends the one allowed write on the create itself, so the
        // update below is the first write to find the limit already hit.
        let create = shared_app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/form")
                    .header("Authorization", format!("Bearer {owner_token}"))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Spectra&powers=flight&power_level=5"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::SEE_OTHER);

        let response = shared_app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/form/1/update")
                    .header("Authorization", format!("Bearer {owner_token}"))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Renamed&powers=stealth&power_level=9"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn delete_returns_429_once_the_per_caller_limit_is_exceeded() {
        let shared_app = app_with_write_limit(1);
        let owner_token = token("alice", &["editor", "maintainer"]);
        let create = shared_app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/form")
                    .header("Authorization", format!("Bearer {owner_token}"))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Spectra&powers=flight&power_level=5"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::SEE_OTHER);

        let response = shared_app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/form/1/delete")
                    .header("Authorization", format!("Bearer {owner_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn components_js_is_served_as_javascript() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/components.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .unwrap(),
            "text/javascript; charset=utf-8"
        );
        let body = body_text(response).await;
        assert!(body.contains("hero-form"));
    }
    // -- the session cookie replaces `?token=` (docs/adrs/0031) --

    fn session_post(path: &str, body: String, extra: &[(&str, &str)]) -> Request<Body> {
        let mut builder = Request::builder()
            .method("POST")
            .uri(path)
            .header("Content-Type", "application/x-www-form-urlencoded");
        for (name, value) in extra {
            builder = builder.header(*name, *value);
        }
        builder.body(Body::from(body)).unwrap()
    }

    fn set_cookie_of(response: &Response) -> String {
        response
            .headers()
            .get(SET_COOKIE)
            .expect("a Set-Cookie header")
            .to_str()
            .unwrap()
            .to_string()
    }

    #[tokio::test]
    async fn the_token_in_the_query_string_is_no_longer_accepted() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri(format!("/form?token={}", token("alice", &["viewer"])))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // Treated as no credentials at all: the bootstrap page, not the list.
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_text(response).await;
        assert!(body.contains("Paste a bearer token"), "{body}");
        assert!(!body.contains("Create a hero"), "{body}");
    }

    #[tokio::test]
    async fn posting_a_token_starts_a_session_cookie_that_authenticates_later_requests() {
        let shared_app = app();
        let tok = token("alice", &["editor"]);
        let started = shared_app
            .clone()
            .oneshot(session_post(
                "/form/session",
                format!("token={tok}"),
                &[("Sec-Fetch-Site", "same-origin")],
            ))
            .await
            .unwrap();
        assert_eq!(started.status(), StatusCode::SEE_OTHER);
        assert_eq!(started.headers()["location"], "/heroes/form");
        let cookie = set_cookie_of(&started);
        assert!(cookie.starts_with(&format!("hero_session={tok};")), "{cookie}");
        for attribute in ["HttpOnly", "SameSite=Strict", "Path=/heroes", "Max-Age=3600"] {
            assert!(cookie.contains(attribute), "{attribute} in {cookie}");
        }
        // http://localhost (the default public origin): a Secure cookie would
        // be dropped by the browser.
        assert!(!cookie.contains("Secure"), "{cookie}");

        // The cookie alone authenticates a navigation...
        let cookie_header = format!("a=b; hero_session={tok}; c=d");
        let page = shared_app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/form")
                    .header("Cookie", cookie_header.as_str())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(page.status(), StatusCode::OK);
        assert_eq!(page.headers()[CACHE_CONTROL], "no-store");
        let body = body_text(page).await;
        assert!(body.contains("Create a hero"));
        // ...and the page never repeats the token, in a URL or anywhere.
        assert!(!body.contains(&tok), "the token must not be rendered");
        assert!(!body.contains("token="), "{body}");

        // ...and a form post, which a same-origin browser sends.
        let created = shared_app
            .clone()
            .oneshot(session_post(
                "/form",
                "name=Spectra&powers=flight&power_level=5".to_string(),
                &[("Cookie", cookie_header.as_str()), ("Sec-Fetch-Site", "same-origin")],
            ))
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::SEE_OTHER);
        assert_eq!(created.headers()["location"], "/heroes/form");
    }

    #[tokio::test]
    async fn a_cookie_authenticated_post_marked_cross_site_is_refused() {
        let tok = token("alice", &["editor"]);
        for site in ["cross-site", "same-site", "bogus"] {
            let response = app()
                .oneshot(session_post(
                    "/form",
                    "name=Spectra&powers=flight".to_string(),
                    &[
                        ("Cookie", format!("hero_session={tok}").as_str()),
                        ("Sec-Fetch-Site", site),
                    ],
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{site}");
        }
        // The same post with a bearer header is not ambient authority, so
        // the fetch metadata is irrelevant to it.
        let response = app()
            .oneshot(session_post(
                "/form",
                "name=Spectra&powers=flight".to_string(),
                &[
                    ("Authorization", format!("Bearer {tok}").as_str()),
                    ("Sec-Fetch-Site", "cross-site"),
                ],
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        // A navigation (GET) is safe whatever it says.
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/form")
                    .header("Cookie", format!("hero_session={}", token("a", &["viewer"])))
                    .header("Sec-Fetch-Site", "cross-site")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn the_session_routes_refuse_cross_site_requests() {
        let tok = token("alice", &["editor"]);
        for (path, body) in [
            ("/form/session", format!("token={tok}")),
            ("/form/logout", String::new()),
        ] {
            let response = app()
                .oneshot(session_post(path, body, &[("Sec-Fetch-Site", "cross-site")]))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
            assert!(response.headers().get(SET_COOKIE).is_none(), "{path}");
        }
    }

    #[tokio::test]
    async fn a_token_that_does_not_verify_or_cannot_be_a_cookie_value_starts_no_session() {
        for body in [
            "token=not-a-real-token".to_string(),
            "token=".to_string(),
            "token=a%3Bb".to_string(),
            "token=a%20b".to_string(),
            format!("token={}", "a".repeat(MAX_TOKEN_BYTES + 1)),
        ] {
            let response = app()
                .oneshot(session_post("/form/session", body.clone(), &[]))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{body}");
            assert!(response.headers().get(SET_COOKIE).is_none(), "{body}");
            assert!(body_text(response).await.contains("was rejected"));
        }
    }

    #[tokio::test]
    async fn logging_out_clears_the_cookie() {
        let response = app()
            .oneshot(session_post("/form/logout", String::new(), &[]))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let cookie = set_cookie_of(&response);
        assert!(cookie.starts_with("hero_session=;"), "{cookie}");
        assert!(cookie.contains("Max-Age=0"), "{cookie}");
    }

    #[tokio::test]
    async fn the_session_cookie_is_secure_when_the_public_origin_is_https() {
        let settings = Arc::new(mock_settings());
        let resource = crate::config::ResourceServer {
            public_base_url: "https://api.example.com".to_string(),
            ..crate::config::ResourceServer::from_settings(&settings)
        };
        let state = AppState {
            oidc: Arc::new(OidcVerifier::with_resource_server(
                settings.clone(),
                Arc::new(resource),
            )),
            settings,
            health_registry: Arc::new(HealthRegistry::new()),
            hero_crud: Arc::new(craftainer_crud::crud::CrudService::new(DynHeroRepository(
                Box::new(HeroMemoryRepository::new()),
            ))),
            rate_limiter: Arc::new(crate::rate_limit::RateLimiter::mock()),
            hero_stats: craftainer_crud::stats_cache::CachedStats::uncached("heroes"),
            shutdown: tokio_util::sync::CancellationToken::new(),
            events: Arc::new(craftainer_crud::events::EventBus::mock()),
        };
        let response = router()
            .with_state(state)
            .oneshot(session_post(
                "/form/session",
                format!("token={}", token("alice", &["viewer"])),
                &[],
            ))
            .await
            .unwrap();
        assert!(set_cookie_of(&response).ends_with("; Secure"));
    }

    #[tokio::test]
    async fn a_lowercase_bearer_scheme_authenticates_the_form_page() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/form")
                    .header(
                        "Authorization",
                        format!("bearer {}", token("alice", &["viewer"])),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(body_text(response).await.contains("Create a hero"));
    }

    #[test]
    fn only_a_bearer_scheme_and_the_session_cookie_are_credentials() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Basic dXNlcjpwYXNz"));
        headers.insert(COOKIE, HeaderValue::from_static("other=1; hero_session_x=2"));
        assert_eq!(bearer_token(&headers), None);
        assert_eq!(cookie_token(&headers), None);
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer"));
        assert_eq!(bearer_token(&headers), None);
    }

}
