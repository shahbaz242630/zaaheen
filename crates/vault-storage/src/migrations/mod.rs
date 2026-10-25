//! Schema migration runner.
//!
//! Migrations are numbered SQL files embedded into the binary at compile time
//! via [`include_str!`]. The runner is *idempotent*: applying twice on an
//! already-up-to-date database is a no-op. Each migration runs inside a
//! transaction and is recorded in `schema_migrations` only on success.
//!
//! Why include the SQL? Predictable behaviour across machines — no surprise
//! when the working directory is different at runtime, no risk of a partial
//! distribution missing a migration file.

use rusqlite::Connection;
use tracing::{debug, info};

use vault_core::{VaultError, VaultResult};

/// One ordered schema migration.
struct Migration {
    /// Monotonic version. Migrations apply in ascending order; gaps not allowed.
    version: i64,
    /// Short human-readable description, recorded for posterity.
    description: &'static str,
    /// SQL to apply. Must be idempotent at the DDL level (`CREATE TABLE IF NOT EXISTS`).
    up: &'static str,
}

/// All migrations, in order. Append new ones — never edit existing ones.
const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        description: "Initial schema: memories, audit_log",
        up: include_str!("0001_initial.sql"),
    },
    Migration {
        version: 2,
        description: "T0.1.6 cascade infra: retry_queue, dead_letter, pending_sync",
        up: include_str!("0002_cascade_infra.sql"),
    },
    Migration {
        version: 3,
        description: "T0.2.4 sync ship-gate: pending_sync cascade payload (sequence_id + payload)",
        up: include_str!("0003_pending_sync_payload.sql"),
    },
    Migration {
        version: 4,
        description: "T0.2.5 checkpoint & rollback: consolidation_checkpoints + checkpoint_entries",
        up: include_str!("0004_consolidation_checkpoints.sql"),
    },
    Migration {
        version: 5,
        description: "Pillar 2 incremental consolidation (ADR-082): consolidation_state watermark",
        up: include_str!("0005_consolidation_watermark.sql"),
    },
    Migration {
        version: 6,
        description: "A1 cold archive (ADR-084): memories.archived_at marker + partial index",
        up: include_str!("0006_memory_archived_at.sql"),
    },
    Migration {
        version: 7,
        description: "ADR-SEC-001 multi-agent daemon: agent_tokens (per-agent capability tokens)",
        up: include_str!("0007_agent_tokens.sql"),
    },
    Migration {
        version: 8,
        description: "UI slice 2: boundaries registry (first-class rows, backfilled from memories)",
        up: include_str!("0008_boundaries.sql"),
    },
    Migration {
        version: 9,
        description: "ADR-114 Recently deleted: deleted and replaced memories kept 30 days",
        up: include_str!("0009_recently_deleted.sql"),
    },
    Migration {
        version: 10,
        description: "ADR-115 areas: backfill the boundaries registry again and seed personal",
        up: include_str!("0010_areas_backfill.sql"),
    },
];

/// Apply any pending migrations to the open connection. Uses the
/// hard-coded production [`MIGRATIONS`] slice.
///
/// # Errors
///
/// Returns [`VaultError::Storage`] if any migration fails. The transaction
/// for the failing migration is rolled back; previously-applied migrations
/// remain.
pub(crate) fn run(conn: &mut Connection) -> VaultResult<()> {
    run_with_migrations(conn, MIGRATIONS)
}

/// Like [`run`] but accepts a custom migration slice. Crate-private so
/// only tests can substitute it; production callers go through `run`.
fn run_with_migrations(conn: &mut Connection, migrations: &[Migration]) -> VaultResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version     INTEGER PRIMARY KEY,
            applied_at  TEXT    NOT NULL,
            description TEXT    NOT NULL
        );",
    )
    .map_err(|e| VaultError::Storage(format!("failed to create schema_migrations: {e}")))?;

    let current_version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )
        .map_err(|e| VaultError::Storage(format!("failed to read current schema version: {e}")))?;

    debug!(current_version, "checking schema migrations");

    let pending: Vec<&Migration> = migrations
        .iter()
        .filter(|m| m.version > current_version)
        .collect();

    if pending.is_empty() {
        debug!("schema is up to date");
        return Ok(());
    }

    // Migrations must be contiguous — refuse to run if there are gaps,
    // because that almost certainly means migrations were edited or dropped
    // (which can produce silent data corruption).
    for (expected, m) in (current_version + 1..).zip(&pending) {
        if m.version != expected {
            return Err(VaultError::Storage(format!(
                "migration version gap: expected {expected}, found {}",
                m.version,
            )));
        }
    }

    for m in pending {
        info!(version = m.version, description = %m.description, "applying migration");

        let tx = conn
            .transaction()
            .map_err(|e| VaultError::Storage(format!("begin tx: {e}")))?;
        tx.execute_batch(m.up)
            .map_err(|e| VaultError::Storage(format!("migration {}: {e}", m.version)))?;
        tx.execute(
            "INSERT INTO schema_migrations (version, applied_at, description) VALUES (?1, ?2, ?3)",
            rusqlite::params![m.version, chrono::Utc::now().to_rfc3339(), m.description,],
        )
        .map_err(|e| VaultError::Storage(format!("record migration {}: {e}", m.version)))?;
        tx.commit()
            .map_err(|e| VaultError::Storage(format!("commit migration {}: {e}", m.version)))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_memory() -> Connection {
        Connection::open_in_memory().unwrap()
    }

    #[test]
    fn fresh_db_applies_all_migrations() {
        let mut conn = open_memory();
        run(&mut conn).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, MIGRATIONS.len() as i64);
    }

    #[test]
    fn running_twice_is_idempotent() {
        let mut conn = open_memory();
        run(&mut conn).unwrap();
        // Second invocation must succeed and apply nothing new.
        run(&mut conn).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, MIGRATIONS.len() as i64);
    }

    #[test]
    fn migrations_create_expected_tables() {
        let mut conn = open_memory();
        run(&mut conn).unwrap();

        for table in [
            "memories",
            "audit_log",
            "schema_migrations",
            "retry_queue",
            "dead_letter",
            "pending_sync",
        ] {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "expected table {table} to exist");
        }
    }

    #[test]
    fn migration_0002_creates_indexes_for_retry_queue_and_dead_letter() {
        // Performance-critical indexes for the cascade orchestrator's
        // hot paths (worker polling, dead-letter list query).
        let mut conn = open_memory();
        run(&mut conn).unwrap();

        for index in [
            "idx_retry_queue_mem_seq",
            "idx_retry_queue_next_attempt",
            "idx_dead_letter_unresolved",
        ] {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
                    [index],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "expected index {index} to exist");
        }
    }

    #[test]
    fn migration_0002_retry_queue_unique_per_memory_sequence() {
        // The (memory_id, sequence_id) UNIQUE constraint is the FIFO-per-memory
        // anchor (cascade-ordering invariant). Verify the constraint is enforced
        // at the SQL layer.
        let mut conn = open_memory();
        run(&mut conn).unwrap();

        let memory_id = vec![1u8; 16];
        let entry_a = vec![10u8; 16];
        let entry_b = vec![11u8; 16];
        let now = chrono::Utc::now().to_rfc3339();
        let payload = vec![0u8];

        conn.execute(
            "INSERT INTO retry_queue (id, memory_id, operation, payload_format_version, \
             payload, sequence_id, next_attempt_at, created_at) \
             VALUES (?1, ?2, 'write', 1, ?3, 5, ?4, ?4)",
            rusqlite::params![entry_a, memory_id, payload, now],
        )
        .unwrap();

        // Same (memory_id, sequence_id) = collision — must fail.
        let err = conn.execute(
            "INSERT INTO retry_queue (id, memory_id, operation, payload_format_version, \
             payload, sequence_id, next_attempt_at, created_at) \
             VALUES (?1, ?2, 'update', 1, ?3, 5, ?4, ?4)",
            rusqlite::params![entry_b, memory_id, payload, now],
        );
        assert!(err.is_err(), "expected UNIQUE constraint violation");

        // Different sequence_id for the same memory_id — must succeed.
        conn.execute(
            "INSERT INTO retry_queue (id, memory_id, operation, payload_format_version, \
             payload, sequence_id, next_attempt_at, created_at) \
             VALUES (?1, ?2, 'update', 1, ?3, 6, ?4, ?4)",
            rusqlite::params![entry_b, memory_id, payload, now],
        )
        .unwrap();
    }

    #[test]
    fn migration_0003_adds_pending_sync_payload_columns() {
        // T0.2.4 sync ship-gate: the sweep needs sequence_id + payload on
        // pending_sync to reconstruct a retry_queue row. Verify the columns
        // exist after migrating.
        let mut conn = open_memory();
        run(&mut conn).unwrap();

        let cols: std::collections::HashSet<String> = conn
            .prepare("SELECT name FROM pragma_table_info('pending_sync')")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();

        assert!(
            cols.contains("sequence_id"),
            "migration 0003 must add pending_sync.sequence_id; columns: {cols:?}"
        );
        assert!(
            cols.contains("payload"),
            "migration 0003 must add pending_sync.payload; columns: {cols:?}"
        );
    }

    #[test]
    fn migration_0004_creates_checkpoint_tables() {
        // T0.2.5 checkpoint & rollback: the rollback path needs a per-run
        // checkpoint table + a per-changed-memory pre-image table. Verify both
        // tables + their hot-path indexes exist after migrating.
        let mut conn = open_memory();
        run(&mut conn).unwrap();

        for table in ["consolidation_checkpoints", "checkpoint_entries"] {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "expected table {table} to exist");
        }

        for index in [
            "idx_checkpoint_entries_cp",
            "idx_consolidation_checkpoints_created",
        ] {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
                    [index],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "expected index {index} to exist");
        }
    }

    #[test]
    fn migration_0005_creates_consolidation_state_singleton() {
        // Pillar 2 (ADR-082): the incremental-consolidation watermark lives in
        // a single-row consolidation_state table. Verify the table exists, the
        // seed row (id = 1) is present, and the CHECK(id = 1) constraint refuses
        // a second row.
        let mut conn = open_memory();
        run(&mut conn).unwrap();

        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='consolidation_state'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "expected consolidation_state table to exist");

        // The seed row is present with a NULL watermark (no run yet).
        let (id, watermark): (i64, Option<String>) = conn
            .query_row(
                "SELECT id, last_run_started_at FROM consolidation_state",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(id, 1);
        assert!(watermark.is_none(), "watermark starts NULL (full-scan)");

        // CHECK(id = 1) forbids a second singleton row.
        let err = conn.execute(
            "INSERT INTO consolidation_state (id, last_run_started_at) VALUES (2, NULL)",
            [],
        );
        assert!(err.is_err(), "CHECK(id = 1) must reject id != 1");
    }

    #[test]
    fn migration_0006_adds_archived_at_column_and_index() {
        // A1 cold archive (ADR-084): memories gain a nullable archived_at
        // marker (NULL = active) + a partial index over the non-NULL rows so
        // the active-only default-retrieval scan stays cheap. Verify the column
        // and the index both exist after migrating.
        let mut conn = open_memory();
        run(&mut conn).unwrap();

        let cols: Vec<String> = conn
            .prepare("PRAGMA table_info(memories)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(
            cols.iter().any(|c| c == "archived_at"),
            "migration 0006 must add memories.archived_at; columns: {cols:?}"
        );

        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_memories_archived_at'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            exists, 1,
            "expected idx_memories_archived_at index to exist"
        );
    }

    #[test]
    fn migration_0007_creates_agent_tokens_table_and_hash_index() {
        // ADR-SEC-001: per-agent capability tokens live in agent_tokens, with a
        // UNIQUE index on token_hash (the auth-lookup hot path). Verify both
        // exist after migrating.
        let mut conn = open_memory();
        run(&mut conn).unwrap();

        let table: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='agent_tokens'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table, 1, "expected agent_tokens table to exist");

        let index: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_agent_tokens_hash'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            index, 1,
            "expected idx_agent_tokens_hash unique index to exist"
        );
    }

    /// Insert a bare memory row directly, for migration-level tests that need
    /// pre-existing `memories.boundary` values without the full store stack.
    fn insert_bare_memory(conn: &Connection, id: &str, boundary: &str, created_at: &str) {
        conn.execute(
            "INSERT INTO memories \
             (id, content, memory_type, boundary, created_at, valid_from, \
              confidence, last_accessed) \
             VALUES (?1, 'c', 'semantic', ?2, ?3, ?3, 0.9, ?3)",
            rusqlite::params![id, boundary, created_at],
        )
        .unwrap();
    }

    #[test]
    fn migration_0008_creates_boundaries_table_and_always_seeds_default() {
        // The UI must never render an empty boundary list, so `default` exists
        // even in a vault that has never stored a memory.
        let mut conn = open_memory();
        run(&mut conn).unwrap();

        let table: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='boundaries'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table, 1, "expected boundaries table to exist");

        let default_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM boundaries WHERE name = 'default'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            default_rows, 1,
            "migration 0008 must seed the conventional `default` boundary"
        );
    }

    #[test]
    fn migration_0008_backfills_boundaries_from_existing_memories() {
        // An upgrading vault already has boundaries implied by memory rows.
        // Those must appear in the registry immediately — otherwise the
        // Boundaries tab would look empty on a vault full of memories.
        // Apply 0001..=0007 first so memories exist BEFORE 0008 backfills.
        let mut conn = open_memory();
        run_with_migrations(&mut conn, &MIGRATIONS[..7]).unwrap();

        insert_bare_memory(&conn, "m1", "work", "2026-03-01T00:00:00+00:00");
        insert_bare_memory(&conn, "m2", "work", "2026-01-15T00:00:00+00:00");
        insert_bare_memory(&conn, "m3", "personal", "2026-02-01T00:00:00+00:00");

        run(&mut conn).unwrap();

        let mut names: Vec<String> = conn
            .prepare("SELECT name FROM boundaries")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        names.sort();
        assert_eq!(
            names,
            vec![
                "default".to_string(),
                "personal".to_string(),
                "work".to_string()
            ],
            "0008 must backfill every distinct memories.boundary plus `default`"
        );

        // created_at is the EARLIEST memory in that boundary — when the
        // boundary actually came into existence, not when we migrated.
        let work_created: String = conn
            .query_row(
                "SELECT created_at FROM boundaries WHERE name = 'work'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            work_created, "2026-01-15T00:00:00+00:00",
            "backfilled created_at must be MIN(created_at) of that boundary's memories"
        );
    }

    #[test]
    fn migration_0008_is_idempotent_and_preserves_user_edits() {
        // Re-running migrations must not clobber a description the user set.
        let mut conn = open_memory();
        run(&mut conn).unwrap();
        conn.execute(
            "UPDATE boundaries SET description = 'my stuff' WHERE name = 'default'",
            [],
        )
        .unwrap();

        run(&mut conn).unwrap();

        let description: Option<String> = conn
            .query_row(
                "SELECT description FROM boundaries WHERE name = 'default'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            description.as_deref(),
            Some("my stuff"),
            "0008's INSERT OR IGNORE must not overwrite an existing row"
        );

        let default_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM boundaries WHERE name = 'default'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            default_rows, 1,
            "re-running must not duplicate the seed row"
        );
    }

    #[test]
    fn migration_0005_is_idempotent_via_insert_or_ignore() {
        // Running migrations twice must not duplicate or reset the seed row
        // (INSERT OR IGNORE). Pin it by writing a watermark, re-running, and
        // confirming the value survives.
        let mut conn = open_memory();
        run(&mut conn).unwrap();
        conn.execute(
            "UPDATE consolidation_state SET last_run_started_at = '2026-06-17T00:00:00+00:00' WHERE id = 1",
            [],
        )
        .unwrap();

        // Re-run: 0005's INSERT OR IGNORE must not clobber the row.
        run(&mut conn).unwrap();

        let watermark: Option<String> = conn
            .query_row(
                "SELECT last_run_started_at FROM consolidation_state WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            watermark.as_deref(),
            Some("2026-06-17T00:00:00+00:00"),
            "re-running migrations must not reset an existing watermark"
        );
    }

    #[test]
    fn run_records_each_migration_in_schema_migrations_table() {
        let mut conn = open_memory();
        run(&mut conn).unwrap();

        // schema_migrations has one row per applied migration, with the
        // exact version + description we declared in MIGRATIONS.
        for expected in MIGRATIONS {
            let row: (i64, String) = conn
                .query_row(
                    "SELECT version, description FROM schema_migrations WHERE version = ?1",
                    rusqlite::params![expected.version],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(row.0, expected.version);
            assert_eq!(row.1, expected.description);
        }
    }

    #[test]
    fn forward_migration_applies_next_version_only() {
        // Test the principle: an already-applied migration is never replayed,
        // and a new migration appended after it gets applied exactly once.
        // Uses a synthetic migration list throughout (not the production
        // MIGRATIONS slice) so the test is robust against future additions.
        let mut conn = open_memory();

        let initial: &[Migration] = &[Migration {
            version: 1,
            description: "test v1",
            up: "CREATE TABLE m1 (id INTEGER PRIMARY KEY);",
        }];
        run_with_migrations(&mut conn, initial).unwrap();

        let after_v1: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(after_v1, 1);

        let extended: &[Migration] = &[
            // v1 already applied — runner should NOT replay this
            Migration {
                version: 1,
                description: "test v1",
                up: "/* no-op — already applied */",
            },
            Migration {
                version: 2,
                description: "Test forward migration: add a marker table",
                up: "CREATE TABLE migration_marker (id INTEGER PRIMARY KEY);",
            },
        ];
        run_with_migrations(&mut conn, extended).unwrap();

        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 2, "expected v2 to be recorded");

        // Marker table created.
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='migration_marker'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1);
    }

    #[test]
    fn version_gap_in_migration_list_is_rejected() {
        // Defensive check: if someone deletes migration v2 by mistake,
        // leaving v1 then v3, the runner must refuse rather than silently
        // skip v2's effects.
        let mut conn = open_memory();
        let gapped: &[Migration] = &[
            Migration {
                version: 1,
                description: "v1",
                up: "CREATE TABLE m1 (id INTEGER PRIMARY KEY);",
            },
            Migration {
                version: 3,
                description: "v3 (gap! v2 missing)",
                up: "CREATE TABLE m3 (id INTEGER PRIMARY KEY);",
            },
        ];
        let err = run_with_migrations(&mut conn, gapped).unwrap_err();
        assert!(
            matches!(&err, VaultError::Storage(s) if s.contains("version gap")),
            "expected gap detection, got {err:?}",
        );

        // Important: v1 also did NOT apply because gap detection happens
        // before any migration runs. (If gap detection happened after, v1
        // would be partially applied — that's worse.)
        let m1_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='m1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(m1_exists, 0, "no migrations should have applied");
    }

    fn registry_names(conn: &Connection) -> Vec<String> {
        conn.prepare("SELECT name FROM boundaries ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn migration_0010_registers_areas_saved_after_0008() {
        // The founder's vault (s113): 0008 ran on a vault with no memories, so
        // only `default` was registered; AI apps then saved into `personal`
        // and the desktop typed into `default`. `personal` held memories but
        // was missing from the registry, so the Boundaries page hid it.
        let mut conn = open_memory();
        run_with_migrations(&mut conn, &MIGRATIONS[..9]).unwrap();
        insert_bare_memory(&conn, "m1", "personal", "2026-10-06T02:17:18+00:00");
        insert_bare_memory(&conn, "m2", "default", "2026-10-08T10:00:00+00:00");
        insert_bare_memory(&conn, "m3", "Health", "2026-10-25T10:00:00+00:00");
        assert_eq!(registry_names(&conn), vec!["default".to_string()]);

        run(&mut conn).unwrap();

        assert_eq!(
            registry_names(&conn),
            vec![
                "Health".to_string(),
                "default".to_string(),
                "personal".to_string()
            ],
            "0010 must register every area that holds memories"
        );
        let personal_created: String = conn
            .query_row(
                "SELECT created_at FROM boundaries WHERE name = 'personal'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            personal_created, "2026-10-06T02:17:18+00:00",
            "a backfilled area dates from its earliest memory"
        );
    }

    #[test]
    fn migration_0010_seeds_personal_in_an_empty_vault() {
        // AI apps save into `personal` when the user names no area, so it
        // must exist before the first save (an unknown area is refused).
        let mut conn = open_memory();
        run(&mut conn).unwrap();
        assert_eq!(
            registry_names(&conn),
            vec!["default".to_string(), "personal".to_string()]
        );
    }

    #[test]
    fn migration_0010_keeps_existing_labels() {
        let mut conn = open_memory();
        run_with_migrations(&mut conn, &MIGRATIONS[..9]).unwrap();
        conn.execute(
            "INSERT INTO boundaries (name, description, created_at) \
             VALUES ('personal', 'mine', '2026-01-01T00:00:00+00:00')",
            [],
        )
        .unwrap();
        insert_bare_memory(&conn, "m1", "personal", "2026-10-06T02:17:18+00:00");

        run(&mut conn).unwrap();

        let description: Option<String> = conn
            .query_row(
                "SELECT description FROM boundaries WHERE name = 'personal'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(description.as_deref(), Some("mine"));
    }
}
