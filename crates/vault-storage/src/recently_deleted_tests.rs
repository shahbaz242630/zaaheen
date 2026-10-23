//! Recently deleted (ADR-114) and the AI-app limit (ADR-SEC-049).

use super::*;

use proptest::prelude::*;
use tempfile::TempDir;

use vault_core::{Boundary, MemoryId, MemoryType, NewMemory};

use crate::key::SqlCipherKey;

const DIM: usize = 4;
const TEST_AT_REST_KEY: [u8; 32] = [0xcd; 32];

fn embedding() -> Vec<f32> {
    vec![0.5; DIM]
}

fn memory(content: &str) -> Memory {
    Memory::try_new(NewMemory {
        content: content.into(),
        memory_type: MemoryType::Semantic,
        boundary: Boundary::new("work").unwrap(),
        source_agent: Some("test".into()),
        confidence: 0.8,
        valid_from: None,
        valid_until: None,
        metadata: serde_json::json!({"k": "v"}),
    })
    .unwrap()
}

async fn backend() -> (TempDir, StorageBackend) {
    let tmp = TempDir::new().unwrap();
    let backend = StorageBackend::open_with_at_rest_key(
        &tmp.path().join("vault.db"),
        &tmp.path().join("lance"),
        &tmp.path().join("graph.duckdb"),
        SqlCipherKey::new("recently-deleted-test-key"),
        DIM,
        &TEST_AT_REST_KEY,
    )
    .await
    .unwrap();
    (tmp, backend)
}

/// Save a memory and return it as the vault holds it.
async fn saved(backend: &StorageBackend, content: &str) -> Memory {
    let m = memory(content);
    backend.write_memory(&m, &embedding()).await.unwrap();
    exists(backend, &m.id).await.unwrap()
}

async fn exists(backend: &StorageBackend, id: &MemoryId) -> Option<Memory> {
    backend.metadata().get_memory(id).await.unwrap()
}

async fn queued(backend: &StorageBackend, id: MemoryId, op: CascadeOperation) -> i64 {
    backend
        .metadata()
        .with_transaction(move |tx| {
            tx.query_row(
                "SELECT COUNT(*) FROM retry_queue WHERE memory_id = ?1 AND operation = ?2",
                params![id.0.as_bytes().to_vec(), op.as_str()],
                |row| row.get(0),
            )
            .map_err(|e| VaultError::Storage(e.to_string()))
        })
        .await
        .unwrap()
}

/// An entry made `age` ago by `by`, written straight into the table.
async fn entry_aged(backend: &StorageBackend, content: &str, by: BinBy, age: Duration) {
    let m = memory(content);
    let at = Utc::now() - age;
    backend
        .metadata()
        .with_transaction(move |tx| tx_insert_bin(tx, &m, BinKind::Deleted, by, at).map(|_| ()))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_delete_moves_the_memory_into_recently_deleted() {
    let (_tmp, b) = backend().await;
    let m = saved(&b, "Shahbaz prefers tea").await;

    b.move_to_recently_deleted(&m.id, BinBy::App).await.unwrap();

    assert!(exists(&b, &m.id).await.is_none(), "gone from the vault");
    let bin = b.list_recently_deleted(10, 0).await.unwrap();
    assert_eq!(bin.len(), 1);
    assert_eq!(bin[0].kind, BinKind::Deleted);
    assert_eq!(bin[0].by, BinBy::App);
    assert_eq!(bin[0].memory, m);
    // Still a real delete downstream: the vector delete is queued (ADR-039).
    assert_eq!(queued(&b, m.id, CascadeOperation::Delete).await, 1);
}

#[tokio::test]
async fn restoring_a_deleted_memory_gives_it_back_as_it_was() {
    let (_tmp, b) = backend().await;
    let m = saved(&b, "The project deadline is 2026-11-30").await;
    b.move_to_recently_deleted(&m.id, BinBy::User)
        .await
        .unwrap();
    let entry = b.list_recently_deleted(10, 0).await.unwrap().remove(0);

    b.restore_from_recently_deleted(&entry.entry_id, &embedding())
        .await
        .unwrap();

    assert_eq!(exists(&b, &m.id).await, Some(m.clone()));
    assert_eq!(b.count_recently_deleted().await.unwrap(), 0);
    assert_eq!(
        queued(&b, m.id, CascadeOperation::Write).await,
        2,
        "saved, then restored"
    );
    // A second restore of the same entry finds nothing.
    let again = b
        .restore_from_recently_deleted(&entry.entry_id, &embedding())
        .await
        .unwrap_err();
    assert!(matches!(again, VaultError::NotFound(_)), "{again}");
}

#[tokio::test]
async fn an_edit_keeps_the_previous_wording_and_an_undo_can_be_undone() {
    let (_tmp, b) = backend().await;
    let before = saved(&b, "Shahbaz lives in Dubai").await;
    let mut after = before.clone();
    after.content = "Shahbaz lives in London".into();

    b.update_memory_keeping_previous(&after, &embedding(), BinBy::App)
        .await
        .unwrap();
    assert_eq!(exists(&b, &before.id).await.unwrap().content, after.content);
    let entry = b.list_recently_deleted(10, 0).await.unwrap().remove(0);
    assert_eq!(entry.kind, BinKind::Replaced);
    assert_eq!(entry.memory, before);

    // Undo the edit: the old wording is back, the newer one is in the bin.
    b.restore_from_recently_deleted(&entry.entry_id, &embedding())
        .await
        .unwrap();
    assert_eq!(exists(&b, &before.id).await, Some(before.clone()));
    let bin = b.list_recently_deleted(10, 0).await.unwrap();
    assert_eq!(bin.len(), 1);
    assert_eq!(bin[0].kind, BinKind::Replaced);
    assert_eq!(bin[0].by, BinBy::User);
    assert_eq!(bin[0].memory.content, after.content);
}

#[tokio::test]
async fn the_plain_delete_and_update_keep_nothing() {
    // The nightly tidy-up's rollback and enrichment use these.
    let (_tmp, b) = backend().await;
    let m = saved(&b, "a fact").await;
    let mut changed = m.clone();
    changed.content = "a changed fact".into();
    b.update_memory(&changed, &embedding()).await.unwrap();
    b.delete_memory(&m.id).await.unwrap();
    assert_eq!(b.count_recently_deleted().await.unwrap(), 0);
}

#[tokio::test]
async fn deleting_a_missing_memory_keeps_nothing_and_counts_for_nothing() {
    let (_tmp, b) = backend().await;
    for _ in 0..(APP_CHANGES_PER_HOUR + 5) {
        b.move_to_recently_deleted(&MemoryId::new(), BinBy::App)
            .await
            .unwrap();
    }
    assert_eq!(b.count_recently_deleted().await.unwrap(), 0);
    let m = saved(&b, "still deletable").await;
    b.move_to_recently_deleted(&m.id, BinBy::App).await.unwrap();
}

#[tokio::test]
async fn ai_apps_are_stopped_after_the_hourly_limit_and_change_nothing() {
    let (_tmp, b) = backend().await;
    for i in 0..APP_CHANGES_PER_HOUR {
        entry_aged(&b, &format!("old {i}"), BinBy::App, Duration::minutes(10)).await;
    }
    let m = saved(&b, "must survive").await;

    let err = b
        .move_to_recently_deleted(&m.id, BinBy::App)
        .await
        .unwrap_err();
    assert!(matches!(err, VaultError::RateLimited(_)), "{err}");
    let mut edit = m.clone();
    edit.content = "rewritten".into();
    let err = b
        .update_memory_keeping_previous(&edit, &embedding(), BinBy::App)
        .await
        .unwrap_err();
    assert!(matches!(err, VaultError::RateLimited(_)), "{err}");

    assert_eq!(exists(&b, &m.id).await, Some(m.clone()), "nothing changed");
    assert_eq!(
        b.count_recently_deleted().await.unwrap(),
        APP_CHANGES_PER_HOUR as u64
    );

    // The owner has no limit.
    b.update_memory_keeping_previous(&edit, &embedding(), BinBy::User)
        .await
        .unwrap();
    b.move_to_recently_deleted(&m.id, BinBy::User)
        .await
        .unwrap();
}

#[tokio::test]
async fn only_the_last_hour_of_app_changes_counts() {
    let (_tmp, b) = backend().await;
    for i in 0..APP_CHANGES_PER_HOUR {
        entry_aged(&b, &format!("older {i}"), BinBy::App, Duration::minutes(61)).await;
    }
    for i in 0..(APP_CHANGES_PER_HOUR - 1) {
        entry_aged(&b, &format!("mine {i}"), BinBy::User, Duration::minutes(5)).await;
        entry_aged(&b, &format!("app {i}"), BinBy::App, Duration::minutes(59)).await;
    }
    let m = saved(&b, "the 30th app change").await;
    b.move_to_recently_deleted(&m.id, BinBy::App).await.unwrap();
    let n = saved(&b, "the 31st").await;
    let err = b
        .move_to_recently_deleted(&n.id, BinBy::App)
        .await
        .unwrap_err();
    assert!(matches!(err, VaultError::RateLimited(_)), "{err}");
}

#[tokio::test]
async fn purge_erases_exactly_the_entries_older_than_thirty_days() {
    let (_tmp, b) = backend().await;
    entry_aged(
        &b,
        "expired",
        BinBy::User,
        Duration::days(RECENTLY_DELETED_DAYS) + Duration::minutes(1),
    )
    .await;
    entry_aged(
        &b,
        "kept",
        BinBy::App,
        Duration::days(RECENTLY_DELETED_DAYS) - Duration::minutes(1),
    )
    .await;

    assert_eq!(b.purge_recently_deleted(Utc::now()).await.unwrap(), 1);
    let bin = b.list_recently_deleted(10, 0).await.unwrap();
    assert_eq!(bin.len(), 1);
    assert_eq!(bin[0].memory.content, "kept");
    assert_eq!(b.purge_recently_deleted(Utc::now()).await.unwrap(), 0);
}

#[tokio::test]
async fn an_entry_past_thirty_days_cannot_be_restored_even_before_the_purge() {
    let (_tmp, b) = backend().await;
    entry_aged(
        &b,
        "expired",
        BinBy::User,
        Duration::days(RECENTLY_DELETED_DAYS) + Duration::minutes(1),
    )
    .await;
    let entry = b.list_recently_deleted(1, 0).await.unwrap().remove(0);
    let err = b
        .restore_from_recently_deleted(&entry.entry_id, &embedding())
        .await
        .unwrap_err();
    assert!(matches!(err, VaultError::NotFound(_)), "{err}");
    assert!(exists(&b, &entry.memory.id).await.is_none());
}

#[tokio::test]
async fn the_purge_works_on_the_encrypted_database_alone() {
    // A locked computer opens only vault.db (no vectors, no graph).
    let (_tmp, b) = backend().await;
    entry_aged(
        &b,
        "expired",
        BinBy::App,
        Duration::days(RECENTLY_DELETED_DAYS + 1),
    )
    .await;
    entry_aged(&b, "kept", BinBy::App, Duration::days(1)).await;
    assert_eq!(purge_expired(b.metadata(), Utc::now()).await.unwrap(), 1);
    assert_eq!(b.count_recently_deleted().await.unwrap(), 1);
}

#[tokio::test]
async fn delete_now_and_empty_erase_for_good() {
    let (_tmp, b) = backend().await;
    for content in ["one", "two", "three"] {
        let m = saved(&b, content).await;
        b.move_to_recently_deleted(&m.id, BinBy::User)
            .await
            .unwrap();
    }
    let first = b.list_recently_deleted(1, 0).await.unwrap().remove(0);
    assert_eq!(first.memory.content, "three", "newest first");
    assert!(b
        .erase_from_recently_deleted(&first.entry_id)
        .await
        .unwrap());
    assert!(!b
        .erase_from_recently_deleted(&first.entry_id)
        .await
        .unwrap());
    assert_eq!(b.empty_recently_deleted().await.unwrap(), 2);
    assert_eq!(b.count_recently_deleted().await.unwrap(), 0);
    let gone = b
        .restore_from_recently_deleted(&first.entry_id, &embedding())
        .await
        .unwrap_err();
    assert!(matches!(gone, VaultError::NotFound(_)), "{gone}");
}

#[tokio::test]
async fn the_bin_keeps_no_embedding() {
    let (_tmp, b) = backend().await;
    let mut m = memory("with a vector");
    m.embedding = Some(embedding());
    b.metadata()
        .with_transaction({
            let m = m.clone();
            move |tx| tx_insert_bin(tx, &m, BinKind::Deleted, BinBy::User, Utc::now()).map(|_| ())
        })
        .await
        .unwrap();
    let raw: String = b
        .metadata()
        .with_transaction(|tx| {
            tx.query_row("SELECT memory_json FROM recently_deleted", [], |row| {
                row.get(0)
            })
            .map_err(|e| VaultError::Storage(e.to_string()))
        })
        .await
        .unwrap();
    let back: Memory = serde_json::from_str(&raw).unwrap();
    assert_eq!(back.embedding, None);
}

#[tokio::test]
async fn every_change_is_audited_without_content() {
    let (_tmp, b) = backend().await;
    let m = saved(&b, "private words").await;
    b.move_to_recently_deleted(&m.id, BinBy::App).await.unwrap();
    let entry = b.list_recently_deleted(1, 0).await.unwrap().remove(0);
    b.restore_from_recently_deleted(&entry.entry_id, &embedding())
        .await
        .unwrap();
    b.move_to_recently_deleted(&m.id, BinBy::User)
        .await
        .unwrap();
    b.empty_recently_deleted().await.unwrap();

    let events = b.metadata().list_audit_events(100).await.unwrap();
    let kinds: Vec<_> = events.iter().map(|e| e.event_type).collect();
    assert!(kinds.contains(&AuditEventType::MemoryRestored));
    assert!(kinds.contains(&AuditEventType::BinErased));
    let app_delete = events
        .iter()
        .find(|e| e.event_type == AuditEventType::MemoryDelete && e.actor_kind == ActorKind::Agent)
        .expect("the app's delete is recorded as the agent's");
    assert!(app_delete.details_json.contains("recently_deleted"));
    for e in &events {
        assert!(
            !e.details_json.contains("private words"),
            "{}",
            e.details_json
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 6, ..ProptestConfig::default() })]

    /// Round trip: whatever is deleted comes back identical.
    #[test]
    fn delete_then_restore_round_trips(
        content in "[a-zA-Z0-9 _.,'-]{1,200}",
        confidence in 0.0f32..=1.0,
        boundary_name in "[a-zA-Z0-9_-]{1,32}",
    ) {
        tokio_test::block_on(async {
            let (_tmp, b) = backend().await;
            let mut m = memory("x");
            m.content = content;
            m.confidence = confidence;
            m.boundary = Boundary::new(boundary_name).unwrap();
            b.write_memory(&m, &embedding()).await.unwrap();
            let m = exists(&b, &m.id).await.unwrap();
            b.move_to_recently_deleted(&m.id, BinBy::App).await.unwrap();
            let entry = b.list_recently_deleted(1, 0).await.unwrap().remove(0);
            b.restore_from_recently_deleted(&entry.entry_id, &embedding()).await.unwrap();
            prop_assert_eq!(exists(&b, &m.id).await, Some(m));
            Ok::<(), proptest::test_runner::TestCaseError>(())
        })?;
    }
}
