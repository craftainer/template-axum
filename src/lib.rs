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

use config::{Mode, ResourceServer, Settings};
use craftainer_crud::crud::CrudService;
use craftainer_crud::events::EventBus;
use craftainer_crud::manifest::{
    api_catalog_router, manifest_router, Access, EventsDescriptor, Features, ManifestConfig,
    ManifestField, ManifestRegistry, Replay, ResourceDescriptor,
};
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
use oidc::well_known::ManifestCallers;
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
            resource_server(settings).discovery_url,
        )))
        .expect(DISTINCT);

    registry
}

/// The resource-server settings (`OIDC_ISSUER`, `OIDC_DISCOVERY_URL`,
/// `OIDC_AUDIENCE`, `PUBLIC_BASE_URL`, `SECURITY_TXT_*`; `docs/adrs/0030`).
/// `main` has already validated them through `Settings::from_env`, so a
/// failure here is a programming error or a test's bad environment.
fn resource_server(settings: &Settings) -> ResourceServer {
    ResourceServer::from_env(settings).unwrap_or_else(|err| panic!("invalid configuration: {err}"))
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
        oidc: Arc::new(OidcVerifier::with_resource_server(
            settings.clone(),
            Arc::new(resource_server(&settings)),
        )),
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

fn schema(properties: serde_json::Value, required: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
    })
}

/// One Hero version's manifest descriptor (`craftainer_crud::manifest`),
/// filterable fields from the same `HERO_FIELD_SPECS` the routes parse
/// with. `powers` (v2) / `superpower` (v1) is not a scalar field, so it
/// has no filter entry (`docs/adrs/0013`).
fn hero_descriptor(version: &str, mode: Mode) -> ResourceDescriptor {
    let v2 = version == "v2";
    let power = if v2 {
        serde_json::json!({"powers": {"type": "array", "items": {"type": "string", "minLength": 1, "maxLength": 200}, "minItems": 1}})
    } else {
        serde_json::json!({"superpower": {"type": "string", "minLength": 1, "maxLength": 200}})
    };
    let power_key = if v2 { "powers" } else { "superpower" };
    let body = |required: &[&str]| {
        let mut properties = serde_json::json!({
            "name": {"type": "string", "minLength": 1, "maxLength": 200},
            "power_level": {"type": ["integer", "null"]},
        });
        properties
            .as_object_mut()
            .expect("an object literal")
            .extend(power.as_object().expect("an object literal").clone());
        schema(properties, required)
    };
    let mut read_properties = serde_json::json!({
        "id": {"type": "integer"},
        "name": {"type": ["string", "null"]},
        "power_level": {"type": ["integer", "null"]},
        "owner_id": {"type": "string"},
        "archived_at": {"type": ["string", "null"], "format": "date-time"},
        "created_at": {"type": "string", "format": "date-time"},
        "updated_at": {"type": "string", "format": "date-time"},
    });
    read_properties
        .as_object_mut()
        .expect("an object literal")
        .insert(
            power_key.to_string(),
            if v2 {
                serde_json::json!({"type": ["array", "null"], "items": {"type": "string"}})
            } else {
                serde_json::json!({"type": ["string", "null"]})
            },
        );

    let json_path = format!("/crud/v1/heroes/{version}/json");
    let title = if v2 {
        "Heroes"
    } else {
        "Heroes (v1, deprecated)"
    };
    let mut descriptor = ResourceDescriptor::new(
        "heroes",
        title,
        version,
        json_path,
        schema(
            read_properties,
            &["id", "owner_id", "created_at", "updated_at"],
        ),
        body(&["name", power_key]),
        body(&[]),
    );
    descriptor
        .formats
        .insert("xml".to_string(), format!("/crud/v1/heroes/{version}/xml"));
    descriptor.fields = hero_controllers::heroes::HERO_FIELD_SPECS
        .iter()
        .map(|spec| {
            let field = ManifestField::from_spec(spec);
            match spec.name {
                "name" => field.required(),
                "id" | "owner_id" | "archived_at" | "created_at" | "updated_at" => {
                    field.read_only()
                }
                _ => field,
            }
        })
        .collect();
    descriptor.features = Features {
        stats: v2,
        predict: v2,
        conditional_requests: true,
        ..Features::default()
    };
    descriptor.events = Some(EventsDescriptor::new(
        HERO_EVENT_RESOURCE,
        if mode == Mode::Mock {
            Replay::None
        } else {
            Replay::Persistent
        },
    ));
    descriptor.access = Access {
        public_read: false,
        read_roles: hero_controllers::HERO_READ_ROLES
            .iter()
            .map(|r| (*r).to_string())
            .collect(),
        write_roles: hero_controllers::HERO_WRITE_ROLES
            .iter()
            .map(|r| (*r).to_string())
            .collect(),
        delete_roles: hero_controllers::HERO_DELETE_ROLES
            .iter()
            .map(|r| (*r).to_string())
            .collect(),
    };
    descriptor
}

/// The registry behind `GET /api/_crud/manifest`: Hero v2 and the
/// deprecated v1 compat view (`docs/adrs/0030`, FR-0036).
pub fn build_manifest_registry(mode: Mode) -> ManifestRegistry {
    let registry = ManifestRegistry::new();
    for version in ["v2", "v1"] {
        registry
            .register(hero_descriptor(version, mode))
            .expect("hero versions are distinct");
    }
    registry
}

/// `GET /api/_crud/manifest` (filtered to what the caller may read) and
/// the RFC 9727 `GET /.well-known/api-catalog` pointing at it. The
/// catalog's anchor is the configured `PUBLIC_BASE_URL`, not the request's
/// `Host` header, which a client controls; there is no OpenAPI document to
/// advertise.
fn discovery_routers(state: &AppState) -> Router {
    let resource = state.oidc.resource_server();
    let mut config = ManifestConfig::new(
        state.settings.app_name.clone(),
        std::env::var("APP_VERSION").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string()),
    );
    config.oidc_discovery_url = Some(resource.public_discovery_url());
    config.stats_ttl = config::stats_cache_ttl()
        .ok()
        .flatten()
        .map(|ttl| ttl.as_secs());
    config.bulk_max_matched = state.settings.bulk_action_max_matched;
    manifest_router(
        build_manifest_registry(state.settings.mode),
        config,
        ManifestCallers(state.oidc.clone()),
    )
    .merge(api_catalog_router(
        "/api/_crud/manifest",
        None,
        Some(resource.public_base_url.clone()),
    ))
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
/// (`docs/adrs/0026`). Inside those, on every response: the RFC 9728
/// `WWW-Authenticate` challenge on a 401 and the baseline security headers
/// (`docs/adrs/0030`); innermost, `analytics_context`, which runs each
/// handler inside the request's GPC/DNT and consent signals for `track()`
/// (`docs/adrs/0029`). It wraps every route, so no handler can call
/// `track()` outside a request context (which would skip the privacy
/// signals).
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

    let resource_metadata_url: Arc<str> =
        state.oidc.resource_server().resource_metadata_url().into();
    let discovery = discovery_routers(&state);
    let well_known = oidc::well_known::router(state.oidc.resource_server().clone());
    app.with_state(state)
        .merge(discovery)
        .merge(well_known)
        .layer(axum::middleware::from_fn(
            hero_controllers::analytics::analytics_context,
        ))
        .layer(axum::middleware::map_response_with_state(
            resource_metadata_url,
            problem_details::add_resource_metadata,
        ))
        .layer(axum::middleware::map_response(
            http_headers::security_headers,
        ))
        .layer(TraceLayer::new_for_http())
        .layer(ServerSpanLayer::new())
        .layer(RequestIdLayer::new())
}

/// How [`flush_analytics`] ended.
#[derive(Debug, PartialEq, Eq)]
pub enum FlushOutcome {
    /// The flush returned inside the limit.
    Flushed,
    /// The flush was still running at the limit; shutdown carries on.
    TimedOut,
    /// The blocking task panicked or was cancelled.
    Failed,
}

/// Runs `flush` (the blocking analytics flush) off the async workers and
/// waits for it at most `limit`, so a stuck backend cannot hold up the OTLP
/// shutdown behind it. A panic or cancellation (`JoinError`) and a timeout
/// are logged as warnings, never fatal. A timed-out flush keeps running on
/// its blocking thread: it cannot be aborted, and the runtime waits for it
/// when `main` returns, which the library's own HTTP timeout bounds.
pub async fn flush_analytics<F>(limit: std::time::Duration, flush: F) -> FlushOutcome
where
    F: FnOnce() + Send + 'static,
{
    match tokio::time::timeout(limit, tokio::task::spawn_blocking(flush)).await {
        Ok(Ok(())) => FlushOutcome::Flushed,
        Ok(Err(err)) => {
            tracing::warn!("flushing the analytics queue failed: {err}");
            FlushOutcome::Failed
        }
        Err(_) => {
            tracing::warn!(
                "flushing the analytics queue timed out after {}s, shutting down without it",
                limit.as_secs()
            );
            FlushOutcome::TimedOut
        }
    }
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
    async fn the_invalidation_task_starts_only_when_the_cache_is_on_and_stops_on_shutdown() {
        let state = build_state(Arc::new(mock_settings())).await;
        assert!(spawn_stats_cache_invalidation(&state, false).is_none());
        let task = spawn_stats_cache_invalidation(&state, true).expect("enabled starts the task");
        state.shutdown.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .expect("cancelling shutdown ends the task")
            .unwrap();
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

    /// A mock-mode router whose resource-server settings are fixed, not
    /// read from the (devcontainer's) environment.
    async fn router_with_public_url() -> Router {
        let settings = Arc::new(mock_settings());
        let mut state = build_state(settings.clone()).await;
        let mut resource = ResourceServer::from_settings(&settings);
        resource.public_base_url = "https://api.example.com".to_string();
        resource.issuer = "https://id.example.com/realms/r".to_string();
        state.oidc = Arc::new(OidcVerifier::with_resource_server(
            settings,
            Arc::new(resource),
        ));
        build_router(state)
    }

    async fn json_of(response: axum::response::Response) -> serde_json::Value {
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    #[tokio::test]
    async fn every_401_points_at_the_configured_resource_metadata_and_never_the_host_header() {
        let app = router_with_public_url().await;
        let challenge = |response: &axum::response::Response| {
            response
                .headers()
                .get("www-authenticate")
                .expect("every 401 carries WWW-Authenticate")
                .to_str()
                .unwrap()
                .to_string()
        };
        let bare = "Bearer resource_metadata=\"https://api.example.com/.well-known/oauth-protected-resource\"";

        let missing = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/audit")
                    .header("host", "evil.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(challenge(&missing), bare);

        let rejected = app
            .oneshot(
                Request::builder()
                    .uri("/audit")
                    .header("authorization", "Bearer not-a-jwt")
                    .header("host", "evil.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            challenge(&rejected),
            format!("{bare}, error=\"invalid_token\"")
        );
    }

    #[tokio::test]
    async fn the_well_known_documents_and_error_responses_carry_the_security_headers() {
        let app = router_with_public_url().await;
        for path in [
            "/.well-known/oauth-protected-resource",
            "/.well-known/api-catalog",
            "/api/_crud/manifest",
            "/health/live",
            "/audit",
            "/no/such/route",
        ] {
            let response = get(&app, path, None).await;
            let header = |name: &str| {
                response
                    .headers()
                    .get(name)
                    .unwrap_or_else(|| panic!("{path} lacks {name}"))
                    .to_str()
                    .unwrap()
                    .to_string()
            };
            assert_eq!(
                header("content-security-policy"),
                "default-src 'self'; frame-ancestors 'none'",
                "{path}"
            );
            assert_eq!(header("x-content-type-options"), "nosniff", "{path}");
            assert_eq!(header("x-frame-options"), "DENY", "{path}");
            assert_eq!(header("referrer-policy"), "no-referrer", "{path}");
            assert!(
                header("strict-transport-security").starts_with("max-age="),
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn serves_the_protected_resource_metadata_and_the_api_catalog() {
        let app = router_with_public_url().await;

        let metadata =
            json_of(get(&app, "/.well-known/oauth-protected-resource", None).await).await;
        assert_eq!(metadata["resource"], "https://api.example.com");
        assert_eq!(
            metadata["authorization_servers"],
            serde_json::json!(["https://id.example.com/realms/r"])
        );

        // No SECURITY_TXT_CONTACT: nothing is served at all.
        let security_txt = get(&app, "/.well-known/security.txt", None).await;
        assert_eq!(security_txt.status(), StatusCode::NOT_FOUND);

        let catalog = get(&app, "/.well-known/api-catalog", None).await;
        assert_eq!(catalog.status(), StatusCode::OK);
        assert_eq!(
            catalog.headers().get("content-type").unwrap(),
            "application/linkset+json"
        );
        let linkset = json_of(catalog).await;
        let entry = &linkset["linkset"][0];
        // The anchor is configured, not reflected from the request.
        assert_eq!(entry["anchor"], "https://api.example.com");
        assert_eq!(entry["describedby"][0]["href"], "/api/_crud/manifest");
        assert!(entry.get("service-desc").is_none());
    }

    #[tokio::test]
    async fn the_manifest_lists_heroes_only_to_a_caller_who_may_read_them() {
        let app = router_with_public_url().await;

        let anonymous = get(&app, "/api/_crud/manifest", None).await;
        assert_eq!(anonymous.status(), StatusCode::OK);
        assert!(anonymous.headers().get("etag").is_some());
        let manifest = json_of(anonymous).await;
        assert_eq!(manifest["resources"], serde_json::json!([]));

        let token = {
            use base64::Engine;
            let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
                serde_json::json!({
                    "sub": "a",
                    "resource_access": {"api": {"roles": ["viewer"]}}
                })
                .to_string(),
            );
            format!("h.{payload}.s")
        };
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/_crud/manifest")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let manifest = json_of(response).await;
        let resources = manifest["resources"].as_array().unwrap();
        let versions: Vec<(&str, &str)> = resources
            .iter()
            .map(|r| {
                (
                    r["name"].as_str().unwrap(),
                    r["api_version"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(versions, [("heroes", "v1"), ("heroes", "v2")]);
        let v2 = &resources[1];
        assert_eq!(v2["base_path"], "/crud/v1/heroes/v2/json");
        assert_eq!(v2["capabilities"]["stats"], true);
        assert_eq!(
            v2["auth"]["oidc_discovery_url"],
            "https://id.example.com/realms/r/.well-known/openid-configuration"
        );
        assert_eq!(
            v2["auth"]["write_roles"],
            serde_json::json!(["editor", "maintainer"])
        );
        assert!(v2["fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["name"] == "power_level"));
    }

    #[tokio::test]
    async fn flush_analytics_reports_done_timeout_and_panic() {
        use std::time::Duration;
        let limit = Duration::from_secs(5);
        assert_eq!(flush_analytics(limit, || {}).await, FlushOutcome::Flushed);

        // A flush that blocks until the test releases it, so the outcome
        // can't depend on timing: only the timeout can end the wait. (The
        // crate's tokio has no `test-util`, so no paused clock.)
        let (release, blocked) = std::sync::mpsc::channel::<()>();
        assert_eq!(
            flush_analytics(Duration::from_millis(20), move || {
                let _ = blocked.recv();
            })
            .await,
            FlushOutcome::TimedOut,
            "a flush still running at the limit must not block shutdown"
        );
        release.send(()).unwrap();

        assert_eq!(
            flush_analytics(limit, || panic!("flush blew up")).await,
            FlushOutcome::Failed
        );
    }

    #[tokio::test]
    async fn the_analytics_layer_gives_every_route_a_request_context() {
        use axum::routing::get;
        async fn probe() -> String {
            craftainer_observability::analytics::current_request_context()
                .map_or_else(|| "none".to_string(), |ctx| format!("gpc={}", ctx.gpc))
        }
        // The layer `build_router` applies to the whole router: a health-style
        // route gets a context too (harmless there), as does a hero-style one.
        let app: Router = Router::new()
            .route("/health", get(probe))
            .route("/hero", get(probe))
            .layer(axum::middleware::from_fn(
                hero_controllers::analytics::analytics_context,
            ));
        for uri in ["/hero", "/health"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(uri)
                        .header("Sec-GPC", "1")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            assert_eq!(bytes.as_ref(), b"gpc=true", "{uri}");
        }
    }
}
