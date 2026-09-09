//! `/crud/v1/heroes/v1/xml` -- the XML sibling of the Hero v1 compat
//! router (`controllers::heroes_v1`), extending `docs/adrs/0014`'s
//! sibling-router pattern to the deprecated model version
//! (`docs/adrs/0017`, FR-0032). `views::hero_v1`'s flat shape (no nested
//! fields) is already XML-friendly, same as v2's. Reuses `controllers::
//! heroes_v1`'s `sunset_at`/`CURRENT_VERSION_LINK`-equivalent mechanism so
//! both v1 formats sunset together, and `controllers::heroes`'s
//! `HERO_FIELD_SPECS`/`HERO_WRITE_RATE_SCOPE` the same way every other
//! sibling router in this app does.

use std::collections::HashMap;
use std::net::SocketAddr;

use axum::body::Bytes;
use axum::extract::{ConnectInfo, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::controllers::crud_actions::{DeleteOutcome, ListOrGet, UpdateOutcome};
use crate::controllers::crud_events;
use crate::controllers::crud_query::{parse_filters, parse_sort};
use crate::controllers::heroes::{HERO_FIELD_SPECS, HERO_WRITE_RATE_SCOPE};
use crate::controllers::heroes_v1::sunset_at;
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
use crate::views::hero_v1::{HeroCreateV1, HeroUpdateV1};
use crate::views::hero_v1_xml::HeroReadV1Xml;
use crate::views::FieldError;

const CURRENT_VERSION_LINK: &str = "/crud/v1/heroes/v2/xml";

#[derive(Serialize)]
struct HeroListV1Xml {
    hero: Vec<HeroReadV1Xml>,
}

fn xml_response(root: &str, value: &impl Serialize) -> Result<Response, AppError> {
    let body = quick_xml::se::to_string_with_root(root, value)
        .map_err(|err| AppError::Internal(format!("xml encode: {err}")))?;
    Ok(([(header::CONTENT_TYPE, "application/xml")], body).into_response())
}

fn parse_xml_body<T: DeserializeOwned>(body: &Bytes) -> Result<T, AppError> {
    let text = std::str::from_utf8(body).map_err(|_| {
        AppError::UnprocessableEntity(vec![FieldError::new(
            "body",
            "request body is not valid UTF-8",
        )])
    })?;
    quick_xml::de::from_str(text).map_err(|err| {
        AppError::UnprocessableEntity(vec![FieldError::new("body", format!("invalid XML: {err}"))])
    })
}

fn with_sunset(response: Response) -> Response {
    (
        Sunset::new(sunset_at(), Some(CURRENT_VERSION_LINK)),
        response,
    )
        .into_response()
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

    let response = match crud_actions::resolve_list_or_get(
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
        ListOrGet::One(hero) => xml_response("hero", &HeroReadV1Xml::from(hero))?,
        ListOrGet::Many(heroes) => xml_response(
            "heroes",
            &HeroListV1Xml {
                hero: heroes.into_iter().map(HeroReadV1Xml::from).collect(),
            },
        )?,
    };
    Ok(with_sunset(response))
}

async fn create(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    AuthClaims(claims): AuthClaims,
    body: Bytes,
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
    let payload: HeroCreateV1 = parse_xml_body(&body)?;
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
    let response = xml_response("hero", &HeroReadV1Xml::from(hero))?;
    Ok(with_sunset((StatusCode::CREATED, response).into_response()))
}

async fn update(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    AuthClaims(claims): AuthClaims,
    Query(query): Query<HeroListQuery>,
    Query(raw_params): Query<HashMap<String, String>>,
    body: Bytes,
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
    let payload: HeroUpdateV1 = parse_xml_body(&body)?;
    payload.validate().map_err(AppError::UnprocessableEntity)?;
    let filters =
        parse_filters(HERO_FIELD_SPECS, &raw_params).map_err(AppError::UnprocessableEntity)?;
    let owner_id = claims.subject()?.to_string();

    let response = match crud_actions::resolve_update(
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
            xml_response("hero", &HeroReadV1Xml::from(hero))?
        }
        UpdateOutcome::Bulk(result) => {
            crud_events::publish(
                &state.events,
                HERO_EVENT_RESOURCE,
                EventAction::UpdateMany,
                result.ids.clone(),
            )
            .await;
            xml_response("bulk_update_result", &result)?
        }
    };
    Ok(with_sunset(response))
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

    let response = match crud_actions::resolve_delete(
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
            xml_response("bulk_delete_result", &result)?
        }
    };
    Ok(with_sunset(response))
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
    use crate::rate_limit::RateLimiter;
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
            rate_limiter: Arc::new(RateLimiter::mock()),
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

    fn authed(method: &str, uri: &str, sub: &str, roles: &[&str], body: &str) -> Request<Body> {
        let body = if body.is_empty() {
            Body::empty()
        } else {
            Body::from(body.to_string())
        };
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header("Authorization", format!("Bearer {}", token(sub, roles)))
            .header("Content-Type", "application/xml")
            .body(body)
            .unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
        request
    }

    async fn body_text(response: Response) -> String {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    const VALID_HERO_V1_XML: &str =
        "<hero><name>Spectra</name><superpower>flight</superpower><power_level>5</power_level></hero>";

    #[tokio::test]
    async fn create_returns_201_with_an_xml_body_and_sunset_headers() {
        let response = app()
            .oneshot(authed("POST", "/", "alice", &["editor"], VALID_HERO_V1_XML))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(response.headers().get("deprecation").unwrap(), "true");
        assert_eq!(
            response.headers().get("link").unwrap(),
            "</crud/v1/heroes/v2/xml>; rel=\"sunset\""
        );
        let body = body_text(response).await;
        assert!(body.contains("<superpower>flight</superpower>"));
    }

    #[tokio::test]
    async fn create_rejects_malformed_xml_with_422() {
        let response = app()
            .oneshot(authed("POST", "/", "alice", &["editor"], "<hero><name>"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn get_by_id_round_trips_through_xml() {
        let shared_app = app();
        let create = shared_app
            .clone()
            .oneshot(authed("POST", "/", "alice", &["editor"], VALID_HERO_V1_XML))
            .await
            .unwrap();
        let created = body_text(create).await;
        let id: i32 = created
            .split("<id>")
            .nth(1)
            .unwrap()
            .split("</id>")
            .next()
            .unwrap()
            .parse()
            .unwrap();

        let get = shared_app
            .oneshot(authed(
                "GET",
                &format!("/?id={id}"),
                "alice",
                &["viewer"],
                "",
            ))
            .await
            .unwrap();
        assert_eq!(get.status(), StatusCode::OK);
        let body = body_text(get).await;
        assert!(body.contains("<superpower>flight</superpower>"));
    }

    #[tokio::test]
    async fn a_role_with_no_write_grant_is_forbidden_from_creating() {
        let response = app()
            .oneshot(authed("POST", "/", "alice", &["viewer"], VALID_HERO_V1_XML))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
