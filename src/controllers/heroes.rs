//! `/crud/v1/heroes/v2/json` -- the Hero v2 resource router (list/get/
//! create/update/delete). Port of `crud_1/heroes/heroes_v2.py`, mounted
//! by `main.rs` at the exact path template-fastapi uses (`docs/adrs/0009`).
//! Owner-scoped per ADR 0011 (reads open, writes/deletes restricted to the
//! caller's own `sub`); soft-deleted per ADR 0012.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};

use crate::controllers::{AppState, HERO_DELETE_ROLES, HERO_READ_ROLES, HERO_WRITE_ROLES};
use crate::crud::DEFAULT_LIMIT;
use crate::oidc::AuthClaims;
use crate::problem_details::AppError;
use crate::views::hero::{HeroCreate, HeroListQuery, HeroRead, HeroUpdate};

/// `GET ?id=` (single) or `GET ` (list, `?skip=`/`?limit=`/
/// `?include_archived=`) -- record addressing is a query parameter, never
/// a path segment, matching `crud_router.py`'s `?id=` convention.
async fn list_or_get(
    State(state): State<AppState>,
    AuthClaims(claims): AuthClaims,
    Query(query): Query<HeroListQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    claims.require_any_role(&state.settings.oidc_client_id, HERO_READ_ROLES)?;
    let include_archived = query.include_archived.unwrap_or(false);

    if let Some(id) = query.id {
        let hero = state.hero_crud.get(id, include_archived).await?;
        let hero = hero.ok_or_else(|| AppError::NotFound(format!("hero {id} not found")))?;
        return Ok(Json(serde_json::to_value(HeroRead::from(hero)).unwrap()));
    }

    let skip = query.skip.unwrap_or(0);
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT);
    let heroes: Vec<HeroRead> = state
        .hero_crud
        .list(skip, limit, include_archived)
        .await?
        .into_iter()
        .map(HeroRead::from)
        .collect();
    Ok(Json(serde_json::to_value(heroes).unwrap()))
}

/// `POST ` -> 201. Stamps `owner_id` from the caller's `sub`, never trusts
/// client input (ADR 0011).
async fn create(
    State(state): State<AppState>,
    AuthClaims(claims): AuthClaims,
    Json(payload): Json<HeroCreate>,
) -> Result<(StatusCode, Json<HeroRead>), AppError> {
    claims.require_any_role(&state.settings.oidc_client_id, HERO_WRITE_ROLES)?;
    payload.validate().map_err(AppError::UnprocessableEntity)?;
    let owner_id = claims.subject()?.to_string();
    let hero = state.hero_crud.create(&owner_id, payload).await?;
    Ok((StatusCode::CREATED, Json(HeroRead::from(hero))))
}

/// `PATCH ?id=` -- partial update; an omitted field is left unchanged
/// (FR-0004). Owner-scoped: a caller can only update their own hero.
async fn update(
    State(state): State<AppState>,
    AuthClaims(claims): AuthClaims,
    Query(query): Query<HeroListQuery>,
    Json(payload): Json<HeroUpdate>,
) -> Result<Json<HeroRead>, AppError> {
    claims.require_any_role(&state.settings.oidc_client_id, HERO_WRITE_ROLES)?;
    payload.validate().map_err(AppError::UnprocessableEntity)?;
    let id = query.id.ok_or_else(|| {
        AppError::UnprocessableEntity(vec![crate::views::FieldError::new(
            "id",
            "id query parameter is required",
        )])
    })?;
    let owner_id = claims.subject()?.to_string();
    let hero = state.hero_crud.update(id, &owner_id, payload).await?;
    let hero = hero.ok_or_else(|| AppError::NotFound(format!("hero {id} not found")))?;
    Ok(Json(HeroRead::from(hero)))
}

/// `DELETE ?id=` -> 204. Soft-delete (sets `archived_at`, ADR 0012), owner-
/// scoped, restricted to the `maintainer` role (FR-0015).
async fn delete_hero(
    State(state): State<AppState>,
    AuthClaims(claims): AuthClaims,
    Query(query): Query<HeroListQuery>,
) -> Result<StatusCode, AppError> {
    claims.require_any_role(&state.settings.oidc_client_id, HERO_DELETE_ROLES)?;
    let id = query.id.ok_or_else(|| {
        AppError::UnprocessableEntity(vec![crate::views::FieldError::new(
            "id",
            "id query parameter is required",
        )])
    })?;
    let owner_id = claims.subject()?.to_string();
    let deleted = state.hero_crud.delete(id, &owner_id).await?;
    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::NotFound(format!("hero {id} not found")))
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
        Request::builder()
            .method(method)
            .uri(uri)
            .header("Authorization", format!("Bearer {}", token(sub, roles)))
            .header("Content-Type", "application/json")
            .body(body)
            .unwrap()
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
}
