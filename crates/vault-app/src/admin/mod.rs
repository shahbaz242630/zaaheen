//! The desktop app's admin surface, served by the keeper (ADR-108,
//! ADR-SEC-033; `DESKTOP-CLIENT-DESIGN.md`).
//!
//! The keeper is the only process that opens the vault. The desktop app
//! reaches it over an authenticated `ADMIN` connection (handshake purpose 3)
//! and every screen's data comes from here:
//!
//! - [`ops`]: the desktop's command bodies, moved from `vault-tauri`
//!   unchanged in what they answer and what they audit;
//! - [`AdminServer`]: one MCP tool per operation, gated per [`ADMIN_TOOLS`];
//! - [`AdminHost`]: the full vault, or on a locked computer only the
//!   encrypted database, for the lock screen's numbers and the export;
//! - [`EngineCell`]: the ranking model's one download.

mod engine;
mod host;
pub mod ops;
mod server;
mod store;
mod table;

pub use engine::{Acquire, Acquiring, EngineCell, Fetch};
pub use host::{AdminHost, FullHost, LockedHost};
pub use server::{
    AdminGate, AdminServer, ADMIN_BUSY, ADMIN_CANCELLED, ERR_EXPORT_READ_FAILED, ERR_INVALID_CURSOR,
};
pub use store::{LockedStore, OwnerStore};
pub use table::{access_of, locked_code, Access, AdminTool, ADMIN_TOOLS};
