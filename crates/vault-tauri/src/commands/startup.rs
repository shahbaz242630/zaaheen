//! Where this start is (ADR-105 amendment 1, L-f, `VAULT-KEY-AND-LOCATION.md`).
//!
//! A start with a move waiting serves the page before it opens the memories:
//! `setup()` returns so the window can draw, the move runs on its own thread,
//! and the rest of the start follows it. Until [`startup_state`] answers
//! `ready`, the page asks nothing else and shows "Moving your memories".
//!
//! **Open before the lock** (ADR-SEC-030 amendment 1, founder-approved): the
//! guard is built after the move, so this command cannot be gated. It is
//! given [`Startup`] and nothing else, no vault, no account, no key, and
//! answers only the stage, the move's phase, two byte counts and the folder
//! being moved to. It takes no argument.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::json;
use tauri::State;
use vault_app::location::display_path;
use vault_app::location::moving::{MovePhase, MoveProgress};

/// Where a start is.
#[derive(Debug)]
enum Stage {
    /// A move is running; the memories are going to `to`.
    Moving {
        progress: Arc<MoveProgress>,
        to: PathBuf,
    },
    /// The move is over; the memories, the key and the lock are opening.
    Opening,
    /// Every piece of state the other commands need is managed.
    Ready,
}

/// Where this start is, for [`startup_state`]. One copy is managed for the
/// command; the thread making the move keeps another, sharing the stage.
#[derive(Clone, Debug)]
pub struct Startup {
    stage: Arc<Mutex<Stage>>,
}

impl Startup {
    /// A start that opens the memories on the setup thread: the page cannot
    /// run until `setup()` returns, and by then this says ready.
    pub fn opening() -> Self {
        Self::at(Stage::Opening)
    }

    /// A start that moves the memories to `to` first, reporting to
    /// `progress`.
    pub fn moving(progress: Arc<MoveProgress>, to: PathBuf) -> Self {
        Self::at(Stage::Moving { progress, to })
    }

    /// The move is over; the rest of the start is running.
    pub fn move_finished(&self) {
        *self.lock() = Stage::Opening;
    }

    /// Every piece of state is managed: the page may go on. Called last.
    pub fn ready(&self) {
        *self.lock() = Stage::Ready;
    }

    /// The answer the page gets. Explicit fields only (BRD §11.7.2).
    pub fn answer(&self) -> serde_json::Value {
        match &*self.lock() {
            Stage::Ready => json!({ "stage": "ready" }),
            Stage::Opening => json!({ "stage": "opening" }),
            Stage::Moving { progress, to } => {
                let now = progress.snapshot();
                json!({
                    "stage": "moving",
                    "phase": phase_wire(now.phase),
                    "done": now.done,
                    "total": now.total,
                    "to": display_path(to),
                })
            }
        }
    }

    fn at(stage: Stage) -> Self {
        Self {
            stage: Arc::new(Mutex::new(stage)),
        }
    }

    /// A stage is one value: a poisoned lock still holds a whole one.
    fn lock(&self) -> MutexGuard<'_, Stage> {
        self.stage.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn phase_wire(phase: MovePhase) -> &'static str {
    match phase {
        MovePhase::Waiting => "waiting",
        MovePhase::Copying => "copying",
        MovePhase::Checking => "checking",
        MovePhase::Finishing => "finishing",
    }
}

/// Where this start is: `ready`, `opening`, or `moving` with the move's
/// phase, bytes done of the total, and the folder. Open before the lock
/// (module doc).
#[tauri::command]
pub async fn startup_state(startup: State<'_, Startup>) -> Result<serde_json::Value, String> {
    Ok(startup.answer())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_start_says_where_it_is() {
        let progress = Arc::new(MoveProgress::default());
        let startup = Startup::moving(
            Arc::clone(&progress),
            PathBuf::from(r"\\?\E:\Backups\Zaaheen Memories"),
        );
        assert_eq!(
            startup.answer(),
            json!({ "stage": "moving", "phase": "waiting", "done": 0, "total": 0,
                    "to": r"E:\Backups\Zaaheen Memories" }),
            "the folder as the person reads it"
        );
        // The thread's copy moves the stage the command's copy reads.
        let on_the_thread = startup.clone();
        on_the_thread.move_finished();
        assert_eq!(startup.answer(), json!({ "stage": "opening" }));
        on_the_thread.ready();
        assert_eq!(startup.answer(), json!({ "stage": "ready" }));
        assert_eq!(Startup::opening().answer(), json!({ "stage": "opening" }));
    }

    #[test]
    fn every_phase_has_its_own_word() {
        let words: Vec<&str> = [
            MovePhase::Waiting,
            MovePhase::Copying,
            MovePhase::Checking,
            MovePhase::Finishing,
        ]
        .into_iter()
        .map(phase_wire)
        .collect();
        assert_eq!(words, ["waiting", "copying", "checking", "finishing"]);
        // The page's words for each (dist/app.js renderMoving).
        let app_js = include_str!("../../dist/app.js");
        for word in [
            "\"copying\"",
            "\"checking\"",
            "\"finishing\"",
            "\"opening\"",
        ] {
            assert!(app_js.contains(word), "the page has no words for {word}");
        }
    }

    /// Open before the lock, so it must not be able to reach a memory, an
    /// account or the key (ADR-SEC-030 amendment 1).
    #[test]
    fn the_startup_command_is_given_nothing_but_the_start() {
        let code: String = include_str!("startup.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for forbidden in [
            "Application",
            "Adapter",
            "adapter()",
            "MasterKey",
            "Entitlement",
            "AccountSlot",
            "keychain",
        ] {
            assert!(
                !code.contains(forbidden),
                "commands/startup.rs names `{forbidden}`: it is open before the lock and must \
                 reach nothing but the start's own progress"
            );
        }
        let command = code
            .split_once("pub async fn startup_state(")
            .expect("the command is defined here")
            .1
            .split_once(')')
            .expect("its arguments close")
            .0;
        assert_eq!(
            command.trim(),
            "startup: State<'_, Startup>",
            "it takes no argument from the page"
        );
    }
}
