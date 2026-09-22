//! Where the memories live — the screens' commands (ADR-105 L-e,
//! `VAULT-KEY-AND-LOCATION.md`): where they are and what the last start did
//! about a move, checking a chosen folder, moving them there (the app
//! restarts to do it, L5), and letting go of an old copy on a drive that
//! never comes back (L-d decision 9).
//!
//! ## Gated, all four
//!
//! None is on §8.26 §6.4's locked allowlist, and widening that list is a
//! founder decision (`guard::the_open_list_is_exactly_the_locked_allowlist`).
//! Nothing needs it widened: a locked computer shows the lock screen, where
//! the export already takes the memories anywhere, and the onboarding's
//! location step comes after its sign-in step (L-e decision 1).
//!
//! ## What reaches the screen
//!
//! Stable codes and folders as the person reads them
//! ([`vault_app::location::display_path`]: never the `\\?\` form the record
//! keeps). The words live in `dist/app.js`, and every code here has them —
//! `tests::every_location_code_has_words_in_the_app`.

use std::path::Path;

use serde_json::{json, Value};
use tauri::State;
use vault_app::keychain::KeyLocation;
use vault_app::location::check::{Checked, Note, Refusal, SystemEnv};
use vault_app::location::moving::{
    self, ForgetRefusal, MoveFailure, MoveOutcome, RequestRefusal, Status,
};
use vault_app::location::{display_path, Homes};

use crate::guard::Entitlement;

/// Opaque code: the work could not be finished (a task that stopped).
pub const ERR_LOCATION_FAILED: &str = "location_failed";

/// What the location commands work with: this person's folders, the key's
/// location (its lock, K1), and what this start did about a move.
pub struct LocationContext {
    homes: Homes,
    key: KeyLocation,
    at_start: MoveOutcome,
}

impl LocationContext {
    /// Built once in `setup()`, after the start's move has run.
    #[must_use]
    pub fn new(homes: Homes, key: KeyLocation, at_start: MoveOutcome) -> Self {
        Self {
            homes,
            key,
            at_start,
        }
    }
}

// ── codes ─────────────────────────────────────────────────────────────────

/// Why a folder was refused (L4).
#[must_use]
pub fn folder_refusal_code(refusal: Refusal) -> &'static str {
    match refusal {
        Refusal::NotLocal => "location_not_local",
        Refusal::Network => "location_network",
        Refusal::Unreachable => "location_unreachable",
        Refusal::DriveRoot => "location_drive_root",
        Refusal::SystemFolder => "location_system_folder",
        Refusal::InsideAppFolders => "location_app_folder",
        Refusal::CloudSynced => "location_cloud_synced",
        Refusal::AlreadyExists => "location_already_exists",
        Refusal::NotWritable => "location_not_writable",
        Refusal::NotEnoughSpace => "location_not_enough_space",
    }
}

/// Why a move could not be asked for (L5).
#[must_use]
pub fn refusal_code(refusal: RequestRefusal) -> &'static str {
    match refusal {
        RequestRefusal::Folder(folder) => folder_refusal_code(folder),
        RequestRefusal::OldCopyWaiting => "location_old_copy_waiting",
        RequestRefusal::MoveWaiting => "location_move_waiting",
        RequestRefusal::ErasureUnfinished => "location_erasure_unfinished",
        RequestRefusal::VaultUnavailable => "location_unavailable",
        RequestRefusal::RecordFailed => "location_record_failed",
    }
}

/// Why an old copy was not forgotten.
#[must_use]
pub fn forget_code(refusal: ForgetRefusal) -> &'static str {
    match refusal {
        ForgetRefusal::NothingWaiting => "location_nothing_to_forget",
        ForgetRefusal::StillThere => "location_old_copy_still_there",
        ForgetRefusal::RecordFailed => "location_record_failed",
    }
}

/// What to tell the person about an accepted folder (L4).
#[must_use]
pub fn note_code(note: Note) -> &'static str {
    match note {
        Note::OtherDrive => "other_drive",
        Note::CloudUnchecked => "cloud_unchecked",
    }
}

/// Why a start's move attempt failed.
#[must_use]
pub fn failure_code(failure: MoveFailure) -> &'static str {
    match failure {
        MoveFailure::FolderNotUsable => "folder_not_usable",
        MoveFailure::NotEnoughSpace => "not_enough_space",
        MoveFailure::CopyFailed => "copy_failed",
        MoveFailure::CopyDidNotMatch => "copy_did_not_match",
        MoveFailure::Interrupted => "interrupted",
        MoveFailure::MemoriesErased => "memories_erased",
        MoveFailure::RecordFailed => "record_failed",
    }
}

// ── what the screens read ────────────────────────────────────────────────

/// What this start did about a move, for the screen. `Busy` never gets here
/// (that window shows `MSG_MEMORIES_BUSY` and exits before this is built);
/// it reads as nothing rather than as anything untrue.
#[must_use]
pub fn outcome_json(outcome: &MoveOutcome) -> Value {
    match outcome {
        MoveOutcome::Nothing | MoveOutcome::Busy => json!({ "kind": "nothing" }),
        MoveOutcome::Moved {
            to,
            not_restricted,
            old_copy_waiting,
        } => json!({
            "kind": "moved",
            "to": display_path(to),
            "not_restricted": not_restricted,
            "old_copy_waiting": old_copy_waiting,
        }),
        MoveOutcome::OldCopyRemoved => json!({ "kind": "old_copy_removed" }),
        MoveOutcome::OldCopyWaiting => json!({ "kind": "old_copy_waiting" }),
        MoveOutcome::Failed { reason, retrying } => json!({
            "kind": "failed",
            "reason": failure_code(*reason),
            "retrying": retrying,
        }),
        MoveOutcome::Deferred => json!({ "kind": "deferred" }),
    }
}

/// Where things stand, plus what this start did.
#[must_use]
pub fn status_json(status: &Status, at_start: &MoveOutcome) -> Value {
    json!({
        "folder": display_path(&status.folder),
        "move_waiting": status.move_waiting.as_deref().map(display_path),
        "old_copy": status.old_copy.as_ref().map(|c| json!({
            "from": display_path(&c.from),
            "connected": c.connected,
        })),
        "at_start": outcome_json(at_start),
    })
}

/// An accepted folder: where the memories would go, and what to say.
#[must_use]
pub fn checked_json(checked: &Checked) -> Value {
    json!({
        "target": display_path(&checked.target),
        "notes": checked.notes.iter().map(|n| note_code(*n)).collect::<Vec<_>>(),
    })
}

/// Blocking work (the record, the folder checks, `reg.exe`) off the async
/// runtime, per BRD §2.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(f).await.map_err(|e| {
        tracing::error!(target: "vault_tauri::location", error = %e, "a location task stopped");
        ERR_LOCATION_FAILED.to_string()
    })
}

// ── the commands ─────────────────────────────────────────────────────────

/// Where the memories are, what is waiting, and what this start did.
#[tauri::command]
pub async fn location_status(
    entitlement: State<'_, Entitlement>,
    ctx: State<'_, LocationContext>,
) -> Result<Value, String> {
    let _entitled = entitlement.require().await?;
    let homes = ctx.homes.clone();
    let status = blocking(move || moving::status(&homes)).await?;
    status
        .map(|s| status_json(&s, &ctx.at_start))
        .map_err(|_| refusal_code(RequestRefusal::VaultUnavailable).to_string())
}

/// Check a folder the person picked, recording nothing: where the memories
/// would go and what to tell them, or why not.
#[tauri::command]
pub async fn location_check(
    entitlement: State<'_, Entitlement>,
    ctx: State<'_, LocationContext>,
    folder: String,
) -> Result<Value, String> {
    let _entitled = entitlement.require().await?;
    let (homes, key) = (ctx.homes.clone(), ctx.key.clone());
    blocking(move || moving::check_request(&homes, &key, Path::new(&folder), &SystemEnv))
        .await?
        .map(|checked| checked_json(&checked))
        .map_err(|r| refusal_code(r).to_string())
}

/// Ask for the move (L5), then restart: the next start makes it, before
/// anything opens the memories. Nothing moves if this is refused.
#[tauri::command]
pub async fn location_move(
    app: tauri::AppHandle,
    entitlement: State<'_, Entitlement>,
    ctx: State<'_, LocationContext>,
    folder: String,
) -> Result<Value, String> {
    let _entitled = entitlement.require().await?;
    let (homes, key) = (ctx.homes.clone(), ctx.key.clone());
    let checked =
        blocking(move || moving::request_move(&homes, &key, Path::new(&folder), &SystemEnv))
            .await?
            .map_err(|r| refusal_code(r).to_string())?;
    tracing::info!(target: "vault_tauri::location", "a move was asked for; restarting to make it");
    app.request_restart();
    Ok(checked_json(&checked))
}

/// Stop waiting for an old copy whose folder cannot be seen. Removes
/// nothing; answers the folder, so the person can be told to delete it if it
/// turns up.
#[tauri::command]
pub async fn location_forget_old_copy(
    entitlement: State<'_, Entitlement>,
    ctx: State<'_, LocationContext>,
) -> Result<Value, String> {
    let _entitled = entitlement.require().await?;
    let (homes, key) = (ctx.homes.clone(), ctx.key.clone());
    blocking(move || moving::forget_old_copy(&homes, &key))
        .await?
        .map(|from| json!({ "from": display_path(&from) }))
        .map_err(|r| forget_code(r).to_string())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use vault_app::location::moving::OldCopy;

    use super::*;

    const ALL_FOLDER_REFUSALS: &[Refusal] = &[
        Refusal::NotLocal,
        Refusal::Network,
        Refusal::Unreachable,
        Refusal::DriveRoot,
        Refusal::SystemFolder,
        Refusal::InsideAppFolders,
        Refusal::CloudSynced,
        Refusal::AlreadyExists,
        Refusal::NotWritable,
        Refusal::NotEnoughSpace,
    ];

    const ALL_FAILURES: &[MoveFailure] = &[
        MoveFailure::FolderNotUsable,
        MoveFailure::NotEnoughSpace,
        MoveFailure::CopyFailed,
        MoveFailure::CopyDidNotMatch,
        MoveFailure::Interrupted,
        MoveFailure::MemoriesErased,
        MoveFailure::RecordFailed,
    ];

    /// Every code a refused command can answer with.
    fn refusal_codes() -> Vec<&'static str> {
        let mut codes: Vec<&str> = ALL_FOLDER_REFUSALS
            .iter()
            .map(|r| refusal_code(RequestRefusal::Folder(*r)))
            .collect();
        codes.extend(
            [
                RequestRefusal::OldCopyWaiting,
                RequestRefusal::MoveWaiting,
                RequestRefusal::ErasureUnfinished,
                RequestRefusal::VaultUnavailable,
                RequestRefusal::RecordFailed,
            ]
            .map(refusal_code),
        );
        codes.extend(
            [
                ForgetRefusal::NothingWaiting,
                ForgetRefusal::StillThere,
                ForgetRefusal::RecordFailed,
            ]
            .map(forget_code),
        );
        codes.push(ERR_LOCATION_FAILED);
        codes
    }

    fn drive(path: &str) -> PathBuf {
        PathBuf::from(format!(r"\\?\{path}"))
    }

    #[test]
    fn each_refusal_has_its_own_stable_code() {
        let codes = refusal_codes();
        let mut distinct = codes.clone();
        distinct.sort_unstable();
        distinct.dedup();
        // `location_record_failed` is shared on purpose: the same fault,
        // the same words.
        assert_eq!(distinct.len(), codes.len() - 1, "{codes:?}");
        for code in codes {
            assert!(code.starts_with("location_"), "{code}");
            assert!(
                code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
                "{code}"
            );
        }
    }

    #[test]
    fn a_move_shows_the_folder_as_the_person_reads_it() {
        let moved = outcome_json(&MoveOutcome::Moved {
            to: drive(r"D:\Backups\Zaaheen Memories"),
            not_restricted: true,
            old_copy_waiting: false,
        });
        assert_eq!(
            moved,
            json!({
                "kind": "moved",
                "to": r"D:\Backups\Zaaheen Memories",
                "not_restricted": true,
                "old_copy_waiting": false,
            })
        );
    }

    #[test]
    fn each_outcome_names_its_kind() {
        for (outcome, kind) in [
            (MoveOutcome::Nothing, "nothing"),
            (MoveOutcome::OldCopyRemoved, "old_copy_removed"),
            (MoveOutcome::OldCopyWaiting, "old_copy_waiting"),
            (MoveOutcome::Deferred, "deferred"),
            (MoveOutcome::Busy, "nothing"),
        ] {
            assert_eq!(outcome_json(&outcome)["kind"], kind, "{outcome:?}");
        }
        for failure in ALL_FAILURES {
            let failed = outcome_json(&MoveOutcome::Failed {
                reason: *failure,
                retrying: false,
            });
            assert_eq!(failed["kind"], "failed");
            assert_eq!(failed["reason"], failure_code(*failure));
            assert_eq!(failed["retrying"], false);
        }
    }

    #[test]
    fn the_status_shows_every_folder_as_the_person_reads_it() {
        let status = Status {
            folder: drive(r"E:\Zaaheen Memories"),
            move_waiting: Some(drive(r"F:\Zaaheen Memories")),
            old_copy: Some(OldCopy {
                from: PathBuf::from(r"C:\Users\me\AppData\Roaming\com.zaaheen.app"),
                connected: false,
            }),
        };
        assert_eq!(
            status_json(&status, &MoveOutcome::Deferred),
            json!({
                "folder": r"E:\Zaaheen Memories",
                "move_waiting": r"F:\Zaaheen Memories",
                "old_copy": {
                    "from": r"C:\Users\me\AppData\Roaming\com.zaaheen.app",
                    "connected": false,
                },
                "at_start": { "kind": "deferred" },
            })
        );
        let plain = Status {
            folder: PathBuf::from(r"C:\v"),
            move_waiting: None,
            old_copy: None,
        };
        let shown = status_json(&plain, &MoveOutcome::Nothing);
        assert!(shown["move_waiting"].is_null() && shown["old_copy"].is_null());
    }

    #[test]
    fn a_checked_folder_carries_its_notes() {
        let checked = Checked {
            target: drive(r"E:\Zaaheen Memories"),
            notes: vec![Note::OtherDrive, Note::CloudUnchecked],
        };
        assert_eq!(
            checked_json(&checked),
            json!({
                "target": r"E:\Zaaheen Memories",
                "notes": ["other_drive", "cloud_unchecked"],
            })
        );
    }

    /// A code with no words would reach the person raw, so every code
    /// ships with its line (the same rule as the lock codes).
    #[test]
    fn every_location_code_has_words_in_the_app() {
        let app_js = include_str!("../../dist/app.js");
        let mut codes = refusal_codes();
        codes.extend(ALL_FAILURES.iter().map(|f| failure_code(*f)));
        codes.extend([Note::OtherDrive, Note::CloudUnchecked].map(note_code));
        codes.extend([
            "nothing",
            "moved",
            "old_copy_removed",
            "old_copy_waiting",
            "failed",
            "deferred",
        ]);
        for code in codes {
            assert!(
                app_js.contains(&format!("case \"{code}\":")),
                "{code} has no words in the desktop bundle"
            );
        }
    }

    /// The move restarts the app only once it has been recorded: a refused
    /// move must leave the person where they were, with the reason.
    #[test]
    fn the_app_restarts_only_after_the_move_is_recorded() {
        let source = include_str!("location.rs").replace("\r\n", "\n");
        let body = source
            .split_once("pub async fn location_move(")
            .expect("the move command is defined here")
            .1
            .split_once("\n}\n")
            .expect("the move command is closed")
            .0;
        let asked = body.find("request_move(").expect("the move is asked for");
        let refused = body
            .find(".map_err(|r| refusal_code(r).to_string())?")
            .expect("a refusal returns before the restart");
        let restart = body.find("request_restart()").expect("the app restarts");
        assert!(asked < refused && refused < restart);
        assert_eq!(body.matches("request_restart").count(), 1);
    }
}
