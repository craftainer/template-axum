//! `src/db.rs` against the devcontainer stack's real Postgres: the pool
//! sea-orm/sqlx builds actually carries the `DB_POOL_*` sizes and timeouts,
//! and closing one handle closes the shared pool.

mod common;

use std::time::Duration;

use template_axum::config::{DbSettings, PostgresSslMode, Settings};
use template_axum::db;

fn settings_with(db: DbSettings) -> Settings {
    Settings {
        db,
        ..(*common::dev_settings(None)).clone()
    }
}

#[tokio::test]
async fn pool_sizes_and_timeouts_reach_the_real_pool() {
    let settings = settings_with(DbSettings {
        pool_size: Some(3),
        pool_min_size: Some(1),
        pool_recycle: Some(Duration::from_secs(1800)),
        pool_idle_timeout: Some(Duration::from_secs(300)),
        connect_timeout: Some(Duration::from_secs(7)),
        sslmode: Some(PostgresSslMode::Prefer),
        ..DbSettings::default()
    });
    let conn = db::connect(&settings)
        .await
        .expect("the devcontainer stack's Postgres must be running for the integration tier");
    let pool = conn.get_postgres_connection_pool();
    let options = pool.options();
    assert_eq!(options.get_max_connections(), 3);
    assert_eq!(options.get_min_connections(), 1);
    assert_eq!(options.get_max_lifetime(), Some(Duration::from_secs(1800)));
    assert_eq!(options.get_idle_timeout(), Some(Duration::from_secs(300)));
    assert_eq!(options.get_acquire_timeout(), Duration::from_secs(7));
    conn.ping().await.expect("the pool must serve a query");
    conn.close().await.expect("closing the pool must succeed");
}

#[tokio::test]
async fn closing_the_pool_closes_every_clone() {
    let conn = db::connect(&settings_with(DbSettings::default()))
        .await
        .expect("the devcontainer stack's Postgres must be running for the integration tier");
    let clone = conn.clone();
    let pool = conn.get_postgres_connection_pool().clone();
    assert!(!pool.is_closed());
    conn.close().await.expect("closing the pool must succeed");
    assert!(pool.is_closed());
    assert!(
        clone.ping().await.is_err(),
        "a clone must see the closed pool"
    );
}
