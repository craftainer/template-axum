//! `POST /mock/token` -- mounted only under `Mode::Mock` (FR-0017). Mints a
//! Keycloak-shaped JWT so RBAC is exercisable with zero real OIDC
//! provider; port of `controllers/mock.py`.

use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::controllers::AppState;
use crate::oidc::MOCK_SIGNING_KEY;
use crate::problem_details::AppError;

#[derive(Deserialize)]
struct MockTokenRequest {
    sub: String,
    #[serde(default)]
    roles: Vec<String>,
}

#[derive(Serialize)]
struct MockTokenResponse {
    access_token: String,
}

async fn mint_token(
    State(state): State<AppState>,
    Json(payload): Json<MockTokenRequest>,
) -> Result<Json<MockTokenResponse>, AppError> {
    let mut resource_access = serde_json::Map::new();
    resource_access.insert(
        state.settings.oidc_client_id.clone(),
        serde_json::json!({ "roles": payload.roles }),
    );
    let claims = serde_json::json!({
        "sub": payload.sub,
        "preferred_username": payload.sub,
        "resource_access": resource_access,
    });
    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(MOCK_SIGNING_KEY),
    )
    .map_err(|err| AppError::Internal(err.to_string()))?;

    Ok(Json(MockTokenResponse {
        access_token: token,
    }))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/token", post(mint_token))
}
