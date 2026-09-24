//! Boundary commands — BRD §5.11 `commands/boundary.rs`.
//!
//! Shared context (audit posture, ADR-SEC-003 boundary posture, testability
//! pattern) is documented on the parent module. Since ADR-108 (D4) the
//! bodies run in the keeper (`vault_app::admin::ops`); these ask the guard
//! and forward.
//!
//! Registering a boundary here grants nothing on its own. What a caller may
//! read is decided by the authorized-boundary slice passed to retrieval, never
//! by the presence of a registry row (BRD §11.4.3 rules 3 and 4).

use serde_json::{json, Value};
use tauri::State;

use crate::guard::{Entitled, Entitlement};
use crate::link::{decoded, KeeperLink, Kind};

/// `list_boundaries`.
pub async fn list_boundaries_inner(
    link: &KeeperLink,
    _entitled: &Entitled,
) -> Result<Vec<Value>, String> {
    let text = link
        .call("admin_boundary_list", json!({}), Kind::Read)
        .await?;
    decoded(&text)
}

#[tauri::command]
pub async fn list_boundaries(
    link: State<'_, KeeperLink>,
    entitlement: State<'_, Entitlement>,
) -> Result<Vec<Value>, String> {
    let entitled = entitlement.require().await?;
    list_boundaries_inner(link.inner(), &entitled).await
}

/// `create_boundary`: `true` when a new boundary was registered, `false` when
/// one of that name already existed. The keeper validates the name with
/// `Boundary::new` before touching the vault (BRD §11.7.1).
pub async fn create_boundary_inner(
    link: &KeeperLink,
    _entitled: &Entitled,
    name: String,
    description: Option<String>,
) -> Result<bool, String> {
    let text = link
        .call(
            "admin_boundary_create",
            json!({ "name": name, "description": description }),
            Kind::Write,
        )
        .await?;
    decoded(&text)
}

#[tauri::command]
pub async fn create_boundary(
    link: State<'_, KeeperLink>,
    entitlement: State<'_, Entitlement>,
    name: String,
    description: Option<String>,
) -> Result<bool, String> {
    let entitled = entitlement.require().await?;
    create_boundary_inner(link.inner(), &entitled, name, description).await
}

#[cfg(test)]
mod tests {
    use vault_core::Boundary;

    /// The keeper's first act is `Boundary::new`, so every name the type
    /// rejects is rejected before any vault I/O happens. These pin the cases
    /// that matter for a UI text field wired straight to this command.
    #[test]
    fn boundary_name_validation_rejects_injection_shaped_input() {
        for bad in [
            "",                 // empty
            "has space",        // whitespace
            "quote'breakout",   // SQL quote breakout (LanceDB only_if)
            "semi;colon",       // statement separator
            "nul\0byte",        // null byte
            "new\nline",        // control character
            "unicode·dot",      // non-ASCII
            "../../etc/passwd", // path traversal shape
        ] {
            assert!(
                Boundary::new(bad).is_err(),
                "boundary name {bad:?} must be rejected before reaching storage"
            );
        }
    }

    #[test]
    fn boundary_name_validation_rejects_over_length_names() {
        // BRD §11.7.1: boundary names cap at 64 bytes.
        let too_long = "a".repeat(65);
        assert!(
            Boundary::new(&too_long).is_err(),
            "a 65-byte boundary name must be rejected"
        );
        let at_limit = "a".repeat(64);
        assert!(
            Boundary::new(&at_limit).is_ok(),
            "a 64-byte boundary name is within the documented limit"
        );
    }

    #[test]
    fn boundary_name_validation_accepts_realistic_names() {
        for good in [
            "default",
            "work",
            "personal",
            "side-project",
            "client_acme",
            "q3",
        ] {
            assert!(
                Boundary::new(good).is_ok(),
                "boundary name {good:?} should be accepted"
            );
        }
    }
}
