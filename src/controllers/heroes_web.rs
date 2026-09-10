//! `/heroes` -- Hero HTML form CRUD with progressive enhancement
//! (`FR-0034`, ports the reference's `FR-0009`). `GET /heroes/form` works
//! as a plain no-JS `<form>` against server-rendered HTML;
//! `GET /heroes/components.js` is vanilla JS that, once loaded,
//! intercepts those same forms and drives the *existing* JSON API
//! (`controllers::heroes`) via `fetch` instead, so create/update/delete
//! don't need a full page navigation once JS is available -- but every
//! capability keeps working with JS disabled, through this router's own
//! handlers, which reuse `views::hero_form`'s conversion into the exact
//! same `HeroCreate`/`HeroUpdate`/`CrudService` path the JSON router
//! uses.
//!
//! **Auth, and why this router alone accepts `?token=`:** this app's only
//! auth mechanism is a bearer token (`oidc::AuthClaims`); it has no
//! browser session/cookie login flow (the `openidconnect`/`oauth2`
//! dependencies `Cargo.toml` pins stay unwired -- see that file's own
//! note). A plain HTML navigation can't attach an `Authorization` header,
//! so this router's own `WebAuthClaims` extractor additionally accepts
//! the token via `?token=`, and every link/form this router renders
//! carries it forward in its `action`/`href`. This is a deliberate,
//! narrow scope decision for demonstrating progressive enhancement over
//! the existing JSON API -- not a general browser-auth mechanism, and not
//! something any other router in this app does.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};

use crate::controllers::heroes::HERO_WRITE_RATE_SCOPE;
use crate::controllers::{
    crud_actions, crud_events, AppState, HERO_DELETE_ROLES, HERO_EVENT_RESOURCE, HERO_READ_ROLES,
    HERO_WRITE_ROLES,
};
use crate::events::EventAction;
use crate::models::hero;
use crate::oidc::{Claims, HasOidcVerifier};
use crate::problem_details::AppError;
use crate::views::hero_form::HeroFormFields;

const COMPONENTS_JS: &str = include_str!("heroes_web_components.js");

// -- auth: bearer header, or `?token=` for a plain HTML navigation --

#[derive(Debug, serde::Deserialize)]
struct TokenQuery {
    token: Option<String>,
}

struct WebAuthClaims {
    claims: Claims,
    token: String,
}

impl<S> axum::extract::FromRequestParts<S> for WebAuthClaims
where
    S: HasOidcVerifier + Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let header_token = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(str::to_string);

        let query_token = Query::<TokenQuery>::from_request_parts(parts, state)
            .await
            .ok()
            .and_then(|Query(TokenQuery { token })| token);

        let Some(token) = header_token.or(query_token) else {
            return Err(bootstrap_page(None).into_response());
        };

        match state.oidc_verifier().decode_bearer_token(&token).await {
            Ok(claims) => Ok(WebAuthClaims { claims, token }),
            Err(_) => Err((
                StatusCode::UNAUTHORIZED,
                bootstrap_page(Some("That token was rejected -- paste a current one.")),
            )
                .into_response()),
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

fn bootstrap_page(message: Option<&str>) -> Html<String> {
    let message = message
        .map(|m| format!("<p class=\"error\">{}</p>", escape_html(m)))
        .unwrap_or_default();
    Html(format!(
        r#"<!doctype html>
<html><head><title>Heroes</title></head>
<body>
<h1>Heroes</h1>
{message}
<p>Paste a bearer token to continue (under <code>MODE=mock</code>, mint
one via <code>POST /mock/token</code>).</p>
<form method="GET" action="/heroes/form">
  <label>Token <input type="text" name="token" size="80" required></label>
  <button type="submit">Continue</button>
</form>
</body></html>"#
    ))
}

fn hero_row_html(hero: &hero::Model, token: &str) -> String {
    let id = hero.id;
    let name = escape_html(hero.name.as_deref().unwrap_or(""));
    let powers = escape_html(&hero.powers.clone().unwrap_or_default().join(", "));
    let power_level = hero.power_level.map(|p| p.to_string()).unwrap_or_default();
    format!(
        r#"<li class="hero" data-hero-id="{id}">
  <form class="hero-form" method="POST" action="/heroes/form/{id}/update?token={token}" data-json-action="/crud/v1/heroes/v2/json?id={id}" data-json-method="PATCH">
    <input type="text" name="name" value="{name}" required>
    <input type="text" name="powers" value="{powers}" required>
    <input type="number" name="power_level" value="{power_level}">
    <button type="submit">Update</button>
  </form>
  <form class="hero-delete-form" method="POST" action="/heroes/form/{id}/delete?token={token}" data-json-action="/crud/v1/heroes/v2/json?id={id}" data-json-method="DELETE">
    <button type="submit">Delete</button>
  </form>
</li>"#
    )
}

fn heroes_page(heroes: &[hero::Model], token: &str) -> Html<String> {
    let rows: String = heroes
        .iter()
        .map(|hero| hero_row_html(hero, token))
        .collect::<Vec<_>>()
        .join("\n");
    Html(format!(
        r#"<!doctype html>
<html><head><title>Heroes</title><script src="/heroes/components.js?token={token}" defer></script></head>
<body data-token="{token}">
<h1>Heroes</h1>
<ul class="hero-list">
{rows}
</ul>
<h2>Create a hero</h2>
<form class="hero-form" method="POST" action="/heroes/form?token={token}" data-json-action="/crud/v1/heroes/v2/json" data-json-method="POST">
  <input type="text" name="name" placeholder="Name" required>
  <input type="text" name="powers" placeholder="Powers, comma-separated" required>
  <input type="number" name="power_level" placeholder="Power level">
  <button type="submit">Create</button>
</form>
</body></html>"#
    ))
}

// -- handlers --

async fn form_page(
    State(state): State<AppState>,
    claims: Result<WebAuthClaims, Response>,
) -> Response {
    let WebAuthClaims { claims, token } = match claims {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    if let Err(err) = claims.require_any_role(&state.settings.oidc_client_id, HERO_READ_ROLES) {
        return err.into_response();
    }
    let heroes = match state.hero_crud.list(0, 100, false, vec![], vec![]).await {
        Ok(heroes) => heroes,
        Err(err) => return AppError::from(err).into_response(),
    };
    heroes_page(&heroes, &token).into_response()
}

async fn create(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    claims: Result<WebAuthClaims, Response>,
    Form(fields): Form<HeroFormFields>,
) -> Response {
    let WebAuthClaims { claims, token } = match claims {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    if let Err(err) = state
        .rate_limiter
        .check(
            HERO_WRITE_RATE_SCOPE,
            addr.ip(),
            state.settings.rate_limit_hero_write_per_minute,
            60,
        )
        .await
    {
        return err.into_response();
    }
    if let Err(err) = claims.require_any_role(&state.settings.oidc_client_id, HERO_WRITE_ROLES) {
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
    crud_events::publish(
        &state.events,
        HERO_EVENT_RESOURCE,
        EventAction::Create,
        vec![hero.id],
    )
    .await;
    Redirect::to(&format!("/heroes/form?token={token}")).into_response()
}

async fn update(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(id): Path<i32>,
    claims: Result<WebAuthClaims, Response>,
    Form(fields): Form<HeroFormFields>,
) -> Response {
    let WebAuthClaims { claims, token } = match claims {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    if let Err(err) = state
        .rate_limiter
        .check(
            HERO_WRITE_RATE_SCOPE,
            addr.ip(),
            state.settings.rate_limit_hero_write_per_minute,
            60,
        )
        .await
    {
        return err.into_response();
    }
    if let Err(err) = claims.require_any_role(&state.settings.oidc_client_id, HERO_WRITE_ROLES) {
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
            crud_events::publish(
                &state.events,
                HERO_EVENT_RESOURCE,
                EventAction::Update,
                vec![hero.id],
            )
            .await;
            Redirect::to(&format!("/heroes/form?token={token}")).into_response()
        }
        Err(err) => err.into_response(),
    }
}

async fn delete(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(id): Path<i32>,
    claims: Result<WebAuthClaims, Response>,
) -> Response {
    let WebAuthClaims { claims, token } = match claims {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    if let Err(err) = state
        .rate_limiter
        .check(
            HERO_WRITE_RATE_SCOPE,
            addr.ip(),
            state.settings.rate_limit_hero_write_per_minute,
            60,
        )
        .await
    {
        return err.into_response();
    }
    if let Err(err) = claims.require_any_role(&state.settings.oidc_client_id, HERO_DELETE_ROLES) {
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
    )
    .await
    {
        Ok(_) => {
            crud_events::publish(
                &state.events,
                HERO_EVENT_RESOURCE,
                EventAction::Delete,
                vec![id],
            )
            .await;
            Redirect::to(&format!("/heroes/form?token={token}")).into_response()
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
        .route("/form/{id}/update", post(update))
        .route("/form/{id}/delete", post(delete))
        .route("/components.js", get(components_js))
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
    use axum::http::Request;
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

    fn with_addr(mut request: Request<Body>) -> Request<Body> {
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
        request
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
    impl crate::repositories::Repository for FaultyRepository {
        type Model = hero::Model;
        type Create = crate::views::hero::HeroCreate;
        type Update = crate::views::hero::HeroUpdate;

        async fn list(
            &self,
            _opts: crate::repositories::ListOptions,
        ) -> Result<Vec<Self::Model>, crate::repositories::RepoError> {
            Err(crate::repositories::RepoError::Backend("boom".to_string()))
        }

        async fn count(
            &self,
            _filters: &[crate::repositories::filtering::FilterClause],
            _include_archived: bool,
        ) -> Result<u64, crate::repositories::RepoError> {
            unimplemented!()
        }

        async fn get(
            &self,
            _id: i32,
            _include_archived: bool,
        ) -> Result<Option<Self::Model>, crate::repositories::RepoError> {
            unimplemented!()
        }

        async fn create(
            &self,
            _owner_id: &str,
            _data: Self::Create,
        ) -> Result<Self::Model, crate::repositories::RepoError> {
            Err(crate::repositories::RepoError::Backend("boom".to_string()))
        }

        async fn update(
            &self,
            _id: i32,
            _owner_id: &str,
            _data: Self::Update,
        ) -> Result<Option<Self::Model>, crate::repositories::RepoError> {
            unimplemented!()
        }

        async fn update_many(
            &self,
            _filters: &[crate::repositories::filtering::FilterClause],
            _data: Self::Update,
        ) -> Result<Vec<Self::Model>, crate::repositories::RepoError> {
            unimplemented!()
        }

        async fn delete(
            &self,
            _id: i32,
            _owner_id: &str,
        ) -> Result<bool, crate::repositories::RepoError> {
            unimplemented!()
        }

        async fn delete_many(
            &self,
            _filters: &[crate::repositories::filtering::FilterClause],
        ) -> Result<Vec<Self::Model>, crate::repositories::RepoError> {
            unimplemented!()
        }
    }

    fn app_with_faulty_repository() -> Router {
        let settings = Arc::new(mock_settings());
        let state = AppState {
            oidc: Arc::new(OidcVerifier::new(settings.clone())),
            settings,
            health_registry: Arc::new(HealthRegistry::new()),
            hero_crud: Arc::new(crate::crud::CrudService::new(DynHeroRepository(Box::new(
                FaultyRepository,
            )))),
            rate_limiter: Arc::new(crate::rate_limit::RateLimiter::mock()),
            events: Arc::new(crate::events::EventBus::mock()),
        };
        router().with_state(state)
    }

    #[tokio::test]
    async fn form_page_maps_a_repository_failure_to_an_app_error_response() {
        let response = app_with_faulty_repository()
            .oneshot(
                Request::builder()
                    .uri(format!("/form?token={}", token("alice", &["viewer"])))
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
            .oneshot(with_addr(
                Request::builder()
                    .method("POST")
                    .uri(format!("/form?token={}", token("alice", &["editor"])))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Spectra&powers=flight&power_level=5"))
                    .unwrap(),
            ))
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
                    .uri("/form?token=not-a-real-token")
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
                    .uri(format!("/form?token={}", token("alice", &["viewer"])))
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
            .oneshot(with_addr(
                Request::builder()
                    .method("POST")
                    .uri(format!("/form?token={create_token}"))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Spectra&powers=flight&power_level=5"))
                    .unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::SEE_OTHER);

        let list = shared_app
            .oneshot(
                Request::builder()
                    .uri(format!("/form?token={}", token("alice", &["viewer"])))
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
            .oneshot(with_addr(
                Request::builder()
                    .method("POST")
                    .uri(format!("/form?token={}", token("alice", &["editor"])))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=&powers=&power_level="))
                    .unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn a_role_with_no_write_grant_is_forbidden_from_creating() {
        let response = app()
            .oneshot(with_addr(
                Request::builder()
                    .method("POST")
                    .uri(format!("/form?token={}", token("alice", &["viewer"])))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Spectra&powers=flight&power_level=5"))
                    .unwrap(),
            ))
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
            .oneshot(with_addr(
                Request::builder()
                    .method("POST")
                    .uri(format!("/form?token={owner_token}"))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Spectra&powers=flight&power_level=5"))
                    .unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::SEE_OTHER);

        let list = shared_app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/form?token={owner_token}"))
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
            .oneshot(with_addr(
                Request::builder()
                    .method("POST")
                    .uri(format!("/form/{id}/update?token={owner_token}"))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Renamed&powers=stealth&power_level=9"))
                    .unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(update.status(), StatusCode::SEE_OTHER);

        let after_update = shared_app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/form?token={owner_token}"))
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
            .oneshot(with_addr(
                Request::builder()
                    .method("POST")
                    .uri(format!("/form/{id}/delete?token={owner_token}"))
                    .body(Body::empty())
                    .unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(delete.status(), StatusCode::SEE_OTHER);

        let after_delete = shared_app
            .oneshot(
                Request::builder()
                    .uri(format!("/form?token={owner_token}"))
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
                    .uri(format!("/form?token={}", token("alice", &["security"])))
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
            .oneshot(with_addr(
                Request::builder()
                    .method("POST")
                    .uri("/form")
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Spectra&powers=flight&power_level=5"))
                    .unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn create_rejects_a_token_with_no_subject() {
        let response = app()
            .oneshot(with_addr(
                Request::builder()
                    .method("POST")
                    .uri(format!("/form?token={}", token_with_no_sub(&["editor"])))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Spectra&powers=flight&power_level=5"))
                    .unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn update_without_a_token_shows_the_bootstrap_page() {
        let response = app()
            .oneshot(with_addr(
                Request::builder()
                    .method("POST")
                    .uri("/form/1/update")
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Renamed&powers=stealth&power_level=9"))
                    .unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn update_is_forbidden_without_a_write_role() {
        let response = app()
            .oneshot(with_addr(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/form/1/update?token={}",
                        token("alice", &["viewer"])
                    ))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Renamed&powers=stealth&power_level=9"))
                    .unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn update_rejects_an_invalid_payload_with_422() {
        let response = app()
            .oneshot(with_addr(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/form/1/update?token={}",
                        token("alice", &["editor"])
                    ))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "name=Renamed&powers=stealth&power_level=not-a-number",
                    ))
                    .unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn update_rejects_a_token_with_no_subject() {
        let response = app()
            .oneshot(with_addr(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/form/1/update?token={}",
                        token_with_no_sub(&["editor"])
                    ))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Renamed&powers=stealth&power_level=9"))
                    .unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn update_of_a_nonexistent_id_returns_404() {
        let response = app()
            .oneshot(with_addr(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/form/999999/update?token={}",
                        token("alice", &["editor"])
                    ))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Renamed&powers=stealth&power_level=9"))
                    .unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_without_a_token_shows_the_bootstrap_page() {
        let response = app()
            .oneshot(with_addr(
                Request::builder()
                    .method("POST")
                    .uri("/form/1/delete")
                    .body(Body::empty())
                    .unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn delete_is_forbidden_without_the_maintainer_role() {
        let response = app()
            .oneshot(with_addr(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/form/1/delete?token={}",
                        token("alice", &["editor"])
                    ))
                    .body(Body::empty())
                    .unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn delete_rejects_a_token_with_no_subject() {
        let response = app()
            .oneshot(with_addr(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/form/1/delete?token={}",
                        token_with_no_sub(&["maintainer"])
                    ))
                    .body(Body::empty())
                    .unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn delete_of_a_nonexistent_id_returns_404() {
        let response = app()
            .oneshot(with_addr(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/form/999999/delete?token={}",
                        token("alice", &["maintainer"])
                    ))
                    .body(Body::empty())
                    .unwrap(),
            ))
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
            hero_crud: Arc::new(crate::crud::CrudService::new(DynHeroRepository(Box::new(
                HeroMemoryRepository::new(),
            )))),
            rate_limiter: Arc::new(crate::rate_limit::RateLimiter::mock()),
            events: Arc::new(crate::events::EventBus::mock()),
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
                .oneshot(with_addr(
                    Request::builder()
                        .method("POST")
                        .uri(format!("/form?token={create_token}"))
                        .header("Content-Type", "application/x-www-form-urlencoded")
                        .body(Body::from("name=Spectra&powers=flight&power_level=5"))
                        .unwrap(),
                ))
                .await
                .unwrap();
        }
        let response = shared_app
            .oneshot(with_addr(
                Request::builder()
                    .method("POST")
                    .uri(format!("/form?token={create_token}"))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Spectra&powers=flight&power_level=5"))
                    .unwrap(),
            ))
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
            .oneshot(with_addr(
                Request::builder()
                    .method("POST")
                    .uri(format!("/form?token={owner_token}"))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Spectra&powers=flight&power_level=5"))
                    .unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::SEE_OTHER);

        let response = shared_app
            .oneshot(with_addr(
                Request::builder()
                    .method("POST")
                    .uri(format!("/form/1/update?token={owner_token}"))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Renamed&powers=stealth&power_level=9"))
                    .unwrap(),
            ))
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
            .oneshot(with_addr(
                Request::builder()
                    .method("POST")
                    .uri(format!("/form?token={owner_token}"))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Spectra&powers=flight&power_level=5"))
                    .unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::SEE_OTHER);

        let response = shared_app
            .oneshot(with_addr(
                Request::builder()
                    .method("POST")
                    .uri(format!("/form/1/delete?token={owner_token}"))
                    .body(Body::empty())
                    .unwrap(),
            ))
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
}
