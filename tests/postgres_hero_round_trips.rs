//! Integration tier: how many statements `repositories::hero_sea_orm` sends
//! per write (`docs/adrs/0028`).
//!
//! A write is one `UPDATE ... RETURNING` however many rows it touches; the
//! read-modify-write loops it replaced cost one SELECT plus one UPDATE per
//! row. The count comes from sea-orm's metric callback, which fires once per
//! statement the connection executes (statement logging is off, `docs/adrs/
//! 0022`, so the log cannot be used).

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use chrono::Utc;
use common::{seed_hero_at, IsolatedDb};
use craftainer_crud::repositories::filtering::{FilterClause, FilterOp, FilterValue};
use craftainer_crud::repositories::{ListOptions, Repository};
use template_axum::hero::repositories::hero_sea_orm::HeroSeaOrmRepository;
use template_axum::hero::views::hero::HeroUpdate;

/// A repository over a connection that counts every statement it runs.
fn counting_repository(db: &IsolatedDb) -> (HeroSeaOrmRepository, Arc<AtomicUsize>) {
    let count = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&count);
    let mut connection = db.connection.clone();
    connection.set_metric_callback(move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
    });
    (HeroSeaOrmRepository::new(connection), count)
}

fn rename(name: &str) -> HeroUpdate {
    HeroUpdate {
        name: Some(name.to_string()),
        powers: None,
        power_level: None,
    }
}

fn owned_by(owner: &str) -> Vec<FilterClause> {
    vec![FilterClause {
        field: "owner_id".to_string(),
        op: FilterOp::Eq,
        value: FilterValue::Str(owner.to_string()),
    }]
}

async fn seed(db: &IsolatedDb, owner: &str, n: usize) -> Vec<i32> {
    let now = Utc::now().naive_utc();
    let mut ids = Vec::new();
    for i in 0..n {
        ids.push(seed_hero_at(&db.connection, owner, &format!("h{i}"), Some(1), now).await);
    }
    ids
}

#[tokio::test]
async fn every_write_is_one_statement_regardless_of_row_count() {
    let db = IsolatedDb::new().await;
    let (repository, count) = counting_repository(&db);
    let ids = seed(&db, "alice", 10).await;

    let updated = repository
        .update(ids[0], "alice", rename("one"))
        .await
        .unwrap();
    assert_eq!(updated.unwrap().name.as_deref(), Some("one"));
    assert_eq!(count.swap(0, Ordering::SeqCst), 1, "update");

    let rows = repository
        .update_many(&owned_by("alice"), rename("all"))
        .await
        .unwrap();
    assert_eq!(rows.len(), 10);
    assert_eq!(count.swap(0, Ordering::SeqCst), 1, "update_many of 10 rows");

    assert!(repository.delete(ids[0], "alice").await.unwrap());
    assert_eq!(count.swap(0, Ordering::SeqCst), 1, "delete");

    let rows = repository
        .delete_many(&owned_by("alice"), true)
        .await
        .unwrap();
    assert_eq!(rows.len(), 9);
    assert_eq!(count.swap(0, Ordering::SeqCst), 1, "delete_many of 9 rows");

    // A miss costs the same single statement, and reports the same values.
    assert!(repository
        .update(ids[0], "alice", rename("x"))
        .await
        .unwrap()
        .is_none());
    assert!(!repository.delete(ids[0], "alice").await.unwrap());
    assert!(repository
        .update_many(&owned_by("nobody"), rename("x"))
        .await
        .unwrap()
        .is_empty());
    assert_eq!(count.swap(0, Ordering::SeqCst), 3, "three misses");

    db.cleanup().await;
}

#[tokio::test]
async fn a_write_from_another_owner_changes_nothing() {
    let db = IsolatedDb::new().await;
    let (repository, _) = counting_repository(&db);
    let ids = seed(&db, "alice", 1).await;

    assert!(repository
        .update(ids[0], "mallory", rename("hacked"))
        .await
        .unwrap()
        .is_none());
    assert!(!repository.delete(ids[0], "mallory").await.unwrap());
    let row = repository.get(ids[0], false).await.unwrap().unwrap();
    assert_eq!(row.name.as_deref(), Some("h0"));

    db.cleanup().await;
}

/// The read-modify-write `update` read a live row, then wrote it without
/// re-checking, so a delete landing in between was overwritten as a live
/// update. One statement decides visibility and writes together: whatever
/// the interleaving, an update that reports `Some` happened before the
/// delete, and an archived row is never touched afterwards.
#[tokio::test]
async fn concurrent_updates_and_a_delete_never_touch_an_archived_row() {
    let db = IsolatedDb::new().await;
    let (repository, _) = counting_repository(&db);
    let repository = Arc::new(repository);
    let id = seed(&db, "alice", 1).await[0];

    let mut updates = Vec::new();
    for i in 0..16 {
        let repository = Arc::clone(&repository);
        updates.push(tokio::spawn(async move {
            repository
                .update(id, "alice", rename(&format!("n{i}")))
                .await
                .unwrap()
        }));
    }
    let deleter = {
        let repository = Arc::clone(&repository);
        tokio::spawn(async move { repository.delete(id, "alice").await.unwrap() })
    };
    let mut winners = 0;
    for update in updates {
        if update.await.unwrap().is_some() {
            winners += 1;
        }
    }
    assert!(deleter.await.unwrap(), "the delete finds the live row");

    let archived = repository.get(id, true).await.unwrap().unwrap();
    assert!(archived.archived_at.is_some());
    // The delete stamps both columns with one value and no update can land
    // after it, so they still agree.
    assert_eq!(Some(archived.updated_at), archived.archived_at);
    let _ = winners;
    let after = repository
        .update(id, "alice", rename("late"))
        .await
        .unwrap();
    assert!(after.is_none(), "an archived row is not updatable");
    let again = repository.get(id, true).await.unwrap().unwrap();
    assert_eq!(again.updated_at, archived.updated_at);

    let listed = repository
        .list(ListOptions {
            skip: 0,
            limit: 10,
            include_archived: false,
            only_archived: false,
            filters: vec![],
            sort: vec![],
        })
        .await
        .unwrap();
    assert!(listed.is_empty());

    db.cleanup().await;
}
