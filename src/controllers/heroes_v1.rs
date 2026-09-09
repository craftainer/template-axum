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

use std::collections::HashMap;
use std::net::SocketAddr;

use axum::extract::{ConnectInfo, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, TimeZone, Utc};

use crate::controllers::crud_actions::{DeleteOutcome, ListOrGet, UpdateOutcome};
use crate::controllers::crud_events;
use crate::controllers::crud_query::{parse_filters, parse_sort};
use crate::controllers::heroes::{HERO_FIELD_SPECS, HERO_WRITE_RATE_SCOPE};
use crate::controllers::{
    crud_actions, AppState, HERO_DELETE_ROLES, HERO_EVENT_RESOURCE, HERO_READ_ROLES,
    HERO_WRITE_ROLES,
};
use crate::crud::DEFAULT_LIMIT;
use crate::events::EventAction;
use crate::http_headers::Sunset;
use crate::oidc::AuthClaims;
use crate::problem_details::AppError;
use crate::views::hero::HeroListQuery;
use crate::views::hero_v1::{HeroCreateV1, HeroReadV1, HeroUpdateV1};

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

fn hero_read_v1_json(hero: crate::models::hero::Model) -> serde_json::Value {
    serde_json::to_value(HeroReadV1::from(hero)).expect("HeroReadV1 always serializes")
}

async fn list_or_get(
    State(state): State<AppState>,
    AuthClaims(claims): AuthClaims,
    Query(query): Query<HeroListQuery>,
    Query(raw_params): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    claims.require_any_role(&state.settings.oidc_client_id, HERO_READ_ROLES)?;
    let include_archived = query.include_archived.unwrap_or(false);
    let filters =
        parse_filters(HERO_FIELD_SPECS, &raw_params).map_err(AppError::UnprocessableEntity)?;
    let sort = parse_sort(HERO_FIELD_SPECS, &raw_params).map_err(AppError::UnprocessableEntity)?;
    let skip = query.skip.unwrap_or(0);
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT);

    let body = match crud_actions::resolve_list_or_get(
        &state.hero_crud,
        query.id,
        skip,
        limit,
        include_archived,
        filters,
        sort,
    )
    .await?
    {
        ListOrGet::One(hero) => Json(hero_read_v1_json(hero)),
        ListOrGet::Many(heroes) => {
            let heroes: Vec<HeroReadV1> = heroes.into_iter().map(HeroReadV1::from).collect();
            Json(serde_json::to_value(heroes).expect("Vec<HeroReadV1> always serializes"))
        }
    };
    Ok(with_sunset(CURRENT_VERSION_LINK, body))
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
    Query(raw_params): Query<HashMap<String, String>>,
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
    let owner_id = claims.subject()?.to_string();

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
            Json(hero_read_v1_json(hero))
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
        }
    };
    Ok(with_sunset(CURRENT_VERSION_LINK, body))
}

async fn delete_hero(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    AuthClaims(claims): AuthClaims,
    Query(query): Query<HeroListQuery>,
    Query(raw_params): Query<HashMap<String, String>>,
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
    let owner_id = claims.subject()?.to_string();

    let body = match crud_actions::resolve_delete(
        &state.hero_crud,
        query.id,
        &owner_id,
        filters,
        state.settings.bulk_action_max_matched,
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
}
