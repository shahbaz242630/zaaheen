//! Which admin tool serves which desktop command, and whether it is open to a
//! locked computer (ADR-108 D3).
//!
//! The desktop's guard (`vault-tauri/src/guard.rs`) has its own lists of
//! gated and open commands. A test there pins that an admin tool is OPEN
//! exactly when every command it serves is open, so the two lists can never
//! drift apart.

use vault_mcp::LockReason;

/// Whether a tool needs an active subscription.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Always available: what the lock screen itself uses.
    Open,
    /// Only while entitled.
    Gated,
    /// Decided per call by the event it records (`admin_audit_event`).
    PerEvent,
}

/// One admin tool.
#[derive(Clone, Copy, Debug)]
pub struct AdminTool {
    pub name: &'static str,
    /// The desktop commands it serves.
    pub serves: &'static [&'static str],
    pub access: Access,
}

/// Every admin tool. The contract hash beside `PINNED`
/// (`keeper_end_to_end.rs`) covers the same list from the server itself.
pub const ADMIN_TOOLS: &[AdminTool] = &[
    AdminTool {
        name: "admin_memory_add",
        serves: &["add_memory"],
        access: Access::Gated,
    },
    AdminTool {
        name: "admin_memory_search",
        serves: &["search_memories"],
        access: Access::Gated,
    },
    AdminTool {
        name: "admin_memory_update",
        serves: &["update_memory"],
        access: Access::Gated,
    },
    AdminTool {
        name: "admin_memory_delete",
        serves: &["delete_memory"],
        access: Access::Gated,
    },
    AdminTool {
        name: "admin_memory_list_recent",
        serves: &["list_recent_memories"],
        access: Access::Gated,
    },
    AdminTool {
        name: "admin_boundary_list",
        serves: &["list_boundaries"],
        access: Access::Gated,
    },
    AdminTool {
        name: "admin_boundary_create",
        serves: &["create_boundary"],
        access: Access::Gated,
    },
    AdminTool {
        name: "admin_agent_list",
        serves: &["list_agents"],
        access: Access::Gated,
    },
    AdminTool {
        name: "admin_agent_revoke",
        serves: &["revoke_agent"],
        access: Access::Gated,
    },
    AdminTool {
        name: "admin_settings_info",
        serves: &["get_settings_info"],
        access: Access::Open,
    },
    AdminTool {
        name: "admin_engine_status",
        serves: &["recall_engine_state", "ensure_recall_engine"],
        access: Access::Gated,
    },
    AdminTool {
        name: "admin_engine_fetch",
        serves: &["ensure_recall_engine"],
        access: Access::Gated,
    },
    AdminTool {
        name: "admin_engine_warm",
        serves: &["warm_recall_engine"],
        access: Access::Gated,
    },
    AdminTool {
        name: "admin_export_page",
        serves: &["export_memories"],
        access: Access::Open,
    },
    AdminTool {
        name: "admin_audit_event",
        serves: &["export_memories", "export_logs", "set_maintenance_schedule"],
        access: Access::PerEvent,
    },
];

/// The access a tool is registered with.
pub fn access_of(tool: &str) -> Option<Access> {
    ADMIN_TOOLS
        .iter()
        .find(|t| t.name == tool)
        .map(|t| t.access)
}

/// The stable code the desktop shows for a lock reason — the same strings as
/// `vault-tauri`'s `guard::code_for` (a test there pins them equal).
pub fn locked_code(reason: LockReason) -> &'static str {
    match reason {
        LockReason::SignedOut => "locked_signed_out",
        LockReason::CannotConfirm => "locked_cannot_confirm",
        LockReason::TrialEnded => "locked_trial_ended",
        LockReason::SubscriptionEnded => "locked_subscription_ended",
        LockReason::Unlocking => "locked_unlocking",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_admin_tool_is_prefixed_and_listed_once() {
        let mut names: Vec<&str> = ADMIN_TOOLS.iter().map(|t| t.name).collect();
        assert!(names.iter().all(|n| n.starts_with("admin_")), "{names:?}");
        let before = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), before, "a tool listed twice");
    }

    #[test]
    fn only_what_the_lock_screen_uses_is_open() {
        let open: Vec<&str> = ADMIN_TOOLS
            .iter()
            .filter(|t| t.access == Access::Open)
            .map(|t| t.name)
            .collect();
        assert_eq!(open, ["admin_settings_info", "admin_export_page"]);
    }
}
