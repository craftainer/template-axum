//! `/crud/v1/heroes/v1/json` -- the deprecated Hero v1 compat router
//! (`docs/adrs/0017`, FR-0031/FR-0032, ported from the reference's
//! `crud_1/heroes/heroes_v1.py`). Backed by the exact same `HeroSeaOrmRepository`/
//! `HeroMemoryRepository` v2 data `controllers::heroes` serves -- there is
//! no separate v1 table or repository, only a lossy DTO conversion at the
//! boundary (`views::hero_v1`) and this router reusing `controllers::
//! heroes`'s own `HERO_FIELD_SPECS`/`HERO_WRITE_RATE_SCOPE`/role sets and
//! `crud_actions`'s decision logic, the same way `controllers::heroes_xml`
//! does for its own sibling relationship (`docs/adrs/0014`).
//!
//! Every route here attaches RFC 8594 `Sunset`/`Deprecation`/`Link`
//! headers via `http_headers::Sunset` (`docs/adrs/0012`'s mechanism,
//! applied for the first time -- NFR-0026), pointing at the current-
//! version JSON equivalent.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, TimeZone, Utc};

use crate::hero::controllers::heroes::{HERO_FIELD_SPECS, HERO_WRITE_RATE_SCOPE};
use crate::hero::controllers::{
    enforce_if_match, read_etag, AppState, HERO_DELETE_ROLES, HERO_EVENT_RESOURCE, HERO_READ_ROLES,
    HERO_WRITE_ROLES,
};
use crate::hero::views::hero::HeroListQuery;
use crate::hero::views::hero_v1::{HeroCreateV1, HeroReadV1, HeroUpdateV1};
use crate::http_headers::Sunset;
use crate::oidc::AuthClaims;
use crate::problem_details::AppError;
use craftainer_crud::conditional::{not_modified, record_etag, revalidation_headers};
use craftainer_crud::controllers::crud_actions::{self, DeleteOutcome, ListOrGet, UpdateOutcome};
use craftainer_crud::controllers::crud_events;
use craftainer_crud::controllers::crud_query::{
    parse_archived_scope, parse_bulk_delete_include_unpublished, parse_filters, parse_sort,
    reject_only_archived,
};
use craftainer_crud::crud::DEFAULT_LIMIT;
use craftainer_crud::events::EventAction;

/// The current-version equivalent every v1 response's `Link: ...;
/// rel="sunset"` header points at (NFR-0026).
const CURRENT_VERSION_LINK: &str = "/crud/v1/heroes/v2/json";

/// When the v1 compat router stops being served. Shared with `controllers::
/// heroes_v1_xml`, both formats sunset together. Matches the date `src/
/// http_headers.rs`'s own tests were written against.
pub(crate) fn sunset_at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).unwrap()
}

fn with_sunset(link: &'static str, response: impl IntoResponse) -> Response {
    (Sunset::new(sunset_at(), Some(link)), response).into_response()
}

fn hero_read_v1_json(hero: crate::hero::models::hero::Model) -> serde_json::Value {
    serde_json::to_value(HeroReadV1::from(hero)).expect("HeroReadV1 always serializes")
}

async fn list_or_get(
    State(state): State<AppState>,
    AuthClaims(claims): AuthClaims,
    Query(query): Query<HeroListQuery>,
    Query(raw_params): Query<Vec<(String, String)>>,
    request_headers: HeaderMap,
) -> Result<Response, AppError> {
    claims.require_any_role(&state.settings.oidc_client_id, HERO_READ_ROLES)?;
    let archived = parse_archived_scope(&raw_params).map_err(AppError::UnprocessableEntity)?;
    let filters =
        parse_filters(HERO_FIELD_SPECS, &raw_params).map_err(AppError::UnprocessableEntity)?;
    let sort = parse_sort(HERO_FIELD_SPECS, &raw_params).map_err(AppError::UnprocessableEntity)?;
    let skip = query.skip.unwrap_or(0);
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT);

    let result = crud_actions::resolve_list_or_get(
        &state.hero_crud,
        query.id,
        skip,
        limit,
        archived,
        filters,
        sort,
    )
    .await?;
    let etag = read_etag(&result);
    if let Some(unchanged) = not_modified(&request_headers, &etag) {
        return Ok(with_sunset(CURRENT_VERSION_LINK, unchanged));
    }
    let body = match result {
        ListOrGet::One(hero) => Json(hero_read_v1_json(hero)),
        ListOrGet::Many(heroes) => {
            let heroes: Vec<HeroReadV1> = heroes.into_iter().map(HeroReadV1::from).collect();
            Json(serde_json::to_value(heroes).expect("Vec<HeroReadV1> always serializes"))
        }
    };
    Ok(with_sunset(
        CURRENT_VERSION_LINK,
        (revalidation_headers(&etag), body),
    ))
}

async fn create(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    AuthClaims(claims): AuthClaims,
    Json(payload): Json<HeroCreateV1>,
) -> Result<Response, AppError> {
    state
        .rate_limiter
        .check(
            HERO_WRITE_RATE_SCOPE,
            addr.ip(),
            state.settings.rate_limit_hero_write_per_minute,
            60,
        )
        .await?;
    claims.require_any_role(&state.settings.oidc_client_id, HERO_WRITE_ROLES)?;
    payload.validate().map_err(AppError::UnprocessableEntity)?;
    let owner_id = claims.subject()?.to_string();
    let hero = state.hero_crud.create(&owner_id, payload.into()).await?;
    crud_events::publish(
        &state.events,
        HERO_EVENT_RESOURCE,
        EventAction::Create,
        vec![hero.id],
    )
    .await;
    Ok(with_sunset(
        CURRENT_VERSION_LINK,
        (StatusCode::CREATED, Json(HeroReadV1::from(hero))),
    ))
}

async fn update(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    AuthClaims(claims): AuthClaims,
    Query(query): Query<HeroListQuery>,
    Query(raw_params): Query<Vec<(String, String)>>,
    request_headers: HeaderMap,
    Json(payload): Json<HeroUpdateV1>,
) -> Result<Response, AppError> {
    state
        .rate_limiter
        .check(
            HERO_WRITE_RATE_SCOPE,
            addr.ip(),
            state.settings.rate_limit_hero_write_per_minute,
            60,
        )
        .await?;
    claims.require_any_role(&state.settings.oidc_client_id, HERO_WRITE_ROLES)?;
    payload.validate().map_err(AppError::UnprocessableEntity)?;
    let filters =
        parse_filters(HERO_FIELD_SPECS, &raw_params).map_err(AppError::UnprocessableEntity)?;
    reject_only_archived(&raw_params).map_err(AppError::UnprocessableEntity)?;
    let owner_id = claims.subject()?.to_string();
    enforce_if_match(&state, &request_headers, query.id, &raw_params).await?;

    let body = match crud_actions::resolve_update(
        &state.hero_crud,
        query.id,
        &owner_id,
        filters,
        payload.into(),
        state.settings.bulk_action_max_matched,
    )
    .await?
    {
        UpdateOutcome::One(hero) => {
            crud_events::publish(
                &state.events,
                HERO_EVENT_RESOURCE,
                EventAction::Update,
                vec![hero.id],
            )
            .await;
            (
                [(header::ETAG, record_etag(&hero))],
                Json(hero_read_v1_json(hero)),
            )
                .into_response()
        }
        UpdateOutcome::Bulk(result) => {
            crud_events::publish(
                &state.events,
                HERO_EVENT_RESOURCE,
                EventAction::UpdateMany,
                result.ids.clone(),
            )
            .await;
            Json(serde_json::to_value(result).expect("BulkUpdateResult always serializes"))
                .into_response()
        }
    };
    Ok(with_sunset(CURRENT_VERSION_LINK, body))
}

async fn delete_hero(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    AuthClaims(claims): AuthClaims,
    Query(query): Query<HeroListQuery>,
    Query(raw_params): Query<Vec<(String, String)>>,
    request_headers: HeaderMap,
) -> Result<Response, AppError> {
    state
        .rate_limiter
        .check(
            HERO_WRITE_RATE_SCOPE,
            addr.ip(),
            state.settings.rate_limit_hero_write_per_minute,
            60,
        )
        .await?;
    claims.require_any_role(&state.settings.oidc_client_id, HERO_DELETE_ROLES)?;
    let filters =
        parse_filters(HERO_FIELD_SPECS, &raw_params).map_err(AppError::UnprocessableEntity)?;
    reject_only_archived(&raw_params).map_err(AppError::UnprocessableEntity)?;
    let include_unpublished = parse_bulk_delete_include_unpublished(&raw_params)
        .map_err(AppError::UnprocessableEntity)?;
    let owner_id = claims.subject()?.to_string();
    enforce_if_match(&state, &request_headers, query.id, &raw_params).await?;

    let body = match crud_actions::resolve_delete(
        &state.hero_crud,
        query.id,
        &owner_id,
        filters,
        state.settings.bulk_action_max_matched,
        include_unpublished,
    )
    .await?
    {
        DeleteOutcome::One => {
            let ids = query.id.into_iter().collect();
            crud_events::publish(&state.events, HERO_EVENT_RESOURCE, EventAction::Delete, ids)
                .await;
            StatusCode::NO_CONTENT.into_response()
        }
        DeleteOutcome::Bulk(result) => {
            crud_events::publish(
                &state.events,
                HERO_EVENT_RESOURCE,
                EventAction::DeleteMany,
                result.ids.clone(),
            )
            .await;
            Json(result).into_response()
        }
    };
    Ok(with_sunset(CURRENT_VERSION_LINK, body))
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/",
        get(list_or_get)
            .post(create)
            .patch(update)
            .delete(delete_hero),
    )
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

    fn authed(
        method: &str,
        uri: &str,
        sub: &str,
        roles: &[&str],
        body: serde_json::Value,
    ) -> Request<Body> {
        let body = if body.is_null() {
            Body::empty()
        } else {
            Body::from(body.to_string())
        };
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header("Authorization", format!("Bearer {}", token(sub, roles)))
            .header("Content-Type", "application/json")
            .body(body)
            .unwrap();
        request
            .extensions_mut()
            .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                [127, 0, 0, 1],
                12345,
            ))));
        request
    }

    async fn json_body(response: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    const VALID_HERO_V1: &str = r#"{"name":"Spectra","superpower":"flight","power_level":5}"#;

    #[tokio::test]
    async fn create_wraps_superpower_into_powers_and_returns_201() {
        let response = app()
            .oneshot(authed(
                "POST",
                "/",
                "alice",
                &["editor"],
                serde_json::from_str(VALID_HERO_V1).unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = json_body(response).await;
        assert_eq!(body["superpower"], "flight");
        assert!(body.get("powers").is_none());
    }

    #[tokio::test]
    async fn every_response_carries_sunset_deprecation_and_link_headers() {
        let response = app()
            .oneshot(authed(
                "GET",
                "/",
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers().get("deprecation").unwrap(), "true");
        assert!(response.headers().get("sunset").is_some());
        assert_eq!(
            response.headers().get("link").unwrap(),
            "</crud/v1/heroes/v2/json>; rel=\"sunset\""
        );
    }

    #[tokio::test]
    async fn update_maps_superpower_without_clobbering_when_omitted() {
        let shared_app = app();
        let create = shared_app
            .clone()
            .oneshot(authed(
                "POST",
                "/",
                "alice",
                &["editor"],
                serde_json::from_str(VALID_HERO_V1).unwrap(),
            ))
            .await
            .unwrap();
        let created = json_body(create).await;
        let id = created["id"].as_i64().unwrap();

        let response = shared_app
            .oneshot(authed(
                "PATCH",
                &format!("/?id={id}"),
                "alice",
                &["editor"],
                serde_json::json!({"name": "Renamed"}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["name"], "Renamed");
        assert_eq!(
            body["superpower"], "flight",
            "an omitted superpower must not clobber the existing power"
        );
    }

    #[tokio::test]
    async fn a_role_with_no_write_grant_is_forbidden_from_creating() {
        let response = app()
            .oneshot(authed(
                "POST",
                "/",
                "alice",
                &["viewer"],
                serde_json::from_str(VALID_HERO_V1).unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn get_missing_id_returns_404() {
        let response = app()
            .oneshot(authed(
                "GET",
                "/?id=999999",
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn only_maintainer_can_delete() {
        let create = app()
            .oneshot(authed(
                "POST",
                "/",
                "alice",
                &["maintainer"],
                serde_json::from_str(VALID_HERO_V1).unwrap(),
            ))
            .await
            .unwrap();
        let created = json_body(create).await;
        let id = created["id"].as_i64().unwrap();

        let response = app()
            .oneshot(authed(
                "DELETE",
                &format!("/?id={id}"),
                "alice",
                &["editor"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    async fn create_hero_v1(app: &Router, sub: &str) -> i64 {
        let response = app
            .clone()
            .oneshot(authed(
                "POST",
                "/",
                sub,
                &["editor"],
                serde_json::from_str(VALID_HERO_V1).unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        json_body(response).await["id"].as_i64().unwrap()
    }

    #[tokio::test]
    async fn get_by_id_returns_a_single_hero() {
        let shared_app = app();
        let id = create_hero_v1(&shared_app, "alice").await;

        let response = shared_app
            .oneshot(authed(
                "GET",
                &format!("/?id={id}"),
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["superpower"], "flight");
    }

    #[tokio::test]
    async fn update_by_id_returns_the_updated_hero() {
        let shared_app = app();
        let id = create_hero_v1(&shared_app, "alice").await;

        let response = shared_app
            .oneshot(authed(
                "PATCH",
                &format!("/?id={id}"),
                "alice",
                &["editor"],
                serde_json::json!({"superpower": "stealth"}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["superpower"], "stealth");
    }

    #[tokio::test]
    async fn bulk_update_over_filters_returns_a_bulk_result() {
        let shared_app = app();
        create_hero_v1(&shared_app, "alice").await;
        create_hero_v1(&shared_app, "alice").await;

        let response = shared_app
            .oneshot(authed(
                "PATCH",
                "/?power_level=5",
                "alice",
                &["editor"],
                serde_json::json!({"superpower": "stealth"}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["matched"], 2);
    }

    #[tokio::test]
    async fn delete_by_id_returns_204() {
        let shared_app = app();
        let id = create_hero_v1(&shared_app, "alice").await;

        let response = shared_app
            .oneshot(authed(
                "DELETE",
                &format!("/?id={id}"),
                "alice",
                &["maintainer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn bulk_delete_over_filters_returns_a_bulk_result() {
        let shared_app = app();
        create_hero_v1(&shared_app, "alice").await;
        create_hero_v1(&shared_app, "alice").await;

        let response = shared_app
            .oneshot(authed(
                "DELETE",
                "/?power_level=5",
                "alice",
                &["maintainer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["matched"], 2);
    }

    // -- conditional requests (docs/adrs/0023): ETag + If-None-Match -> 304
    // on reads, If-Match -> 412 on a single-record write. --

    fn etag(response: &Response) -> String {
        response.headers()[header::ETAG]
            .to_str()
            .unwrap()
            .to_string()
    }

    async fn call(
        app: &Router,
        method: &str,
        uri: &str,
        roles: &[&str],
        body: &str,
        headers: &[(&str, &str)],
    ) -> Response {
        let mut request = {
            let body = if body.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::from_str(body).unwrap()
            };
            authed(method, uri, "alice", roles, body)
        };
        for (name, value) in headers {
            request.headers_mut().insert(
                axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                value.parse().unwrap(),
            );
        }
        app.clone().oneshot(request).await.unwrap()
    }

    async fn text(response: Response) -> String {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    async fn cond_create(app: &Router) -> i64 {
        let response = call(app, "POST", "/", &["editor"], VALID_HERO_V1, &[]).await;
        assert_eq!(response.status(), StatusCode::CREATED);
        serde_json::from_str::<serde_json::Value>(&text(response).await).unwrap()["id"]
            .as_i64()
            .unwrap()
    }

    fn rename_body(to: &str) -> String {
        format!(r#"{{"name":"{to}"}}"#)
    }

    async fn rename(app: &Router, id: i64, to: &str, headers: &[(&str, &str)]) -> Response {
        call(
            app,
            "PATCH",
            &format!("/?id={id}"),
            &["editor"],
            &rename_body(to),
            headers,
        )
        .await
    }

    #[tokio::test]
    async fn get_by_id_carries_a_strong_etag_and_revalidates_with_304() {
        let app = app();
        let id = cond_create(&app).await;
        let uri = format!("/?id={id}");

        let first = call(&app, "GET", &uri, &["viewer"], "", &[]).await;
        assert_eq!(first.status(), StatusCode::OK);
        let tag = etag(&first);
        assert!(tag.starts_with('"'), "a record tag is strong: {tag}");
        assert_eq!(first.headers()[header::CACHE_CONTROL], "private, no-cache");

        let second = call(
            &app,
            "GET",
            &uri,
            &["viewer"],
            "",
            &[("if-none-match", &tag)],
        )
        .await;
        assert_eq!(second.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(etag(&second), tag);
        assert_eq!(second.headers()[header::CACHE_CONTROL], "private, no-cache");
        assert_eq!(second.headers()["deprecation"], "true");
        assert!(second.headers().get("sunset").is_some());
        assert!(text(second).await.is_empty());

        let weak = format!("W/{tag}");
        let third = call(
            &app,
            "GET",
            &uri,
            &["viewer"],
            "",
            &[("if-none-match", &weak)],
        )
        .await;
        assert_eq!(third.status(), StatusCode::NOT_MODIFIED, "weak comparison");
        let star = call(
            &app,
            "GET",
            &uri,
            &["viewer"],
            "",
            &[("if-none-match", "*")],
        )
        .await;
        assert_eq!(star.status(), StatusCode::NOT_MODIFIED);
    }

    #[tokio::test]
    async fn a_stale_if_none_match_gets_the_full_response() {
        let app = app();
        let id = cond_create(&app).await;
        let response = call(
            &app,
            "GET",
            &format!("/?id={id}"),
            &["viewer"],
            "",
            &[("if-none-match", "\"stale\"")],
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!text(response).await.is_empty());
    }

    #[tokio::test]
    async fn a_list_carries_a_weak_etag_that_moves_with_a_write() {
        let app = app();
        let id = cond_create(&app).await;

        let first = call(&app, "GET", "/", &["viewer"], "", &[]).await;
        assert_eq!(first.status(), StatusCode::OK);
        let tag = etag(&first);
        assert!(tag.starts_with("W/\""), "a page tag is weak: {tag}");
        assert_eq!(first.headers()[header::CACHE_CONTROL], "private, no-cache");

        let unchanged = call(
            &app,
            "GET",
            "/",
            &["viewer"],
            "",
            &[("if-none-match", &tag)],
        )
        .await;
        assert_eq!(unchanged.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(etag(&unchanged), tag);

        assert_eq!(
            rename(&app, id, "Renamed", &[]).await.status(),
            StatusCode::OK
        );
        let changed = call(
            &app,
            "GET",
            "/",
            &["viewer"],
            "",
            &[("if-none-match", &tag)],
        )
        .await;
        assert_eq!(changed.status(), StatusCode::OK);
        assert_ne!(etag(&changed), tag);
    }

    #[tokio::test]
    async fn patch_by_id_answers_with_the_records_new_etag() {
        let app = app();
        let id = cond_create(&app).await;
        let uri = format!("/?id={id}");
        let before = etag(&call(&app, "GET", &uri, &["viewer"], "", &[]).await);

        let patched = rename(&app, id, "Renamed", &[]).await;
        assert_eq!(patched.status(), StatusCode::OK);
        let after = etag(&patched);
        assert_ne!(after, before, "an update changes the tag");

        let read = call(&app, "GET", &uri, &["viewer"], "", &[]).await;
        assert_eq!(
            etag(&read),
            after,
            "the PATCH tag is the tag a GET now serves"
        );
        let stale = call(
            &app,
            "GET",
            &uri,
            &["viewer"],
            "",
            &[("if-none-match", &before)],
        )
        .await;
        assert_eq!(
            stale.status(),
            StatusCode::OK,
            "the old tag no longer revalidates"
        );
    }

    #[tokio::test]
    async fn a_stale_if_match_on_patch_is_412_and_writes_nothing() {
        let app = app();
        let id = cond_create(&app).await;
        let uri = format!("/?id={id}");
        let stale = etag(&call(&app, "GET", &uri, &["viewer"], "", &[]).await);
        assert_eq!(
            rename(&app, id, "Renamed", &[]).await.status(),
            StatusCode::OK
        );

        let refused = rename(&app, id, "Clobbered", &[("if-match", &stale)]).await;
        assert_eq!(refused.status(), StatusCode::PRECONDITION_FAILED);
        assert_eq!(
            refused.headers()[header::CONTENT_TYPE],
            "application/problem+json"
        );
        let problem = text(refused).await;
        assert!(problem.contains("Hero was modified since the supplied If-Match"));

        let read = text(call(&app, "GET", &uri, &["viewer"], "", &[]).await).await;
        assert!(read.contains("Renamed") && !read.contains("Clobbered"));
    }

    #[tokio::test]
    async fn patch_without_if_match_or_with_a_current_tag_or_star_succeeds() {
        let app = app();
        let id = cond_create(&app).await;
        let uri = format!("/?id={id}");

        assert_eq!(
            rename(&app, id, "One", &[]).await.status(),
            StatusCode::OK,
            "absent header"
        );
        let tag = etag(&call(&app, "GET", &uri, &["viewer"], "", &[]).await);
        let current = rename(&app, id, "Two", &[("if-match", &tag)]).await;
        assert_eq!(current.status(), StatusCode::OK, "current tag");
        let star = rename(&app, id, "Three", &[("if-match", "*")]).await;
        assert_eq!(star.status(), StatusCode::OK, "star");

        let tag = etag(&star);
        let weak = format!("W/{tag}");
        let refused = rename(&app, id, "Four", &[("if-match", &weak)]).await;
        assert_eq!(
            refused.status(),
            StatusCode::PRECONDITION_FAILED,
            "If-Match compares strongly, so a weak tag never matches"
        );
    }

    #[tokio::test]
    async fn a_stale_if_match_on_delete_is_412_and_deletes_nothing() {
        let app = app();
        let id = cond_create(&app).await;
        let uri = format!("/?id={id}");
        let stale = etag(&call(&app, "GET", &uri, &["viewer"], "", &[]).await);
        assert_eq!(
            rename(&app, id, "Renamed", &[]).await.status(),
            StatusCode::OK
        );

        let refused = call(
            &app,
            "DELETE",
            &uri,
            &["maintainer"],
            "",
            &[("if-match", &stale)],
        )
        .await;
        assert_eq!(refused.status(), StatusCode::PRECONDITION_FAILED);
        let still_there = call(&app, "GET", &uri, &["viewer"], "", &[]).await;
        assert_eq!(still_there.status(), StatusCode::OK);

        let current = etag(&still_there);
        let deleted = call(
            &app,
            "DELETE",
            &uri,
            &["maintainer"],
            "",
            &[("if-match", &current)],
        )
        .await;
        assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
        let gone = call(&app, "GET", &uri, &["viewer"], "", &[]).await;
        assert_eq!(gone.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_without_if_match_still_succeeds() {
        let app = app();
        let id = cond_create(&app).await;
        let deleted = call(
            &app,
            "DELETE",
            &format!("/?id={id}"),
            &["maintainer"],
            "",
            &[],
        )
        .await;
        assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn bulk_writes_ignore_if_match() {
        let app = app();
        cond_create(&app).await;
        let bulk_patch = call(
            &app,
            "PATCH",
            "/?power_level=5",
            &["editor"],
            &rename_body("Bulk"),
            &[("if-match", "\"stale\"")],
        )
        .await;
        assert_eq!(bulk_patch.status(), StatusCode::OK);
        assert!(bulk_patch.headers().get(header::ETAG).is_none());

        let bulk_delete = call(
            &app,
            "DELETE",
            "/?power_level=5",
            &["maintainer"],
            "",
            &[("if-match", "\"stale\"")],
        )
        .await;
        assert_eq!(bulk_delete.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn if_match_on_a_missing_record_falls_through_to_404() {
        let app = app();
        let patch = rename(&app, 9999, "Renamed", &[("if-match", "\"stale\"")]).await;
        assert_eq!(patch.status(), StatusCode::NOT_FOUND);
        let delete = call(
            &app,
            "DELETE",
            "/?id=9999",
            &["maintainer"],
            "",
            &[("if-match", "\"stale\"")],
        )
        .await;
        assert_eq!(delete.status(), StatusCode::NOT_FOUND);
    }
}
