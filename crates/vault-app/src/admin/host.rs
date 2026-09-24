//! What an admin connection is served from (ADR-108): the full vault, or — on
//! a locked computer — only what the lock screen needs.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use vault_core::VaultResult;
use vault_storage::{MetadataStore, SqlCipherKey};

use super::engine::EngineCell;
use super::store::{LockedStore, OwnerStore};
use crate::{Application, VaultAdapter};

/// The vault behind an admin connection.
pub enum AdminHost {
    /// An entitled keeper (or a build with no sign-in): the whole vault.
    Full(FullHost),
    /// A locked keeper: the encrypted database, opened only when the lock
    /// screen asks for its numbers or the export, and never created.
    Locked(LockedHost),
}

/// The whole vault.
pub struct FullHost {
    app: Arc<Application>,
    engine: Arc<EngineCell>,
}

impl FullHost {
    pub fn new(app: Arc<Application>, engine: Arc<EngineCell>) -> Self {
        Self { app, engine }
    }

    pub fn adapter(&self) -> &VaultAdapter {
        self.app.adapter()
    }

    pub fn app(&self) -> &Arc<Application> {
        &self.app
    }

    pub fn engine(&self) -> &Arc<EngineCell> {
        &self.engine
    }

    /// Start (or join) the model acquisition, warming the model after it.
    pub fn fetch_engine(&self) {
        let app = Arc::clone(&self.app);
        self.engine.start(move || {
            let _ = app.spawn_reranker_warmup();
        });
    }
}

/// A locked keeper's host: one database handle for the whole process, opened
/// lazily (review B-S5: audit appends are deferred transactions, so two
/// handles could lose a row to a busy snapshot).
pub struct LockedHost {
    vault_db: PathBuf,
    key: SqlCipherKey,
    store: tokio::sync::OnceCell<Option<LockedStore>>,
}

impl LockedHost {
    /// `key` is derived from the master key the lock-mode keeper already read
    /// (read-only) for its handshake; nothing is read twice.
    pub fn new(vault_db: PathBuf, key: SqlCipherKey) -> Self {
        Self {
            vault_db,
            key,
            store: tokio::sync::OnceCell::new(),
        }
    }

    /// The database, or `None` when there is none (nothing is created). A
    /// failed open is not remembered, so the next call tries again.
    ///
    /// # Errors
    ///
    /// The database is there but could not be opened.
    pub async fn store(&self) -> VaultResult<Option<&dyn OwnerStore>> {
        let opened = self
            .store
            .get_or_try_init(|| async {
                MetadataStore::open_existing(&self.vault_db, self.key.clone())
                    .await
                    .map(|m| m.map(LockedStore::new))
            })
            .await?;
        Ok(opened.as_ref().map(|s| s as &dyn OwnerStore))
    }

    pub fn vault_root(&self) -> &Path {
        self.vault_db.parent().unwrap_or(Path::new(""))
    }
}

impl AdminHost {
    /// The folder the memories live in, for Settings.
    pub fn vault_root(&self) -> &Path {
        match self {
            Self::Full(full) => full.app.vault_root(),
            Self::Locked(locked) => locked.vault_root(),
        }
    }
}
