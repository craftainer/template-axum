//! `connect_database`'s `RUN_MIGRATIONS_ON_STARTUP` switch against a
//! throwaway database on the devcontainer stack's Postgres. A database of
//! its own (not a schema) because `Settings::database_url` honours
//! `DATABASE_URL`, which this binary's one test sets for itself; it is the
//! only test in this binary, so nothing else reads the variable meanwhile.

mod common;

use sea_orm::{ConnectionTrait, Database, DatabaseBackend, Statement};
use template_axum::config::{DbSettings, Settings};
use template_axum::connect_database;

async fn table_exists(db: &sea_orm::DatabaseConnection, table: &str) -> bool {
    db.query_one_raw(Statement::from_string(
        DatabaseBackend::Postgres,
        format!("SELECT to_regclass('public.{table}')::text AS name"),
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get::<Option<String>>("", "name")
    .unwrap()
    .is_some()
}

#[tokio::test]
async fn migrations_run_on_startup_unless_switched_off() {
    let base = (*common::dev_settings(None)).clone();
    let admin = Database::connect(base.database_url())
        .await
        .expect("the devcontainer stack's Postgres must be running for the integration tier");
    let name = format!("cd_{}", common::unique_suffix());
    admin
        .execute_unprepared(&format!(r#"CREATE DATABASE "{name}""#))
        .await
        .expect("failed to create the throwaway database");

    let url = format!(
        "postgres://{}:{}@{}:{}/{name}",
        base.postgres_user, base.postgres_password, base.postgres_host, base.postgres_port
    );
    unsafe { std::env::set_var("DATABASE_URL", &url) };

    let off = Settings {
        db: DbSettings {
            run_migrations_on_startup: false,
            ..DbSettings::default()
        },
        ..base.clone()
    };
    let db = connect_database(&off).await.expect("a real mode connects");
    let migrated_when_off = table_exists(&db, "heroes").await;
    db.close().await.unwrap();

    let on = Settings {
        db: DbSettings::default(),
        ..base
    };
    let db = connect_database(&on).await.expect("a real mode connects");
    let migrated_when_on = table_exists(&db, "heroes").await;
    db.close().await.unwrap();

    // Clean up before asserting, so a failure leaves nothing behind.
    unsafe { std::env::remove_var("DATABASE_URL") };
    let _ = admin
        .execute_unprepared(&format!(r#"DROP DATABASE "{name}" WITH (FORCE)"#))
        .await;
    assert!(
        !migrated_when_off,
        "RUN_MIGRATIONS_ON_STARTUP=false must leave the schema alone"
    );
    assert!(migrated_when_on, "the default applies pending migrations");
}
