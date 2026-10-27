//! `/crud/v1/heroes/v1/xml` -- the XML sibling of the Hero v1 compat
//! router (`controllers::heroes_v1`), extending `docs/adrs/0014`'s
//! sibling-router pattern to the deprecated model version
//! (`docs/adrs/0017`, FR-0032). `views::hero_v1`'s flat shape (no nested
//! fields) is already XML-friendly, same as v2's. Reuses `controllers::
//! heroes_v1`'s `sunset_at`/`CURRENT_VERSION_LINK`-equivalent mechanism so
//! both v1 formats sunset together, and `controllers::heroes`'s
//! `HERO_FIELD_SPECS`/`HERO_WRITE_RATE_SCOPE` the same way every other
//! sibling router in this app does.

use std::net::SocketAddr;

use axum::body::Bytes;
use axum::extract::{ConnectInfo, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::hero::controllers::heroes::{HERO_FIELD_SPECS, HERO_WRITE_RATE_SCOPE};
use crate::hero::controllers::heroes_v1::sunset_at;
use crate::hero::controllers::{
    enforce_if_match, read_etag, AppState, HERO_DELETE_ROLES, HERO_EVENT_RESOURCE, HERO_READ_ROLES,
    HERO_WRITE_ROLES,
};
use crate::hero::views::hero::HeroListQuery;
use crate::hero::views::hero_v1::{HeroCreateV1, HeroUpdateV1};
use crate::hero::views::hero_v1_xml::HeroReadV1Xml;
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
use craftainer_crud::views::FieldError;

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
        return Ok(with_sunset(unchanged));
    }
    let response = match result {
        ListOrGet::One(hero) => xml_response("hero", &HeroReadV1Xml::from(hero))?,
        ListOrGet::Many(heroes) => xml_response(
            "heroes",
            &HeroListV1Xml {
                hero: heroes.into_iter().map(HeroReadV1Xml::from).collect(),
            },
        )?,
    };
    Ok(with_sunset(
        (revalidation_headers(&etag), response).into_response(),
    ))
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
    Query(raw_params): Query<Vec<(String, String)>>,
    request_headers: HeaderMap,
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
    reject_only_archived(&raw_params).map_err(AppError::UnprocessableEntity)?;
    let owner_id = claims.subject()?.to_string();
    enforce_if_match(&state, &request_headers, query.id, &raw_params).await?;

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
            let etag = record_etag(&hero);
            (
                [(header::ETAG, etag)],
                xml_response("hero", &HeroReadV1Xml::from(hero))?,
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

    let response = match crud_actions::resolve_delete(
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
    use crate::hero::controllers::DynHeroRepository;
    use crate::hero::repositories::hero_memory::HeroMemoryRepository;
    use crate::oidc::OidcVerifier;
    use crate::rate_limit::RateLimiter;
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
            rate_limiter: Arc::new(RateLimiter::mock()),
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
    async fn list_returns_every_hero_wrapped_in_a_heroes_root() {
        let shared_app = app();
        shared_app
            .clone()
            .oneshot(authed("POST", "/", "alice", &["editor"], VALID_HERO_V1_XML))
            .await
            .unwrap();
        shared_app
            .clone()
            .oneshot(authed("POST", "/", "alice", &["editor"], VALID_HERO_V1_XML))
            .await
            .unwrap();

        let response = shared_app
            .oneshot(authed("GET", "/", "alice", &["viewer"], ""))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_text(response).await;
        assert!(body.starts_with("<heroes>"), "{body}");
        assert_eq!(body.matches("<hero>").count(), 2, "{body}");
    }

    #[tokio::test]
    async fn update_by_id_returns_the_updated_record() {
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

        let response = shared_app
            .oneshot(authed(
                "PATCH",
                &format!("/?id={id}"),
                "alice",
                &["editor"],
                "<hero><superpower>stealth</superpower></hero>",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_text(response).await;
        assert!(body.contains("<superpower>stealth</superpower>"), "{body}");
    }

    #[tokio::test]
    async fn bulk_update_over_filters_returns_a_bulk_result() {
        let shared_app = app();
        shared_app
            .clone()
            .oneshot(authed("POST", "/", "alice", &["editor"], VALID_HERO_V1_XML))
            .await
            .unwrap();
        shared_app
            .clone()
            .oneshot(authed("POST", "/", "alice", &["editor"], VALID_HERO_V1_XML))
            .await
            .unwrap();

        let response = shared_app
            .oneshot(authed(
                "PATCH",
                "/?power_level=5",
                "alice",
                &["editor"],
                "<hero><superpower>stealth</superpower></hero>",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_text(response).await;
        assert!(body.contains("<bulk_update_result>"), "{body}");
        assert!(body.contains("<matched>2</matched>"), "{body}");
    }

    #[tokio::test]
    async fn delete_by_id_returns_204() {
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
    async fn bulk_delete_over_filters_returns_a_bulk_result() {
        let shared_app = app();
        shared_app
            .clone()
            .oneshot(authed("POST", "/", "alice", &["editor"], VALID_HERO_V1_XML))
            .await
            .unwrap();
        shared_app
            .clone()
            .oneshot(authed("POST", "/", "alice", &["editor"], VALID_HERO_V1_XML))
            .await
            .unwrap();

        let response = shared_app
            .oneshot(authed(
                "DELETE",
                "/?power_level=5",
                "alice",
                &["maintainer"],
                "",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_text(response).await;
        assert!(body.contains("<bulk_delete_result>"), "{body}");
        assert!(body.contains("<matched>2</matched>"), "{body}");
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
        let mut request = authed(method, uri, "alice", roles, body);
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
        let response = call(app, "POST", "/", &["editor"], VALID_HERO_V1_XML, &[]).await;
        assert_eq!(response.status(), StatusCode::CREATED);
        text(response)
            .await
            .split("<id>")
            .nth(1)
            .unwrap()
            .split("</id>")
            .next()
            .unwrap()
            .parse()
            .unwrap()
    }

    fn rename_body(to: &str) -> String {
        format!("<hero><name>{to}</name></hero>")
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
