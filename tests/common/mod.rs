//! Shared fixtures for the integration tier -- see `tests/README.md`.
//!
//! Everything here talks to the devcontainer stack's own already-running
//! services (`.devcontainer/stack/`), never to a container this suite
//! starts itself. Connection details come from the same environment
//! variables `config::Settings` reads (`POSTGRES_*`, `MQTT_*`), so a run
//! inside the devcontainer or in CI needs no extra configuration.

#![allow(dead_code)] // Each test binary uses its own subset of these.

use std::sync::Arc;

use chrono::NaiveDateTime;
use sea_orm::{ActiveModelTrait, ConnectionTrait, Database, DatabaseConnection, Set};
use template_axum::config::{Mode, Settings};
use template_axum::controllers::{AppState, DynHeroRepository};
use template_axum::crud::CrudService;
use template_axum::events::{new_subscriber_id, EventBus};
use template_axum::health::HealthRegistry;
use template_axum::models::hero;
use template_axum::oidc::OidcVerifier;
use template_axum::rate_limit::RateLimiter;
use template_axum::repositories::hero_sea_orm::HeroSeaOrmRepository;

/// A short, unique suffix for a schema or MQTT topic, so concurrently
/// running test binaries (and reruns) never collide.
pub fn unique_suffix() -> String {
    new_subscriber_id()[..12].to_string()
}

fn base_database_url() -> String {
    std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        format!(
            "postgres://{}:{}@{}:{}/{}",
            std::env::var("POSTGRES_USER").unwrap_or_else(|_| "app".into()),
            std::env::var("POSTGRES_PASSWORD").unwrap_or_else(|_| "app".into()),
            std::env::var("POSTGRES_HOST").unwrap_or_else(|_| "localhost".into()),
            std::env::var("POSTGRES_PORT").unwrap_or_else(|_| "5432".into()),
            std::env::var("POSTGRES_DB").unwrap_or_else(|_| "app".into()),
        )
    })
}

/// A connection to the stack's Postgres, scoped to a schema of its own.
///
/// Every test gets a private schema (`search_path`) with the app's real
/// migrations applied into it, so tests that assert on whole-table
/// aggregates (`/stats`' `total`) can run in parallel with each other and
/// with anything else already in the shared dev database.
pub struct IsolatedDb {
    pub connection: DatabaseConnection,
    pub schema: String,
}

impl IsolatedDb {
    pub async fn new() -> Self {
        let base = base_database_url();
        let admin = Database::connect(&base)
            .await
            .expect("the devcontainer stack's Postgres must be running for the integration tier");
        let schema = format!("it_{}", unique_suffix());
        admin
            .execute_unprepared(&format!(r#"CREATE SCHEMA "{schema}""#))
            .await
            .expect("failed to create the test schema");

        // sqlx passes `options` straight through as libpq connection
        // options, which is how the pool's every connection gets the
        // search_path -- a one-off `SET search_path` would only stick to
        // whichever pooled connection happened to run it.
        let url = format!("{base}?options=-c%20search_path%3D{schema}");
        let connection = Database::connect(&url)
            .await
            .expect("failed to connect to the test schema");
        template_axum::run_migrations(&connection).await;

        Self { connection, schema }
    }

    /// Drop the schema and everything in it. Called explicitly rather than
    /// from `Drop`, which cannot await.
    pub async fn cleanup(&self) {
        let _ = self
            .connection
            .execute_unprepared(&format!(r#"DROP SCHEMA "{}" CASCADE"#, self.schema))
            .await;
    }
}

/// Insert one Hero row with a caller-chosen `created_at`.
///
/// Test-only, and deliberately not reachable from any production path:
/// `HeroSeaOrmRepository::create` always stamps `Utc::now()` (FR-0007), so
/// the only way to exercise a multi-bucket `/predict` forecast end-to-end
/// is to write the row from the test side instead of through the
/// repository. Going through the entity (not raw SQL) keeps the column
/// mapping honest.
pub async fn seed_hero_at(
    db: &DatabaseConnection,
    owner_id: &str,
    name: &str,
    power_level: Option<i32>,
    created_at: NaiveDateTime,
) -> i32 {
    hero::ActiveModel {
        name: Set(Some(name.to_string())),
        powers: Set(Some(vec!["flight".to_string()])),
        power_level: Set(power_level),
        owner_id: Set(owner_id.to_string()),
        archived_at: Set(None),
        created_at: Set(created_at),
        updated_at: Set(created_at),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("failed to seed a hero row")
    .id
}

/// Settings for the integration tier: real Postgres/MQTT coordinates, but
/// `Mode::Mock`'s auth so a test can mint its own bearer token
/// (FR-0017) instead of standing up a Keycloak client. The tiers under
/// test here are the repository and the HTTP handlers, not token
/// validation -- which has its own unit coverage in `oidc`.
pub fn integration_settings() -> Settings {
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
        mqtt_host: std::env::var("MQTT_HOST").unwrap_or_else(|_| "localhost".into()),
        mqtt_port: std::env::var("MQTT_PORT")
            .ok()
            .and_then(|port| port.parse().ok())
            .unwrap_or(1883),
        rate_limit_mock_token_per_minute: 10_000,
        rate_limit_hero_write_per_minute: 10_000,
        bulk_action_max_matched: 1000,
        oidc_issuer_url: "http://localhost:8080/realms/template-fastapi".to_string(),
        oidc_authorization_url: "http://localhost:8080/auth".to_string(),
        oidc_token_url: "http://localhost:8080/token".to_string(),
        oidc_client_id: "api".to_string(),
        oidc_audience: None,
    }
}

/// `AppState` wired to a real Postgres-backed repository.
pub fn state_for(db: DatabaseConnection, events: EventBus) -> AppState {
    let settings = Arc::new(integration_settings());
    AppState {
        oidc: Arc::new(OidcVerifier::new(settings.clone())),
        settings,
        health_registry: Arc::new(HealthRegistry::new()),
        hero_crud: Arc::new(CrudService::new(DynHeroRepository(Box::new(
            HeroSeaOrmRepository::new(db),
        )))),
        rate_limiter: Arc::new(RateLimiter::mock()),
        events: Arc::new(events),
    }
}

/// A `Mode::Mock` bearer token (`docs/adrs/0005`) with the given roles.
pub fn token(sub: &str, roles: &[&str]) -> String {
    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &serde_json::json!({
            "sub": sub,
            "resource_access": { "api": { "roles": roles } }
        }),
        &jsonwebtoken::EncodingKey::from_secret(b"mock-mode-doesnt-verify-signatures"),
    )
    .expect("failed to encode a mock token")
}

/// An authenticated request carrying the `ConnectInfo` the rate-limited
/// handlers extract (`tower::ServiceExt::oneshot` bypasses the real
/// `into_make_service_with_connect_info`).
pub fn authed_request(
    method: &str,
    uri: &str,
    sub: &str,
    roles: &[&str],
    body: serde_json::Value,
) -> axum::http::Request<axum::body::Body> {
    let body = if body.is_null() {
        axum::body::Body::empty()
    } else {
        axum::body::Body::from(body.to_string())
    };
    let mut request = axum::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("Authorization", format!("Bearer {}", token(sub, roles)))
        .header("Content-Type", "application/json")
        .body(body)
        .expect("failed to build a request");
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            12345,
        ))));
    request
}

pub async fn json_body(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("failed to read the response body");
    serde_json::from_slice(&bytes).expect("response body was not JSON")
}
