//! The axum application as a library, so both `main.rs` (the binary) and
//! `tests/` (Cargo's integration-test convention -- a black-box binary per
//! file, linked against this crate's public API) build the *same* router
//! and state rather than each re-deriving the wiring.
//!
//! See `src/README.md` for the module layering this file sits at the top
//! of: `config` -> `oidc` -> `hero::models` -> `hero::views` ->
//! `hero::repositories` -> `generic::controllers`/`hero::controllers`,
//! with `rate_limit`/`problem_details`/`http_headers` flat alongside. The
//! generic CRUD layer and the health/logging layer are the
//! `craftainer-crud` and `craftainer-observability` crates
//! (docs/adrs/0020); the wiring functions below are the only thing above
//! `controllers`, and `main.rs` does nothing but call them.

pub mod cache;
pub mod config;
pub mod db;
pub mod generic;
pub mod hero;
pub mod http_headers;
pub mod migration;
pub mod oidc;
pub mod problem_details;
pub mod rate_limit;

use std::sync::Arc;

use axum::Router;
use sea_orm_migration::MigratorTrait;
use tower_http::trace::TraceLayer;

use config::{Mode, Settings};
use craftainer_crud::crud::CrudService;
use craftainer_crud::events::EventBus;
use craftainer_crud::stats_cache::CachedStats;
use craftainer_observability::health::checks::{
    DatabaseHealthCheck, HttpHealthCheck, MockHealthCheck, RedisHealthCheck, S3HealthCheck,
};
use craftainer_observability::health::info::info_router;
use craftainer_observability::health::router::router as health_router;
use craftainer_observability::health::HealthRegistry;
use craftainer_observability::otel::ServerSpanLayer;
use craftainer_observability::request_id::RequestIdLayer;
use generic::controllers as generic_controllers;
use hero::controllers::{self as hero_controllers, AppState, HERO_EVENT_RESOURCE};
use hero::repositories::hero_memory::HeroMemoryRepository;
use hero::repositories::hero_sea_orm::HeroSeaOrmRepository;
use oidc::OidcVerifier;
use rate_limit::RateLimiter;
use tokio_util::sync::CancellationToken;

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
    // `register` rejects a duplicate check name; the names below are fixed
    // and distinct, so a failure is a programming error.
    const DISTINCT: &str = "health check names are distinct";
    let mut registry = HealthRegistry::new();
    if settings.mode == Mode::Mock {
        for name in ["database", "redis", "s3", "oidc"] {
            registry
                .register(Box::new(MockHealthCheck::new(name)))
                .expect(DISTINCT);
        }
        return registry;
    }

    let db = db.expect("a real database connection is required outside Mode::Mock");
    registry
        .register(Box::new(DatabaseHealthCheck::new(
            db.get_postgres_connection_pool().clone(),
        )))
        .expect(DISTINCT);
    registry
        .register(Box::new(RedisHealthCheck::new(settings.redis_url.clone())))
        .expect(DISTINCT);

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
    registry
        .register(Box::new(S3HealthCheck::new(s3_client)))
        .expect(DISTINCT);
    registry
        .register(Box::new(HttpHealthCheck::new(
            "oidc",
            format!(
                "{}/.well-known/openid-configuration",
                settings.oidc_issuer_url.trim_end_matches('/')
            ),
        )))
        .expect(DISTINCT);

    registry
}

/// The CRUD event bus (`docs/adrs/0016`): a real MQTT publisher outside
/// `Mode::Mock`, an in-memory `tokio::sync::broadcast` fan-out inside it
/// (NFR-0022 -- mock mode boots with zero containers). Same `Mode`-branch
/// shape as the repository and rate-limiter choices above/below.
///
/// `subscriber_secret` (`config::subscriber_secret`, at least 32 bytes) is
/// handed to `EventBus::with_subscriber_secret` so the subscriber ids the
/// bus mints verify on every replica and after a restart; `None` keeps the
/// bus's random per-process key (`docs/adrs/0020`).
pub fn build_event_bus(settings: &Settings, subscriber_secret: Option<&[u8]>) -> EventBus {
    let bus = if settings.mode == Mode::Mock {
        EventBus::mock()
    } else {
        EventBus::connect(&settings.mqtt_host, settings.mqtt_port)
    };
    match subscriber_secret {
        Some(secret) => bus.with_subscriber_secret(secret),
        None => bus,
    }
}

/// Open the Postgres pool (`db::connect`, `DB_POOL_*`) and, unless
/// `RUN_MIGRATIONS_ON_STARTUP=false`, apply pending migrations. `None`
/// under `Mode::Mock`, which has no database. The caller owns the handle
/// and closes it at shutdown (`DatabaseConnection::close`) so pooled
/// connections end cleanly.
pub async fn connect_database(settings: &Settings) -> Option<sea_orm::DatabaseConnection> {
    if settings.mode == Mode::Mock {
        return None;
    }
    let db = db::connect(settings)
        .await
        .expect("failed to connect to Postgres");
    if settings.db.run_migrations_on_startup {
        run_migrations(&db).await;
    }
    Some(db)
}

/// Assemble every shared dependency into the state each request is handed.
/// Connects to (and migrates) Postgres, Redis and MQTT outside
/// `Mode::Mock`; builds the in-memory fakes inside it.
pub async fn build_state(settings: Arc<Settings>) -> AppState {
    let db = connect_database(&settings).await;
    build_state_with_db(settings, db).await
}

/// `build_state` over an already-opened pool (`connect_database`), for a
/// caller that keeps its own handle to close at shutdown. `db` must be
/// `Some` exactly when `settings.mode` is not `Mode::Mock` (panics if not).
pub async fn build_state_with_db(
    settings: Arc<Settings>,
    db: Option<sea_orm::DatabaseConnection>,
) -> AppState {
    assert_eq!(
        db.is_some(),
        settings.mode != Mode::Mock,
        "a database handle is required exactly outside Mode::Mock"
    );
    let hero_repository: hero_controllers::DynHeroRepository = match &db {
        None => hero_controllers::DynHeroRepository(Box::new(HeroMemoryRepository::new())),
        Some(db) => {
            hero_controllers::DynHeroRepository(Box::new(HeroSeaOrmRepository::new(db.clone())))
        }
    };
    let db_for_health = db;

    let health_registry = build_health_registry(&settings, db_for_health).await;

    let rate_limiter = if settings.mode == Mode::Mock {
        RateLimiter::mock()
    } else {
        RateLimiter::connect(&settings.redis_url)
            .await
            .expect("failed to connect to Redis for rate limiting")
    };

    let stats_ttl =
        config::stats_cache_ttl().unwrap_or_else(|err| panic!("invalid configuration: {err}"));
    let hero_stats = match cache::build_stats_cache(settings.mode, &settings.redis_url, stats_ttl)
        .await
        .expect("failed to connect to Redis for the stats cache")
    {
        Some(cache) => CachedStats::new(cache, HERO_EVENT_RESOURCE),
        None => CachedStats::uncached(HERO_EVENT_RESOURCE),
    };

    let subscriber_secret = config::subscriber_secret(settings.mode)
        .unwrap_or_else(|err| panic!("invalid configuration: {err}"));

    AppState {
        events: Arc::new(build_event_bus(
            &settings,
            subscriber_secret.as_deref().map(str::as_bytes),
        )),
        oidc: Arc::new(OidcVerifier::new(settings.clone())),
        settings,
        health_registry: Arc::new(health_registry),
        hero_crud: Arc::new(CrudService::new(hero_repository)),
        rate_limiter: Arc::new(rate_limiter),
        hero_stats,
        shutdown: CancellationToken::new(),
    }
}

/// Start the task that drops Hero's cached `/stats`/`/predict` bodies on
/// every CRUD event (`docs/adrs/0024`). `None` when the cache is off
/// (`enabled` false, i.e. `STATS_CACHE_TTL_SECONDS` unset). The task ends
/// when `state.shutdown` is cancelled; the caller awaits the handle.
pub fn spawn_stats_cache_invalidation(
    state: &AppState,
    enabled: bool,
) -> Option<tokio::task::JoinHandle<()>> {
    enabled.then(|| {
        tokio::spawn(cache::run_invalidation(
            state.hero_stats.clone(),
            state.events.clone(),
            HERO_EVENT_RESOURCE,
            cache::INVALIDATION_RETRY,
            state.shutdown.clone(),
        ))
    })
}

/// Mount every router onto one `Router`, at the exact paths
/// template-fastapi uses (`docs/adrs/0002`). `/mock` exists only under
/// `Mode::Mock`.
///
/// Layers, outermost first: `RequestIdLayer` (validates or creates
/// `X-Request-ID`, echoes it, opens the `request` span so every log line
/// below carries `request_id`), `ServerSpanLayer` (one OpenTelemetry server
/// span per request named after the matched route, none for `/health/*`;
/// built here, so after `configure_telemetry` in `main`), `TraceLayer`
/// (`docs/adrs/0026`).
pub fn build_router(state: AppState) -> Router {
    let mut app = Router::new()
        .nest("/health", health_router().merge(info_router()))
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

    app.with_state(state)
        .layer(TraceLayer::new_for_http())
        .layer(ServerSpanLayer::new())
        .layer(RequestIdLayer::new())
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
        let bus = build_event_bus(&mock_settings(), None);
        // The broker-backed variant would need a live Mosquitto; this one
        // fans out in-process, which is what makes MODE=mock container-free.
        let (_id, mut stream) = bus.subscribe("heroes", None).await.unwrap();
        bus.publish(craftainer_crud::events::CrudEvent::new(
            "heroes",
            craftainer_crud::events::EventAction::Create,
            vec![1],
        ))
        .await;
        assert_eq!(stream.next_event().await.unwrap().ids, vec![1]);
    }

    #[test]
    fn a_shared_subscriber_secret_makes_ids_verify_across_buses() {
        let secret = b"s".repeat(32);
        let first = build_event_bus(&mock_settings(), Some(&secret));
        let id = first.mint_subscriber_id("heroes");
        // A "restart" (or another replica): a new bus with the same secret.
        let second = build_event_bus(&mock_settings(), Some(&secret));
        assert_eq!(second.verify_subscriber_id("heroes", &id), Some(id.clone()));
        // Without it, each bus has its own random key and rejects the id.
        let unkeyed = build_event_bus(&mock_settings(), None);
        assert_eq!(unkeyed.verify_subscriber_id("heroes", &id), None);
        let other = build_event_bus(&mock_settings(), Some(&b"o".repeat(32)));
        assert_eq!(other.verify_subscriber_id("heroes", &id), None);
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

    async fn get(app: &Router, path: &str, request_id: Option<&str>) -> axum::response::Response {
        let mut request = Request::builder().uri(path);
        if let Some(value) = request_id {
            request = request.header("x-request-id", value);
        }
        app.clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    fn request_id_of(response: &axum::response::Response) -> String {
        response
            .headers()
            .get("x-request-id")
            .expect("every response carries X-Request-ID")
            .to_str()
            .unwrap()
            .to_string()
    }

    #[tokio::test]
    async fn the_request_id_is_echoed_when_valid_and_replaced_otherwise() {
        let app = build_router(build_state(Arc::new(mock_settings())).await);

        let echoed = get(&app, "/health/live", Some("client.id_1-A")).await;
        assert_eq!(request_id_of(&echoed), "client.id_1-A");

        let generated = get(&app, "/health/live", None).await;
        let id = request_id_of(&generated);
        assert_eq!(id.len(), 32);
        assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));

        // Not in `[A-Za-z0-9._-]`, and over 128 characters: both replaced.
        for bad in ["has space", "semi;colon", &"a".repeat(129)] {
            let replaced = get(&app, "/health/live", Some(bad)).await;
            let id = request_id_of(&replaced);
            assert_ne!(id, bad);
            assert_eq!(id.len(), 32);
        }

        // Routes outside /health get it too, including a 404.
        let other = get(&app, "/heroes/form", Some("abc")).await;
        assert_eq!(request_id_of(&other), "abc");
        let missing = get(&app, "/no/such/route", Some("abc")).await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert_eq!(request_id_of(&missing), "abc");
    }

    #[tokio::test]
    async fn health_info_is_mounted_next_to_the_health_checks() {
        let app = build_router(build_state(Arc::new(mock_settings())).await);
        let response = get(&app, "/health/info", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.get("version").is_some() && json.get("commit").is_some());
    }
}
