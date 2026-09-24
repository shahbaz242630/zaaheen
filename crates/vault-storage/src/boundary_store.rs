//! Boundary registry (migration 0008) — the named-boundary read/write surface
//! backing the desktop UI's Boundaries tab.
//!
//! ## What this is NOT
//!
//! This is a registry of boundary NAMES and their metadata. It is **not** an
//! access-control enforcement point. Per BRD §11.4.3 rule 3, boundary filtering
//! happens "BEFORE any retrieval logic — at the storage layer", i.e. in the
//! `WHERE boundary = ?` clause against `memories.boundary`, and rule 4 requires
//! that "the retrieval engine cannot bypass boundary filters even with
//! privileged code paths". Nothing in this module is consulted on a read path,
//! so an absent or stale registry row can never widen what a caller may reach.
//!
//! The registry exists purely so a boundary can be NAMED before it holds any
//! memories — previously a boundary was only implied by the memories inside it,
//! so an empty one could not exist and the UI could not create one.
//!
//! ## Counts
//!
//! [`BoundaryInfo::memory_count`] counts ACTIVE memories only — excluding both
//! consolidator-superseded rows (`superseded_by IS NOT NULL`) and cold-archived
//! rows (`archived_at IS NOT NULL`, ADR-084) — so the number the user sees
//! matches what default retrieval would actually consider.

use chrono::{DateTime, Utc};
use tracing::instrument;

use vault_core::{Boundary, VaultError, VaultResult};

use crate::{MetadataStore, StorageBackend};

/// Maximum length of a user-supplied boundary description, in bytes.
///
/// BRD §11.7.1 fixes limits for memory content / boundary names / entity names
/// but not for this field, which is new at UI slice 2. 256 matches the spec's
/// entity-name bound — the closest analogue (a short human label, not prose).
pub const MAX_BOUNDARY_DESCRIPTION_LEN: usize = 256;

/// A registered boundary plus the live count of active memories inside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundaryInfo {
    /// The boundary itself (already validated by [`Boundary`]'s constructor).
    pub boundary: Boundary,
    /// Optional user-supplied label. `None` when never set.
    pub description: Option<String>,
    /// When the boundary was registered, or — for boundaries backfilled by
    /// migration 0008 — when its earliest memory was written.
    pub created_at: DateTime<Utc>,
    /// Count of active (non-superseded, non-archived) memories in this boundary.
    pub memory_count: u64,
}

/// Validate a user-supplied boundary description per BRD §11.7.1 (every input
/// is adversarial: bound the length, reject control characters).
fn validate_description(description: &str) -> VaultResult<()> {
    if description.len() > MAX_BOUNDARY_DESCRIPTION_LEN {
        return Err(VaultError::InvalidInput(format!(
            "boundary description exceeds {MAX_BOUNDARY_DESCRIPTION_LEN} bytes",
        )));
    }
    if description.chars().any(|c| c.is_control()) {
        return Err(VaultError::InvalidInput(
            "boundary description must not contain control characters".into(),
        ));
    }
    Ok(())
}

impl StorageBackend {
    /// List every registered boundary with its active-memory count,
    /// name-ordered. See [`MetadataStore::list_boundaries`].
    #[instrument(skip_all)]
    pub async fn list_boundaries(&self) -> VaultResult<Vec<BoundaryInfo>> {
        self.metadata().list_boundaries().await
    }
}

impl MetadataStore {
    /// List every registered boundary with its active-memory count,
    /// name-ordered. On the metadata store itself, so a locked keeper can show
    /// the counts without opening the rest of the vault (ADR-108).
    ///
    /// The LEFT JOIN keeps freshly-created empty boundaries in the result with
    /// `memory_count = 0` — an inner join would silently hide exactly the
    /// boundaries this feature exists to make visible.
    #[instrument(skip_all)]
    pub async fn list_boundaries(&self) -> VaultResult<Vec<BoundaryInfo>> {
        self.with_conn_blocking(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT b.name, b.description, b.created_at, \
                                COUNT(m.id) AS memory_count \
                         FROM boundaries b \
                         LEFT JOIN memories m \
                           ON m.boundary = b.name \
                          AND m.superseded_by IS NULL \
                          AND m.archived_at IS NULL \
                         GROUP BY b.name, b.description, b.created_at \
                         ORDER BY b.name",
                )
                .map_err(|e| VaultError::Storage(format!("prepare list boundaries: {e}")))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                })
                .map_err(|e| VaultError::Storage(format!("query boundaries: {e}")))?;

            let mut out = Vec::new();
            for r in rows {
                let (name, description, created_at, count) =
                    r.map_err(|e| VaultError::Storage(format!("read boundary row: {e}")))?;
                out.push(BoundaryInfo {
                    boundary: Boundary::new(name)?,
                    description,
                    created_at: parse_rfc3339(&created_at)?,
                    memory_count: count.max(0) as u64,
                });
            }
            Ok(out)
        })
        .await
    }
}

impl StorageBackend {
    /// Register a new named boundary. Returns `false` when a boundary of that
    /// name already exists (idempotent no-op, not an error — the caller's
    /// desired end state already holds).
    ///
    /// Registering a name grants nothing on its own: what a caller may read is
    /// decided by the authorized-boundary slice passed to retrieval, never by
    /// the presence of a row here.
    #[instrument(skip_all, fields(boundary = %boundary.as_str()))]
    pub async fn create_boundary(
        &self,
        boundary: &Boundary,
        description: Option<&str>,
    ) -> VaultResult<bool> {
        if let Some(d) = description {
            validate_description(d)?;
        }
        let name = boundary.as_str().to_string();
        let description = description.map(|d| d.to_string());
        let created_at = Utc::now().to_rfc3339();

        self.metadata()
            .with_conn_blocking(move |conn| {
                let affected = conn
                    .execute(
                        "INSERT OR IGNORE INTO boundaries (name, description, created_at) \
                         VALUES (?1, ?2, ?3)",
                        rusqlite::params![name, description, created_at],
                    )
                    .map_err(|e| VaultError::Storage(format!("create boundary: {e}")))?;
                Ok(affected > 0)
            })
            .await
    }
}

/// Parse a stored RFC3339 timestamp, mapping a malformed value to a storage
/// error rather than panicking on a corrupt row.
fn parse_rfc3339(raw: &str) -> VaultResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| VaultError::Storage(format!("invalid boundary timestamp '{raw}': {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_at_the_limit_is_accepted() {
        let at_limit = "a".repeat(MAX_BOUNDARY_DESCRIPTION_LEN);
        assert!(validate_description(&at_limit).is_ok());
    }

    #[test]
    fn description_over_the_limit_is_rejected() {
        let too_long = "a".repeat(MAX_BOUNDARY_DESCRIPTION_LEN + 1);
        let err = validate_description(&too_long).unwrap_err();
        assert!(
            matches!(err, VaultError::InvalidInput(_)),
            "expected InvalidInput, got {err:?}"
        );
    }

    #[test]
    fn description_with_control_characters_is_rejected() {
        // Null bytes and newlines are the classic injection / log-forging
        // vectors called out in BRD §11.7.1.
        for bad in ["nul\0byte", "line\nbreak", "carriage\rreturn"] {
            let err = validate_description(bad).unwrap_err();
            assert!(
                matches!(err, VaultError::InvalidInput(_)),
                "expected InvalidInput for {bad:?}, got {err:?}"
            );
        }
    }

    #[test]
    fn description_with_unicode_is_accepted() {
        // Non-ASCII is fine in a human label — unlike boundary NAMES, this
        // field is never interpolated into a filter expression.
        assert!(validate_description("arbeit · 仕事 · travail").is_ok());
    }

    #[test]
    fn parse_rfc3339_rejects_a_corrupt_timestamp() {
        let err = parse_rfc3339("not-a-timestamp").unwrap_err();
        assert!(
            matches!(err, VaultError::Storage(_)),
            "expected Storage error, got {err:?}"
        );
    }

    #[test]
    fn parse_rfc3339_round_trips_a_stored_value() {
        let parsed = parse_rfc3339("2026-01-15T00:00:00+00:00").unwrap();
        assert_eq!(parsed.to_rfc3339(), "2026-01-15T00:00:00+00:00");
    }
}
