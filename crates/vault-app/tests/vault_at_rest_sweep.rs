//! Whole-vault at-rest sweep: BRD §11.5.1 as an executable gate.
//!
//! # The gap this closes
//!
//! BRD §11.5.1 is unambiguous: *"All data on disk is encrypted. No
//! exceptions."* On 2026-08-15 the vault was violating it — `reports/
//! personal.report.json` sat on disk in plaintext holding verbatim fact
//! text, memory ids, `as_of` timestamps and confidences, readable with no
//! key (ADR-SEC-007).
//!
//! The striking part is that it was NOT found by a test, and could not have
//! been. `vault-storage` already had an excellent sweep —
//! `tests/t0_2_0_acceptance.rs::acceptance_a_no_plaintext_on_disk_after_write_close`
//! walks every file, checks for `PAR1` magic and enforces an entropy floor.
//! But it only ever sees `LanceVectorStore`'s own temp directory. The REPORT
//! is written by `vault-consolidator` into a directory that test never looks
//! at. Every crate was individually compliant; the vault was not.
//!
//! So this test deliberately sits ONE LAYER UP, at `vault-app`, which is the
//! only layer that owns the whole vault directory. It asserts two things
//! that no per-crate test can:
//!
//! 1. **No plaintext.** Distinctive markers are written INTO the vault as
//!    memory content, then every byte of every file is searched for them.
//!    This is direct evidence, not a proxy: entropy is a heuristic, but a
//!    marker appearing verbatim in a file IS the leak.
//! 2. **Inventory closure.** Every entry in the vault directory must be
//!    declared in [`vault_app::erasure::VAULT_ENTRIES`]. A new artifact type
//!    appearing on disk fails here until someone classifies it — which is
//!    the specific thing that would have caught ADR-SEC-007 on the day the
//!    report writer shipped.
//!
//! # Why this runs WITHOUT any ML model
//!
//! It builds the vault from the four real writers directly — SQLCipher,
//! sealed LanceDB, the sealed graph, and the real `write_report_atomic` —
//! rather than through `Application`, whose construction needs the 133 MB
//! BGE model. That keeps the test non-`#[ignore]` and fast, so it runs on
//! every CI pull request. A security control that only runs when someone
//! remembers `--ignored` is the same failure mode in a different costume.
//!
//! # Honest scope
//!
//! This catches (a) any of those four writers regressing to plaintext, and
//! (b) any new file appearing next to them in a vault this test builds. It
//! CANNOT catch a fifth writer that this test never invokes. That residual
//! is why `VAULT_ENTRIES` is shared with the eraser rather than copied: a
//! new artifact has to be declared somewhere, and the declaration is what
//! this test reads.

use std::path::{Path, PathBuf};

use chrono::Utc;
use tempfile::TempDir;
use uuid::Uuid;

use vault_app::erasure::{VAULT_ENTRIES, VAULT_LOCK_FILES};
use vault_consolidator::report::{generate_report, write_report_atomic};
use vault_consolidator::topics::{Topic, TopicMap};
use vault_core::{Boundary, Memory, MemoryType, NewMemory};
use vault_storage::{RetryWorker, SqlCipherKey, StepResult, StorageBackend};

/// Test-only at-rest key. Matches the cross-crate convention used by
/// `vault-consolidator/tests/common/mod.rs`.
const TEST_AT_REST_KEY: [u8; 32] = [0xab; 32];

/// Embedding width. Matches `vault_embedding::EMBEDDING_DIM` without taking
/// a dependency on the embedding crate (which would pull in ORT).
const TEST_DIM: usize = 384;

/// Strings written into the vault as memory content and then hunted for
/// across every byte on disk.
///
/// Chosen to be things that CANNOT occur by chance in an encrypted blob, a
/// schema string, or a file header, so a hit is unambiguous. Each maps to a
/// field that the leaked plaintext REPORT actually exposed in ADR-SEC-007:
/// the fact text, and values a reader could correlate.
const PLAINTEXT_MARKERS: &[&str] = &[
    "ZZQX-CANARY-FACT-TEXT-SHOULD-NEVER-REACH-DISK",
    "ZZQX-CANARY-EMPLOYER-Umbrella-Dynamics",
    "ZZQX-CANARY-DIAGNOSIS-shellfish-anaphylaxis",
];

/// Extra top-level names a live vault legitimately holds that are NOT user
/// data and so are deliberately absent from `VAULT_ENTRIES`.
///
/// `models/` is the documented case: ~3.5 GB of downloaded ML model files
/// carrying no user data, which `erase_vault` deliberately preserves so
/// wiping memories does not force a multi-GB re-download.
const NON_USER_DATA_ENTRIES: &[&str] = &["models"];

fn walk_every_file(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => panic!("read_dir {}: {e}", dir.display()),
        };
        for entry in entries {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}

/// Deterministic unit vector, so no embedding model is needed.
fn unit_vector(seed: usize) -> Vec<f32> {
    let mut v = vec![0.0f32; TEST_DIM];
    v[seed % TEST_DIM] = 1.0;
    v
}

fn make_memory(content: &str, boundary: &Boundary) -> Memory {
    Memory::try_new(NewMemory {
        content: content.to_string(),
        memory_type: MemoryType::Semantic,
        boundary: boundary.clone(),
        source_agent: Some("at-rest-sweep".to_string()),
        confidence: 0.9,
        valid_from: None,
        valid_until: None,
        metadata: serde_json::json!({}),
    })
    .expect("valid memory")
}

/// Build a vault at production filenames, write canary memories through
/// every real writer, close it, and hand back the directory.
async fn build_real_vault(root: &Path) -> Boundary {
    let boundary = Boundary::new("personal").expect("valid boundary");

    // Production filenames on purpose: the inventory assertion compares
    // against VAULT_ENTRIES, which lists the names a REAL vault uses.
    let storage = StorageBackend::open_with_at_rest_key(
        &root.join("vault.db"),
        &root.join("lance"),
        &root.join("graph.duckdb"),
        SqlCipherKey::new("at-rest-sweep-passphrase"),
        TEST_DIM,
        &TEST_AT_REST_KEY,
    )
    .await
    .expect("open sealed StorageBackend");

    let memories: Vec<Memory> = PLAINTEXT_MARKERS
        .iter()
        .map(|m| make_memory(&format!("The user's record: {m}."), &boundary))
        .collect();

    // `write_memory` persists metadata and ENQUEUES the vector write as a
    // cascade entry; the RetryWorker is what actually lands it in Lance. Skip
    // the drain and `lance/` stays empty -- the sweep would then "pass"
    // without ever having inspected the vector store. Mirrors the proven
    // `insert_and_drain` helper in vault-consolidator/tests/common/mod.rs.
    for (i, mem) in memories.iter().enumerate() {
        storage
            .write_memory(mem, &unit_vector(i))
            .await
            .expect("write_memory");
    }
    let mut worker = RetryWorker::new(storage.clone());
    let drain_at = Utc::now() + chrono::Duration::seconds(60);
    let mut drained = 0usize;
    for _ in 0..(memories.len() * 2 + 10) {
        match worker.step_at(drain_at).await.expect("worker step_at") {
            StepResult::Idle => break,
            StepResult::SucceededEntry { .. } => drained += 1,
            other => panic!("unexpected worker outcome during drain: {other:?}"),
        }
    }
    assert_eq!(
        drained,
        memories.len(),
        "cascade did not fully drain; the vector store would be empty and \
         the sweep would inspect nothing"
    );

    // The REPORT is the artifact that actually leaked. Write a real one
    // through the real writer -- not a hand-rolled stand-in, or the test
    // would prove nothing about the code that shipped the bug.
    let topic_map = TopicMap {
        boundary: boundary.clone(),
        topics: vec![Topic {
            topic_id: 0,
            label: "canary_topic".to_string(),
            member_ids: memories.iter().map(|m| m.id).collect(),
        }],
        topic_names_unavailable: false,
    };
    let report = generate_report(&topic_map, &memories, Uuid::new_v4(), Utc::now());
    write_report_atomic(&report, root, &TEST_AT_REST_KEY).expect("write sealed report");

    // Release every internal buffer before walking disk. Lance flushes on
    // drop; SQLCipher checkpoints its WAL. The worker holds its own clone of
    // the backend, so dropping only `storage` would leave a live handle and
    // the sweep could read a half-flushed file.
    drop(worker);
    drop(storage);

    boundary
}

#[tokio::test]
async fn no_vault_file_contains_plaintext_memory_content() {
    let tmp = TempDir::new().expect("temp dir");
    let root = tmp.path();
    build_real_vault(root).await;

    let files = walk_every_file(root);
    assert!(
        !files.is_empty(),
        "no files written under {} -- the vault layout changed and this \
         test would pass vacuously",
        root.display()
    );

    let mut leaks: Vec<String> = Vec::new();
    for path in &files {
        let bytes = std::fs::read(path).expect("read vault file");
        for marker in PLAINTEXT_MARKERS {
            if bytes.windows(marker.len()).any(|w| w == marker.as_bytes()) {
                leaks.push(format!(
                    "{} contains {marker}",
                    path.strip_prefix(root).unwrap_or(path).display()
                ));
            }
        }
    }

    assert!(
        leaks.is_empty(),
        "BRD §11.5.1 violation -- \"All data on disk is encrypted. No \
         exceptions.\" These vault files hold memory content in the clear, \
         readable with no key:\n  {}\n\nThis is the ADR-SEC-007 defect class. \
         Route the artifact through `vault_storage::seal_vault_blob` before \
         writing it.",
        leaks.join("\n  ")
    );
}

#[tokio::test]
async fn vault_directory_contains_nothing_undeclared() {
    let tmp = TempDir::new().expect("temp dir");
    let root = tmp.path();
    build_real_vault(root).await;

    let declared: Vec<&str> = VAULT_ENTRIES
        .iter()
        .copied()
        .chain(VAULT_LOCK_FILES.iter().copied())
        .chain(NON_USER_DATA_ENTRIES.iter().copied())
        .collect();

    let mut undeclared: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(root).expect("read vault root") {
        let name = entry.expect("dir entry").file_name();
        let name = name.to_string_lossy().to_string();
        if !declared.contains(&name.as_str()) {
            undeclared.push(name);
        }
    }
    undeclared.sort();

    assert!(
        undeclared.is_empty(),
        "undeclared artifact(s) in the vault directory: {undeclared:?}\n\n\
         Every file the vault writes must be declared in \
         `vault_app::erasure::VAULT_ENTRIES`. An artifact nobody declared is \
         an artifact nobody decided to encrypt OR to erase -- ADR-SEC-007 was \
         exactly this: `reports/*.json` appeared on disk, was never declared, \
         and shipped in plaintext for weeks.\n\n\
         If the new artifact holds user data: seal it AND add it to \
         VAULT_ENTRIES. If it does not: add it to NON_USER_DATA_ENTRIES here \
         with a comment saying why, as `models/` does."
    );
}

#[tokio::test]
async fn the_sealed_report_is_present_and_is_not_json() {
    // Guards the specific regression: the report must exist (so the sweep
    // above is not passing because nothing was written) AND must not be
    // readable JSON. Without this, deleting the report writer would make
    // both tests above go green.
    let tmp = TempDir::new().expect("temp dir");
    let root = tmp.path();
    build_real_vault(root).await;

    let reports = root.join("reports");
    assert!(reports.is_dir(), "reports/ was never created");

    let files: Vec<PathBuf> = std::fs::read_dir(&reports)
        .expect("read reports dir")
        .map(|e| e.expect("dir entry").path())
        .collect();
    assert_eq!(files.len(), 1, "expected exactly one report, got {files:?}");

    let report_path = &files[0];
    let name = report_path
        .file_name()
        .expect("file name")
        .to_string_lossy()
        .to_string();
    assert!(
        name.ends_with(".report.sealed"),
        "report must use the sealed suffix, got {name}"
    );
    assert!(
        !name.ends_with(".report.json"),
        "the legacy plaintext report suffix must never be written again"
    );

    let bytes = std::fs::read(report_path).expect("read report");
    assert!(!bytes.is_empty(), "sealed report is empty");
    assert_ne!(
        bytes.first(),
        Some(&b'{'),
        "sealed report starts with '{{' -- it is serialised JSON, not a \
         sealed envelope. This is precisely how ADR-SEC-007 presented: \
         `Get-Content reports/personal.report.json` returned readable JSON."
    );
    assert!(
        serde_json::from_slice::<serde_json::Value>(&bytes).is_err(),
        "sealed report parsed as JSON -- it is not encrypted"
    );
}
