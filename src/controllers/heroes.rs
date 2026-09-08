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
