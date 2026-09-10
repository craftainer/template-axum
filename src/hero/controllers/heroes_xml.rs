//! `/crud/v1/heroes/v2/xml` -- the XML sibling representation of Hero's
//! CRUD routes, alongside `controllers::heroes`'s JSON router
//! (`docs/adrs/0005`'s sibling-router-not-content-negotiation decision,
//! ported here as `docs/adrs/0014`). Shares the exact same `AppState`
//! (`hero_crud`, `rate_limiter`, `oidc`, `settings`) and the exact same
//! `crud_query`/`crud_actions` decision logic as the JSON router -- only
//! the request/response (de)serialization differs, so create/update/
//! delete/list/filter/sort/bulk behave identically regardless of which
//! sibling a caller uses.

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

use crate::crud::DEFAULT_LIMIT;
use crate::events::EventAction;
use crate::generic::controllers::crud_actions::{self, DeleteOutcome, ListOrGet, UpdateOutcome};
use crate::generic::controllers::crud_events;
use crate::generic::controllers::crud_query::{parse_filters, parse_sort};
use crate::generic::views::FieldError;
use crate::hero::controllers::heroes::{HERO_FIELD_SPECS, HERO_WRITE_RATE_SCOPE};
use crate::hero::controllers::{
    AppState, HERO_DELETE_ROLES, HERO_EVENT_RESOURCE, HERO_READ_ROLES, HERO_WRITE_ROLES,
};
use crate::hero::views::hero::{HeroCreate, HeroListQuery, HeroUpdate};
use crate::hero::views::hero_xml::HeroReadXml;
use crate::oidc::AuthClaims;
use crate::problem_details::AppError;

/// Wraps a list response as `<heroes><hero>...</hero>...</heroes>` --
/// `quick_xml`'s serde support repeats a `Vec` field using the field's own
/// name (see `views::hero_xml`'s own tests), so this is the same trick
/// used there, one level up.
#[derive(Serialize)]
struct HeroListXml {
    hero: Vec<HeroReadXml>,
}

fn xml_response(root: &str, value: &impl Serialize) -> Result<Response, AppError> {
    let body = quick_xml::se::to_string_with_root(root, value)
        .map_err(|err| AppError::Internal(format!("xml encode: {err}")))?;
    Ok(([(header::CONTENT_TYPE, "application/xml")], body).into_response())
}

/// Parse an XML request body -- `quick_xml`'s `Reader` (unlike the
/// stdlib `xml.etree.ElementTree` the Python reference's `xml_codec.py`
/// specifically avoids using directly) never expands a DTD-declared
/// custom entity: `quick_xml::escape::unescape` only resolves the five
/// predefined XML entities and numeric character references, with an
/// explicit nested-entity depth guard
/// (`EscapeError::TooManyNestedEntities`). The "billion laughs"
/// entity-expansion vector `defusedxml` exists to patch in the reference
/// isn't present in `quick_xml`'s design to begin with, so no separate
/// hardening crate is needed here (`docs/adrs/0014`).
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

    match crud_actions::resolve_list_or_get(
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
        ListOrGet::One(hero) => xml_response("hero", &HeroReadXml::from(hero)),
        ListOrGet::Many(heroes) => xml_response(
            "heroes",
            &HeroListXml {
                hero: heroes.into_iter().map(HeroReadXml::from).collect(),
            },
        ),
    }
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
    let payload: HeroCreate = parse_xml_body(&body)?;
    payload.validate().map_err(AppError::UnprocessableEntity)?;
    let owner_id = claims.subject()?.to_string();
    let hero = state.hero_crud.create(&owner_id, payload).await?;
    // Onto the same `crud-events/heroes` topic the JSON sibling
    // publishes to (docs/adrs/0016): a subscriber watches the *records*,
    // and which representation a writer happened to use is not something
    // it should have to care about.
    crud_events::publish(
        &state.events,
        HERO_EVENT_RESOURCE,
        EventAction::Create,
        vec![hero.id],
    )
    .await;
    let response = xml_response("hero", &HeroReadXml::from(hero))?;
    Ok((StatusCode::CREATED, response).into_response())
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
    let payload: HeroUpdate = parse_xml_body(&body)?;
    payload.validate().map_err(AppError::UnprocessableEntity)?;
    let filters =
        parse_filters(HERO_FIELD_SPECS, &raw_params).map_err(AppError::UnprocessableEntity)?;
    let owner_id = claims.subject()?.to_string();

    match crud_actions::resolve_update(
        &state.hero_crud,
        query.id,
        &owner_id,
        filters,
        payload,
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
            xml_response("hero", &HeroReadXml::from(hero))
        }
        UpdateOutcome::Bulk(result) => {
            crud_events::publish(
                &state.events,
                HERO_EVENT_RESOURCE,
                EventAction::UpdateMany,
                result.ids.clone(),
            )
            .await;
            xml_response("bulk_update_result", &result)
        }
    }
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

    match crud_actions::resolve_delete(
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
            Ok(StatusCode::NO_CONTENT.into_response())
        }
        DeleteOutcome::Bulk(result) => {
            crud_events::publish(
                &state.events,
                HERO_EVENT_RESOURCE,
                EventAction::DeleteMany,
                result.ids.clone(),
            )
            .await;
            xml_response("bulk_delete_result", &result)
        }
    }
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
    use crate::health::HealthRegistry;
    use crate::hero::controllers::DynHeroRepository;
    use crate::hero::repositories::hero_memory::HeroMemoryRepository;
    use crate::oidc::OidcVerifier;
    use crate::rate_limit::RateLimiter;
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

    const VALID_HERO_XML: &str =
        "<hero><name>Spectra</name><powers>flight</powers><power_level>5</power_level></hero>";

    // NFR-0026: this current-version (/v2) XML router carries none of the
    // deprecation headers `controllers::heroes_v1_xml` attaches.
    #[tokio::test]
    async fn current_version_responses_carry_no_deprecation_headers() {
        let response = app()
            .oneshot(authed("GET", "/", "alice", &["viewer"], ""))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get("deprecation").is_none());
        assert!(response.headers().get("sunset").is_none());
        assert!(response.headers().get("link").is_none());
    }

    #[tokio::test]
    async fn create_returns_201_with_an_xml_body() {
        let response = app()
            .oneshot(authed("POST", "/", "alice", &["editor"], VALID_HERO_XML))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/xml"
        );
        let body = body_text(response).await;
        assert!(body.contains("<name>Spectra</name>"));
        assert!(body.contains("<owner_id>alice</owner_id>"));
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
    async fn create_rejects_an_invalid_payload_with_422() {
        let response = app()
            .oneshot(authed(
                "POST",
                "/",
                "alice",
                &["editor"],
                "<hero><name></name></hero>",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn create_rejects_a_non_utf8_body_with_422() {
        let mut request = Request::builder()
            .method("POST")
            .uri("/")
            .header(
                "Authorization",
                format!("Bearer {}", token("alice", &["editor"])),
            )
            .header("Content-Type", "application/xml")
            .body(Body::from(vec![0xff, 0xfe, 0xfd]))
            .unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
        let response = app().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn get_by_id_and_list_round_trip_through_xml() {
        let shared_app = app();
        let create = shared_app
            .clone()
            .oneshot(authed("POST", "/", "alice", &["editor"], VALID_HERO_XML))
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
            .clone()
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
        assert!(body.starts_with("<hero>"));

        let list = shared_app
            .oneshot(authed("GET", "/", "alice", &["viewer"], ""))
            .await
            .unwrap();
        assert_eq!(list.status(), StatusCode::OK);
        let body = body_text(list).await;
        assert!(body.starts_with("<heroes>"));
        assert_eq!(body.matches("<hero>").count(), 1);
    }

    #[tokio::test]
    async fn get_missing_id_returns_404() {
        let response = app()
            .oneshot(authed("GET", "/?id=999999", "alice", &["viewer"], ""))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn update_by_id_returns_the_updated_record() {
        let shared_app = app();
        let create = shared_app
            .clone()
            .oneshot(authed("POST", "/", "alice", &["editor"], VALID_HERO_XML))
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

        let response = shared_app
            .oneshot(authed(
                "PATCH",
                &format!("/?id={id}"),
                "alice",
                &["editor"],
                "<hero><name>Renamed</name></hero>",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_text(response).await;
        assert!(body.contains("<name>Renamed</name>"));
    }

    #[tokio::test]
    async fn bulk_update_and_delete_over_filters_return_a_bulk_result() {
        let shared_app = app();
        shared_app
            .clone()
            .oneshot(authed("POST", "/", "alice", &["editor"], VALID_HERO_XML))
            .await
            .unwrap();

        let bulk_update = shared_app
            .clone()
            .oneshot(authed(
                "PATCH",
                "/?power_level=5",
                "alice",
                &["editor"],
                "<hero><power_level>9</power_level></hero>",
            ))
            .await
            .unwrap();
        assert_eq!(bulk_update.status(), StatusCode::OK);
        let body = body_text(bulk_update).await;
        assert!(body.contains("<matched>1</matched>"));

        let bulk_delete = shared_app
            .oneshot(authed(
                "DELETE",
                "/?power_level=9",
                "alice",
                &["maintainer"],
                "",
            ))
            .await
            .unwrap();
        assert_eq!(bulk_delete.status(), StatusCode::OK);
        let body = body_text(bulk_delete).await;
        assert!(body.contains("<matched>1</matched>"));
    }

    #[tokio::test]
    async fn delete_by_id_returns_204() {
        let shared_app = app();
        let create = shared_app
            .clone()
            .oneshot(authed(
                "POST",
                "/",
                "alice",
                &["maintainer"],
                VALID_HERO_XML,
            ))
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

        let response = shared_app
            .oneshot(authed(
                "DELETE",
                &format!("/?id={id}"),
                "alice",
                &["maintainer"],
                "",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn a_role_with_no_write_grant_is_forbidden_from_creating() {
        let response = app()
            .oneshot(authed("POST", "/", "alice", &["viewer"], VALID_HERO_XML))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
