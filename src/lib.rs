//! The axum application as a library, so both `main.rs` (the binary) and
//! `tests/` (Cargo's integration-test convention -- a black-box binary per
//! file, linked against this crate's public API) build the *same* router
//! and state rather than each re-deriving the wiring.
//!
//! See `src/README.md` for the module layering this file sits at the top
//! of: `config` -> `oidc` -> `generic::models`/`hero::models` ->
//! `generic::views`/`hero::views` -> `generic::repositories`/
//! `hero::repositories` -> `crud` -> `health` ->
//! `generic::controllers`/`hero::controllers`, with `events`/
//! `rate_limit`/`telemetry`/`problem_details`/`http_headers` flat
//! alongside. `generic`/`hero` are this app's two resource-layering
//! packages (see `src/README.md`'s "Generic vs. Hero-specific split");
//! the wiring functions below are the only thing above `controllers`, and
//! `main.rs` does nothing but call them.

pub mod config;
pub mod crud;
pub mod events;
pub mod generic;
pub mod health;
pub mod hero;
pub mod http_headers;
pub mod migration;
pub mod oidc;
pub mod problem_details;
pub mod rate_limit;
pub mod telemetry;

use std::sync::Arc;

use axum::Router;
use sea_orm_migration::MigratorTrait;
use tower_http::trace::TraceLayer;

use config::{Mode, Settings};
use crud::CrudService;
use events::EventBus;
use generic::controllers as generic_controllers;
use health::checks::{
    DatabaseHealthCheck, MockHealthCheck, OidcHealthCheck, RedisHealthCheck, S3HealthCheck,
};
use health::HealthRegistry;
use hero::controllers::{self as hero_controllers, AppState};
use hero::repositories::hero_memory::HeroMemoryRepository;
use hero::repositories::hero_sea_orm::HeroSeaOrmRepository;
use oidc::OidcVerifier;
use rate_limit::RateLimiter;

/// Applies pending SeaORM migrations, off the request path, before the
/// server starts accepting connections (FR-0020). Never runs under
/// `Mode::Mock` -- there's no database to migrate (see
/// `repositories::hero_memory`).
pub async fn run_migrations(db: &sea_orm::DatabaseConnection) {
    migration::Migrator::up(db, None)
        .await
        .expect("failed to apply pending migrations");
}

pub async fn build_health_registry(
    settings: &Settings,
    db: Option<sea_orm::DatabaseConnection>,
) -> HealthRegistry {
    let mut registry = HealthRegistry::new();
    if settings.mode == Mode::Mock {
        registry.register(Box::new(MockHealthCheck::new("database")));
        registry.register(Box::new(MockHealthCheck::new("redis")));
        registry.register(Box::new(MockHealthCheck::new("s3")));
        registry.register(Box::new(MockHealthCheck::new("oidc")));
        return registry;
    }

    let db = db.expect("a real database connection is required outside Mode::Mock");
    registry.register(Box::new(DatabaseHealthCheck::new(db)));
    registry.register(Box::new(RedisHealthCheck::new(settings.redis_url.clone())));

    let s3_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .endpoint_url(&settings.s3_endpoint_url)
        .credentials_provider(aws_sdk_s3::config::Credentials::new(
            &settings.s3_access_key,
            &settings.s3_secret_key,
            None,
            None,
            "template-axum",
        ))
        .region(aws_sdk_s3::config::Region::new("us-east-1"))
        .load()
        .await;
    let s3_client = aws_sdk_s3::Client::new(&s3_config);
    registry.register(Box::new(S3HealthCheck::new(s3_client)));
    registry.register(Box::new(OidcHealthCheck::new(
        settings.oidc_issuer_url.clone(),
    )));

    registry
}

/// The CRUD event bus (`docs/adrs/0016`): a real MQTT publisher outside
/// `Mode::Mock`, an in-memory `tokio::sync::broadcast` fan-out inside it
/// (NFR-0022 -- mock mode boots with zero containers). Same `Mode`-branch
/// shape as the repository and rate-limiter choices above/below.
pub fn build_event_bus(settings: &Settings) -> EventBus {
    if settings.mode == Mode::Mock {
        EventBus::mock()
    } else {
        EventBus::connect(&settings.mqtt_host, settings.mqtt_port)
    }
}

/// Assemble every shared dependency into the state each request is handed.
/// Connects to (and migrates) Postgres, Redis and MQTT outside
/// `Mode::Mock`; builds the in-memory fakes inside it.
pub async fn build_state(settings: Arc<Settings>) -> AppState {
    let (hero_repository, db_for_health): (
        hero_controllers::DynHeroRepository,
        Option<sea_orm::DatabaseConnection>,
    ) = if settings.mode == Mode::Mock {
        (
            hero_controllers::DynHeroRepository(Box::new(HeroMemoryRepository::new())),
            None,
        )
    } else {
        let db = sea_orm::Database::connect(settings.database_url())
            .await
            .expect("failed to connect to Postgres");
        run_migrations(&db).await;
        (
            hero_controllers::DynHeroRepository(Box::new(HeroSeaOrmRepository::new(db.clone()))),
            Some(db),
        )
    };

    let health_registry = build_health_registry(&settings, db_for_health).await;

    let rate_limiter = if settings.mode == Mode::Mock {
        RateLimiter::mock()
    } else {
        RateLimiter::connect(&settings.redis_url)
            .await
            .expect("failed to connect to Redis for rate limiting")
    };

    AppState {
        events: Arc::new(build_event_bus(&settings)),
        oidc: Arc::new(OidcVerifier::new(settings.clone())),
        settings,
        health_registry: Arc::new(health_registry),
        hero_crud: Arc::new(CrudService::new(hero_repository)),
        rate_limiter: Arc::new(rate_limiter),
    }
}

/// Mount every router onto one `Router`, at the exact paths
/// template-fastapi uses (`docs/adrs/0002`). `/mock` exists only under
/// `Mode::Mock`.
pub fn build_router(state: AppState) -> Router {
    let mut app = Router::new()
        .nest("/health", generic_controllers::health::router())
        .nest("/audit", generic_controllers::audit::router())
        .nest(
            "/crud/v1/heroes/v2/json",
            hero_controllers::heroes::router(),
        )
        .nest(
            "/crud/v1/heroes/v2/xml",
            hero_controllers::heroes_xml::router(),
        )
        .nest(
            "/crud/v1/heroes/v1/json",
            hero_controllers::heroes_v1::router(),
        )
        .nest(
            "/crud/v1/heroes/v1/xml",
            hero_controllers::heroes_v1_xml::router(),
        )
        .nest("/heroes", hero_controllers::heroes_web::router());

    if state.settings.mode == Mode::Mock {
        app = app.nest("/mock", generic_controllers::mock::router());
    }

    app.with_state(state).layer(TraceLayer::new_for_http())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    /// `Mode::Mock` settings -- the only mode whose wiring can be built
    /// without reaching Postgres/Redis/MQTT, and therefore the only branch
    /// of these functions a unit test can take. The `Mode::Dev` branches
    /// are exercised by the integration tier (`tests/`), which does have
    /// the real services.
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

    #[tokio::test]
    async fn mock_mode_registers_a_fake_for_every_external_service() {
        let registry = build_health_registry(&mock_settings(), None).await;
        let results = registry.run_all().await;
        let mut names: Vec<&str> = results.iter().map(|(name, _)| name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["database", "oidc", "redis", "s3"]);
        assert!(
            results.iter().all(|(_, result)| result.healthy),
            "every mock check reports healthy (NFR-0022)"
        );
    }

    #[tokio::test]
    async fn mock_mode_builds_an_in_memory_event_bus() {
        let bus = build_event_bus(&mock_settings());
        // The broker-backed variant would need a live Mosquitto; this one
        // fans out in-process, which is what makes MODE=mock container-free.
        let mut stream = bus.subscribe("heroes", "sub-1").await.unwrap();
        bus.publish(events::CrudEvent::new(
            "heroes",
            events::EventAction::Create,
            vec![1],
        ))
        .await;
        assert_eq!(stream.next_event().await.unwrap().ids, vec![1]);
    }

    #[tokio::test]
    async fn mock_mode_state_and_router_serve_every_mounted_prefix() {
        let state = build_state(Arc::new(mock_settings())).await;
        let app = build_router(state);

        for path in [
            "/health/live",
            "/crud/v1/heroes/v2/json",
            "/crud/v1/heroes/v2/xml",
            "/crud/v1/heroes/v1/json",
            "/crud/v1/heroes/v1/xml",
            "/audit",
            "/heroes/form",
            "/heroes/components.js",
        ] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_ne!(
                response.status(),
                StatusCode::NOT_FOUND,
                "{path} must be mounted"
            );
        }

        // /mock exists only under Mode::Mock -- a 405 (wrong method for a
        // POST-only route) proves the route is mounted, unlike a 404.
        let mock_route = app
            .oneshot(
                Request::builder()
                    .uri("/mock/token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(mock_route.status(), StatusCode::NOT_FOUND);
    }
}
