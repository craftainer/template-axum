//! Integration tier: the `heroes` migration's `down()` (table drop)
//! against real Postgres -- `run_migrations` only ever calls `up()`
//! (FR-0020), so `down()` needs a test to invoke it at all.

mod common;

use common::IsolatedDb;
use sea_orm::ConnectionTrait;
use sea_orm_migration::MigratorTrait;
use template_axum::migration::Migrator;

#[tokio::test]
async fn down_drops_the_heroes_table() {
    let db = IsolatedDb::new().await;

    Migrator::down(&db.connection, None)
        .await
        .expect("down() must drop the heroes table");

    let result = db
        .connection
        .execute_unprepared("SELECT 1 FROM heroes")
        .await;
    assert!(
        result.is_err(),
        "the heroes table must no longer exist after down()"
    );

    db.cleanup().await;
}
