//! Tauri command surface — BRD §5.11.
//!
//! Commands dispatch through `Application::adapter()` (the wired
//! `VaultAdapter` from Phase 4a) per ADR-030 outcome (a) single-process
//! MCP architecture.
//!
//! ## Layout
//!
//! One module per command category, per BRD §5.11's `src/commands/` file
//! list. Split out at UI slice 2 (from the former flat `commands.rs`) when
//! the vault-administration commands landed: memory CRUD and vault
//! administration are distinct responsibilities, and the combined file would
//! have run past the 500-line cap.
//!
//! **`generate_handler!` must use the defining module path** —
//! `commands::memory::add_memory`, not `commands::add_memory`.
//! `#[tauri::command]` emits hidden companion items (`__cmd__<name>`,
//! `__tauri_command_name_<name>`) alongside each function, and a `pub use`
//! re-export does not carry them, so the macro fails to resolve a
//! re-exported path. The re-exports below exist for the `*_inner` functions
//! (which tests call directly); registration goes through the real path.
//!
//! ## ADR-024 amendment 2026-05-05 (Decision 5(γ)) — TauriCommandInvoke audit
//!
//! Each Tauri command writes a `TauriCommandInvoke` audit row via
//! `VaultAdapter::append_tauri_command_audit` after the operation. The
//! row carries the same `ToolInvokeDetails` shape as `mcp.tool_invoke`
//! but the event_type discriminator distinguishes UI-origin from
//! MCP-origin. Per ADR-024: "audit chain is the authoritative record of
//! vault-state changes" — Tauri commands ARE vault-state changes.
//!
//! Read-only commands audit too: BRD §11.9.1 logs "memory create/read/update/
//! delete" and "boundary creation/deletion", not writes alone.
//!
//! ## Auth-gating posture vs MCP commands
//!
//! Tauri commands operate as `actor_kind = User` (founder is the actor),
//! NOT as `Agent` (which is the ADR-025-locked actor for MCP commands).
//! For V0.1 founder-only dogfood, the founder owns all vault state and
//! auth-gating doesn't apply at the Tauri layer. The ADR-025 amendment
//! auth-gate from Phase 4a remains in place at the MCP/StdioServer layer
//! to protect against untrusted MCP agents — different trust contexts,
//! different auth needs.
//!
//! ## Boundary posture (ADR-SEC-003)
//!
//! The desktop UI reads across all boundaries: boundaries scope *agents*
//! (BRD §11.4.3 rule 5), and the Tauri layer acts as the vault OWNER, who is
//! the party granting that scope. Full reasoning on `VaultAdapter`'s
//! UI-facing read block in `vault-app/src/adapter.rs`.
//!
//! ## Testability pattern
//!
//! Each `#[tauri::command]` wrapper delegates to a sibling `*_inner`
//! async fn that takes `&Application` directly (not wrapped in
//! `State<'_, Application>`). Tests exercise the inner function with a
//! real Application from a test fixture; the `#[tauri::command]`
//! wrapper is a thin glue that converts errors to user-friendly Strings
//! and cannot be tested without the full Tauri runtime.

pub mod account;
pub mod agent;
pub mod boundary;
pub mod connect;
pub mod engine;
pub mod erasure;
pub mod export;
pub mod location;
pub mod logs;
pub mod maintenance;
pub mod memory;
pub mod settings;
pub mod startup;

pub use account::{
    account_access, account_refresh_now, account_sign_in, account_sign_out, account_status,
    account_subscribe, AccountSlot,
};
pub use agent::{list_agents, list_agents_inner, revoke_agent, revoke_agent_inner};
pub use boundary::{
    create_boundary, create_boundary_inner, list_boundaries, list_boundaries_inner,
};
pub use engine::{ensure_recall_engine, RecallEngineFetch};
pub use erasure::{erase_everything, erase_everything_inner};
pub use export::{export_memories, export_memories_inner};
pub use location::LocationContext;
pub use logs::{export_logs, LogContext};
pub use maintenance::{MaintenanceContext, MaintenanceEngineFetch};
pub use memory::{
    add_memory, add_memory_inner, delete_memory, delete_memory_inner, list_recent_memories,
    list_recent_memories_inner, search_memories, search_memories_inner, update_memory,
    update_memory_inner,
};
pub use settings::{get_settings_info, get_settings_info_inner};
pub use startup::Startup;
