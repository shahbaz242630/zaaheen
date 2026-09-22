//! `vault-core` — domain types and error types shared across all Memory Vault crates.
//!
//! This crate is the leaf of the dependency graph (BRD §3.3): it depends on
//! nothing else in the workspace and is depended on by every other `vault-*`
//! crate. Types here express the domain of the vault — memories, entities,
//! relationships, boundaries — and the single error catalogue used by every
//! fallible operation.
//!
//! See `Agent Build Specification.txt` §5.1 for the canonical specification.

#![forbid(unsafe_code)]

pub mod boundary;
pub mod entity;
pub mod error;
pub mod memory;

// Flat re-exports of the most commonly used types so downstream crates can
// `use vault_core::Memory;` rather than `use vault_core::memory::Memory;`.
pub use boundary::{Boundary, MAX_BOUNDARY_LEN};
pub use entity::{
    Entity, EntityId, EntityType, NewEntity, Relationship, RelationshipId, MAX_ENTITY_NAME_BYTES,
    MAX_RELATION_TYPE_BYTES,
};
pub use error::{VaultError, VaultKeyFailure, VaultLocationFailure, VaultResult};
pub use memory::{Memory, MemoryId, MemoryType, NewMemory, MAX_MEMORY_CONTENT_BYTES};
