//! Entry point: wires up settings, migrations, health checks, OIDC, and
//! routers -- port of `main.py`. See `src/README.md` for the module
//! layering this file sits at the top of.

mod config;
mod controllers;
mod crud;
mod health;
mod http_headers;
mod migration;
mod models;
mod oidc;
mod problem_details;
mod rate_limit;
mod repositories;
mod telemetry;
mod views;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use sea_orm_migration::MigratorTrait;
use tower_http::trace::TraceLayer;

use config::{Mode, Settings};
use controllers::AppState;
use crud::CrudService;
use health::checks::{
    DatabaseHealthCheck, MockHealthCheck, OidcHealthCheck, RedisHealthCheck, S3HealthCheck,
};
use health::HealthRegistry;
use oidc::OidcVerifier;
use rate_limit::RateLimiter;
use repositories::hero_memory::HeroMemoryRepository;
use repositories::hero_sea_orm::HeroSeaOrmRepository;

/// Applies pending SeaORM migrations, off the request path, before the
/// server starts accepting connections (FR-0020). Never runs under
/// `Mode::Mock` -- there's no database to migrate (see
/// `repositories::hero_memory`).
async fn run_migrations(db: &sea_orm::DatabaseConnection) {
    migration::Migrator::up(db, None)
        .await
        .expect("failed to apply pending migrations");
}

async fn build_health_registry(
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

#[tokio::main]
async fn main() {
    telemetry::configure_logging();

    let settings = Settings::from_env().unwrap_or_else(|err| {
        eprintln!("invalid configuration: {err}");
        std::process::exit(1);
    });
    problem_details::configure_detail_redaction(settings.mode);
    let settings = Arc::new(settings);

    let (hero_repository, db_for_health): (
        controllers::DynHeroRepository,
        Option<sea_orm::DatabaseConnection>,
    ) = if settings.mode == Mode::Mock {
        (
            controllers::DynHeroRepository(Box::new(HeroMemoryRepository::new())),
            None,
        )
    } else {
        let db = sea_orm::Database::connect(settings.database_url())
            .await
            .expect("failed to connect to Postgres");
        run_migrations(&db).await;
        (
            controllers::DynHeroRepository(Box::new(HeroSeaOrmRepository::new(db.clone()))),
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

    let state = AppState {
        settings: settings.clone(),
        oidc: Arc::new(OidcVerifier::new(settings.clone())),
        health_registry: Arc::new(health_registry),
        hero_crud: Arc::new(CrudService::new(hero_repository)),
        rate_limiter: Arc::new(rate_limiter),
    };

    let mut app = Router::new()
        .nest("/health", controllers::health::router())
        .nest("/crud/v1/heroes/v2/json", controllers::heroes::router())
        .nest("/crud/v1/heroes/v2/xml", controllers::heroes_xml::router());

    if settings.mode == Mode::Mock {
        app = app.nest("/mock", controllers::mock::router());
    }

    let app = app.with_state(state).layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8000")
        .await
        .expect("failed to bind 0.0.0.0:8000");
    tracing::info!("listening on {}", listener.local_addr().unwrap());

    // `into_make_service_with_connect_info` (rather than plain `app.
    // into_make_service()`) makes the caller's socket address available to
    // handlers via the `ConnectInfo<SocketAddr>` extractor -- src/
    // rate_limit.rs's per-IP check needs it.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .expect("server error");
}
