//! What the admin tools read in BOTH keeper modes (ADR-108, and its amendment
//! to ADR-104): the Settings numbers and the export.
//!
//! A full keeper answers through the whole [`VaultAdapter`]. A locked keeper
//! has opened nothing but the encrypted database — through
//! `MetadataStore::open_existing`, which never creates one — and answers
//! through [`LockedStore`]. Both write the same audit rows and page the same
//! way, because both go through the adapter module's shared helpers.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use vault_core::{Memory, MemoryId, VaultResult};
use vault_mcp::ToolInvokeDetails;
use vault_storage::{ActorKind, AuditEventType, BoundaryInfo, MetadataStore};

use crate::adapter::{invoke_audit_event, list_page};
use crate::VaultAdapter;

/// The owner's reads that must work whether or not the subscription is
/// active (export is always available, BRD §1.6 amendment 1).
#[async_trait]
pub trait OwnerStore: Send + Sync {
    /// Active memories across every boundary.
    async fn total_memory_count(&self) -> VaultResult<u64>;
    /// Every registered boundary with its active-memory count.
    async fn list_boundaries(&self) -> VaultResult<Vec<BoundaryInfo>>;
    /// Walk the tamper-evident audit chain (BRD §11.9.2).
    async fn verify_audit_chain(&self) -> VaultResult<()>;
    /// One export page, newest first, after `before`.
    async fn list_memories_page(
        &self,
        before: Option<(DateTime<Utc>, MemoryId)>,
        limit: usize,
    ) -> VaultResult<Vec<Memory>>;
    /// A `TauriCommandInvoke` row with the owner as the actor.
    async fn append_tauri_command_audit(&self, details: ToolInvokeDetails) -> VaultResult<()>;
}

#[async_trait]
impl OwnerStore for VaultAdapter {
    async fn total_memory_count(&self) -> VaultResult<u64> {
        VaultAdapter::total_memory_count(self).await
    }

    async fn list_boundaries(&self) -> VaultResult<Vec<BoundaryInfo>> {
        VaultAdapter::list_boundaries(self).await
    }

    async fn verify_audit_chain(&self) -> VaultResult<()> {
        VaultAdapter::verify_audit_chain(self).await
    }

    async fn list_memories_page(
        &self,
        before: Option<(DateTime<Utc>, MemoryId)>,
        limit: usize,
    ) -> VaultResult<Vec<Memory>> {
        VaultAdapter::list_memories_page(self, before, limit).await
    }

    async fn append_tauri_command_audit(&self, details: ToolInvokeDetails) -> VaultResult<()> {
        VaultAdapter::append_tauri_command_audit(self, details).await
    }
}

/// A locked keeper's view of the vault: the encrypted database and nothing
/// else — no vectors, no graph, no models, no retry worker.
pub struct LockedStore {
    metadata: MetadataStore,
}

impl LockedStore {
    pub(crate) fn new(metadata: MetadataStore) -> Self {
        Self { metadata }
    }
}

#[async_trait]
impl OwnerStore for LockedStore {
    async fn total_memory_count(&self) -> VaultResult<u64> {
        let boundaries = self.metadata.list_boundaries().await?;
        Ok(boundaries.iter().map(|b| b.memory_count).sum())
    }

    async fn list_boundaries(&self) -> VaultResult<Vec<BoundaryInfo>> {
        self.metadata.list_boundaries().await
    }

    async fn verify_audit_chain(&self) -> VaultResult<()> {
        self.metadata.verify_audit_chain().await
    }

    async fn list_memories_page(
        &self,
        before: Option<(DateTime<Utc>, MemoryId)>,
        limit: usize,
    ) -> VaultResult<Vec<Memory>> {
        list_page(&self.metadata, before, limit).await
    }

    async fn append_tauri_command_audit(&self, details: ToolInvokeDetails) -> VaultResult<()> {
        let pending =
            invoke_audit_event(AuditEventType::TauriCommandInvoke, details, ActorKind::User)?;
        self.metadata.append_audit_event(pending).await?;
        Ok(())
    }
}
