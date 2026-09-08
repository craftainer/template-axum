//! `/crud/v1/heroes/v2/json` -- the Hero v2 resource router (list/get/
//! create/update/delete). Port of `crud_1/heroes/heroes_v2.py`, mounted
//! by `main.rs` at the exact path template-fastapi uses (`docs/adrs/0009`).
//! Owner-scoped per ADR 0011 (reads open, writes/deletes restricted to the
//! caller's own `sub`); soft-deleted per ADR 0012.

use std::collections::HashMap;
use std::net::SocketAddr;

use axum::extract::{ConnectInfo, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};

use crate::controllers::crud_actions::{DeleteOutcome, ListOrGet, UpdateOutcome};
use crate::controllers::crud_query::{parse_filters, parse_sort, FieldSpec};
use crate::controllers::{
    crud_actions, AppState, HERO_DELETE_ROLES, HERO_READ_ROLES, HERO_WRITE_ROLES,
};
use crate::crud::DEFAULT_LIMIT;
use crate::oidc::AuthClaims;
use crate::problem_details::AppError;
use crate::views::hero::{HeroCreate, HeroListQuery, HeroRead, HeroUpdate};

/// Rate-limit scope key (`src/rate_limit.rs`) shared by create/update/
/// delete -- a single record edit shares the same per-caller budget as
/// every other mutating call, matching `rate_limit.py`'s own reasoning
/// (see that module's doc comment) for applying the limit to a route's
/// handler as a whole rather than exempting any one verb. `pub(crate)`:
/// shared with `controllers::heroes_xml` (`docs/adrs/0014`) so both
/// sibling routers draw from the same per-caller budget rather than each
/// format getting its own.
pub(crate) const HERO_WRITE_RATE_SCOPE: &str = "hero-write";

/// Hero's filterable/sortable fields, derived by hand from `HeroRead`'s
/// scalar fields (`docs/adrs/0013`) -- `powers` (a list, not a scalar) has
/// no equivalent here, matching `crud_query.py`'s own field-classifier
/// skipping non-scalar fields. `pub(crate)`: shared with
/// `controllers::heroes_xml`, same reasoning as the rate-limit scope above.
pub(crate) const HERO_FIELD_SPECS: &[FieldSpec] = &[
    FieldSpec::number("id"),
    FieldSpec::string("name"),
    FieldSpec::number("power_level"),
    FieldSpec::string("owner_id"),
    FieldSpec::datetime("archived_at"),
    FieldSpec::datetime("created_at"),
    FieldSpec::datetime("updated_at"),
];

fn hero_read_json(hero: crate::models::hero::Model) -> serde_json::Value {
    serde_json::to_value(HeroRead::from(hero)).expect("HeroRead always serializes")
}

/// `GET ?id=` (single) or `GET ` (list, `?skip=`/`?limit=`/
/// `?include_archived=`, plus any `field[__op]=`/`sort=` filter/sort
/// params -- `docs/adrs/0013`) -- record addressing is a query parameter,
/// never a path segment, matching `crud_router.py`'s `?id=` convention.
async fn list_or_get(
    State(state): State<AppState>,
    AuthClaims(claims): AuthClaims,
    Query(query): Query<HeroListQuery>,
    Query(raw_params): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, AppError> {
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
        ListOrGet::One(hero) => Ok(Json(hero_read_json(hero))),
        ListOrGet::Many(heroes) => {
            let heroes: Vec<HeroRead> = heroes.into_iter().map(HeroRead::from).collect();
            Ok(Json(
                serde_json::to_value(heroes).expect("Vec<HeroRead> always serializes"),
            ))
        }
    }
}

/// `POST ` -> 201. Stamps `owner_id` from the caller's `sub`, never trusts
/// client input (ADR 0011).
async fn create(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    AuthClaims(claims): AuthClaims,
    Json(payload): Json<HeroCreate>,
) -> Result<(StatusCode, Json<HeroRead>), AppError> {
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
    let hero = state.hero_crud.create(&owner_id, payload).await?;
    Ok((StatusCode::CREATED, Json(HeroRead::from(hero))))
}

/// `PATCH ?id=` -- partial update of one record; an omitted field is left
/// unchanged (FR-0004). `PATCH` with no `?id=` but at least one filter
/// (`field[__op]=`) instead bulk-updates every matching record
/// (`docs/adrs/0013`) with the same payload. Owner-scoped either way: a
/// caller can only update their own heroes.
async fn update(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    AuthClaims(claims): AuthClaims,
    Query(query): Query<HeroListQuery>,
    Query(raw_params): Query<HashMap<String, String>>,
    Json(payload): Json<HeroUpdate>,
) -> Result<Json<serde_json::Value>, AppError> {
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
        UpdateOutcome::One(hero) => Ok(Json(hero_read_json(hero))),
        UpdateOutcome::Bulk(result) => Ok(Json(
            serde_json::to_value(result).expect("BulkUpdateResult always serializes"),
        )),
    }
}

/// `DELETE ?id=` -> 204. Soft-delete (sets `archived_at`, ADR 0012), owner-
/// scoped, restricted to the `maintainer` role (FR-0015). `DELETE` with no
/// `?id=` but at least one filter instead bulk-deletes every matching
/// record (`docs/adrs/0013`), returning a `BulkDeleteResult` (200) rather
/// than 204 -- there's no single record's absence to signal with an empty
/// body.
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
        DeleteOutcome::One => Ok(StatusCode::NO_CONTENT.into_response()),
        DeleteOutcome::Bulk(result) => Ok(Json(result).into_response()),
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
    use crate::controllers::DynHeroRepository;
    use crate::health::HealthRegistry;
    use crate::oidc::OidcVerifier;
    use crate::repositories::hero_memory::HeroMemoryRepository;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
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
        // create/update/delete_hero extract ConnectInfo<SocketAddr> for
        // rate limiting -- `oneshot` bypasses the real
        // `into_make_service_with_connect_info` main.rs wires up, so tests
        // insert the same extension by hand (harmless for routes that
        // don't extract it, e.g. the GET list/get handler).
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

    const VALID_HERO: &str = r#"{"name":"Spectra","powers":["flight"],"power_level":5}"#;

    // -- role matrix (FR-0015): every route x every role this app grants. --

    #[tokio::test]
    async fn read_roles_can_list_heroes() {
        for role in ["viewer", "editor", "maintainer", "detective"] {
            let response = app()
                .oneshot(authed(
                    "GET",
                    "/",
                    "alice",
                    &[role],
                    serde_json::Value::Null,
                ))
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "role {role} should be able to list"
            );
        }
    }

    #[tokio::test]
    async fn a_role_with_no_read_grant_is_forbidden_from_listing() {
        let response = app()
            .oneshot(authed(
                "GET",
                "/",
                "alice",
                &["security"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_request_with_no_bearer_token_is_unauthorized() {
        let response = app()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn write_roles_can_create_but_viewer_and_detective_cannot() {
        for role in ["editor", "maintainer"] {
            let response = app()
                .oneshot(authed(
                    "POST",
                    "/",
                    "alice",
                    &[role],
                    serde_json::from_str(VALID_HERO).unwrap(),
                ))
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::CREATED,
                "role {role} should create"
            );
        }
        for role in ["viewer", "detective"] {
            let response = app()
                .oneshot(authed(
                    "POST",
                    "/",
                    "alice",
                    &[role],
                    serde_json::from_str(VALID_HERO).unwrap(),
                ))
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "role {role} should not create"
            );
        }
    }

    #[tokio::test]
    async fn create_rejects_an_invalid_payload_with_422_and_field_errors() {
        let response = app()
            .oneshot(authed(
                "POST",
                "/",
                "alice",
                &["editor"],
                serde_json::json!({"name": "", "powers": []}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = json_body(response).await;
        let errors = body["detail"].as_array().unwrap();
        assert_eq!(errors.len(), 2);
    }

    #[tokio::test]
    async fn only_maintainer_can_delete() {
        let create = app()
            .oneshot(authed(
                "POST",
                "/",
                "alice",
                &["maintainer"],
                serde_json::from_str(VALID_HERO).unwrap(),
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
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "editor must not delete"
        );
    }

    // -- ownership scoping (ADR 0011): writes are restricted to the caller
    // that created the record, even with an otherwise-sufficient role. --

    #[tokio::test]
    async fn update_by_a_non_owner_returns_404_not_403() {
        let shared_app = app();
        let create = shared_app
            .clone()
            .oneshot(authed(
                "POST",
                "/",
                "alice",
                &["editor"],
                serde_json::from_str(VALID_HERO).unwrap(),
            ))
            .await
            .unwrap();
        let created = json_body(create).await;
        let id = created["id"].as_i64().unwrap();

        let response = shared_app
            .oneshot(authed(
                "PATCH",
                &format!("/?id={id}"),
                "mallory",
                &["editor"],
                serde_json::json!({"name": "Hacked"}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
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

    // -- Redis-backed rate limiting (Tier B item 1, docs/adrs/0011). --

    #[tokio::test]
    async fn hero_write_routes_return_429_once_the_per_caller_limit_is_exceeded() {
        let settings = Arc::new(Settings {
            rate_limit_hero_write_per_minute: 2,
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
        };
        let shared_app = router().with_state(state);

        for _ in 0..2 {
            let response = shared_app
                .clone()
                .oneshot(authed(
                    "POST",
                    "/",
                    "alice",
                    &["editor"],
                    serde_json::from_str(VALID_HERO).unwrap(),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::CREATED);
        }

        let response = shared_app
            .oneshot(authed(
                "POST",
                "/",
                "alice",
                &["editor"],
                serde_json::from_str(VALID_HERO).unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn delete_without_id_query_param_returns_422() {
        let response = app()
            .oneshot(authed(
                "DELETE",
                "/",
                "alice",
                &["maintainer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    // -- generic filter/sort/bulk (docs/adrs/0013, Tier C item 3). --

    async fn create_hero(app: &Router, sub: &str, body: serde_json::Value) -> i64 {
        let response = app
            .clone()
            .oneshot(authed("POST", "/", sub, &["editor"], body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        json_body(response).await["id"].as_i64().unwrap()
    }

    #[tokio::test]
    async fn list_filters_by_an_equality_query_param() {
        let shared_app = app();
        create_hero(
            &shared_app,
            "alice",
            serde_json::json!({"name": "Spectra", "powers": ["flight"], "power_level": 5}),
        )
        .await;
        create_hero(
            &shared_app,
            "alice",
            serde_json::json!({"name": "Umbra", "powers": ["stealth"], "power_level": 3}),
        )
        .await;

        let response = shared_app
            .oneshot(authed(
                "GET",
                "/?name=Umbra",
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        let heroes = body.as_array().unwrap();
        assert_eq!(heroes.len(), 1);
        assert_eq!(heroes[0]["name"], "Umbra");
    }

    #[tokio::test]
    async fn list_sorts_descending_with_a_leading_dash() {
        let shared_app = app();
        create_hero(
            &shared_app,
            "alice",
            serde_json::json!({"name": "Low", "powers": ["a"], "power_level": 1}),
        )
        .await;
        create_hero(
            &shared_app,
            "alice",
            serde_json::json!({"name": "High", "powers": ["a"], "power_level": 9}),
        )
        .await;

        let response = shared_app
            .oneshot(authed(
                "GET",
                "/?sort=-power_level",
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        let heroes = body.as_array().unwrap();
        assert_eq!(heroes[0]["name"], "High");
        assert_eq!(heroes[1]["name"], "Low");
    }

    #[tokio::test]
    async fn list_rejects_an_unrecognized_filter_field_with_422() {
        let response = app()
            .oneshot(authed(
                "GET",
                "/?nope=1",
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn bulk_update_applies_the_payload_to_every_matching_owned_record() {
        let shared_app = app();
        create_hero(
            &shared_app,
            "alice",
            serde_json::json!({"name": "Spectra", "powers": ["flight"], "power_level": 5}),
        )
        .await;
        create_hero(
            &shared_app,
            "alice",
            serde_json::json!({"name": "Umbra", "powers": ["stealth"], "power_level": 5}),
        )
        .await;
        // A different owner's matching record must not be touched.
        create_hero(
            &shared_app,
            "mallory",
            serde_json::json!({"name": "Ghost", "powers": ["stealth"], "power_level": 5}),
        )
        .await;

        let response = shared_app
            .clone()
            .oneshot(authed(
                "PATCH",
                "/?power_level=5",
                "alice",
                &["editor"],
                serde_json::json!({"power_level": 10}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["matched"], 2);
        assert_eq!(body["ids"].as_array().unwrap().len(), 2);

        let mallory_check = shared_app
            .oneshot(authed(
                "GET",
                "/?power_level=5",
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        let body = json_body(mallory_check).await;
        // Only mallory's untouched record still has power_level=5.
        assert_eq!(body.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn bulk_update_with_no_id_and_no_filters_returns_422() {
        let response = app()
            .oneshot(authed(
                "PATCH",
                "/",
                "alice",
                &["editor"],
                serde_json::json!({"power_level": 10}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn bulk_delete_soft_deletes_every_matching_owned_record() {
        let shared_app = app();
        let id_a = create_hero(
            &shared_app,
            "alice",
            serde_json::json!({"name": "Spectra", "powers": ["flight"], "power_level": 7}),
        )
        .await;
        let id_b = create_hero(
            &shared_app,
            "alice",
            serde_json::json!({"name": "Umbra", "powers": ["stealth"], "power_level": 7}),
        )
        .await;

        let response = shared_app
            .clone()
            .oneshot(authed(
                "DELETE",
                "/?power_level=7",
                "alice",
                &["maintainer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["matched"], 2);
        let mut ids: Vec<i64> = body["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_i64().unwrap())
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, vec![id_a, id_b]);

        let after = shared_app
            .oneshot(authed(
                "GET",
                &format!("/?id={id_a}"),
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(after.status(), StatusCode::NOT_FOUND);
    }
}
