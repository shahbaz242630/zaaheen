//! The OS-agnostic scheduling contract: what to run, and when.
//!
//! These types are the input every backend consumes. They also carry the
//! **injection-safety gate** (ADR-SEC-005): [`TaskId::new`] and
//! [`ScheduleSpec::validate`] reject anything that could break out of a field
//! when a backend serialises the spec into a `schtasks` command, a launchd
//! plist, or a systemd/cron line. Every backend MUST call
//! [`ScheduleSpec::validate`] before it builds any OS artefact — the
//! per-backend escaping is defence in depth on top of this gate, not a
//! replacement for it.

use std::path::PathBuf;

use chrono::{NaiveTime, Weekday};
use serde::{Deserialize, Serialize};

use crate::error::{SchedulerError, SchedulerResult};

/// Maximum length of a task id. Comfortably under every backend's own limit
/// (Windows task names, launchd Labels, systemd unit names) while leaving no
/// room for an abusive value.
const TASK_ID_MAX_LEN: usize = 200;

/// A stable, validated identifier for a scheduled task.
///
/// Constrained to `A-Z a-z 0-9 . _ -` so it can be interpolated into an OS
/// task name, a launchd `Label`, or a systemd unit name with **no escaping
/// required** — the charset excludes every path separator, shell
/// metacharacter, whitespace, and quote. This is the first line of the
/// injection-safety gate: a value that reaches a backend is already known to
/// be inert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskId(String);

impl TaskId {
    /// Construct a validated task id, or reject it.
    ///
    /// Rejects: empty, over [`TASK_ID_MAX_LEN`], or any character outside
    /// `A-Z a-z 0-9 . _ -`. That excludes `/ \ : ; & | $ ` " ' < > * ?`,
    /// whitespace, and control characters — so path traversal (`..` segments
    /// still pass the charset but carry no separator to act on) and command
    /// injection are both structurally impossible downstream.
    pub fn new(raw: impl Into<String>) -> SchedulerResult<Self> {
        let s = raw.into();
        if s.is_empty() {
            return Err(SchedulerError::InvalidSpec(
                "task id must not be empty".into(),
            ));
        }
        if s.len() > TASK_ID_MAX_LEN {
            return Err(SchedulerError::InvalidSpec(format!(
                "task id too long ({} > {})",
                s.len(),
                TASK_ID_MAX_LEN
            )));
        }
        if let Some(bad) = s
            .chars()
            .find(|&c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
        {
            return Err(SchedulerError::InvalidSpec(format!(
                "task id contains an illegal character {bad:?} (allowed: A-Z a-z 0-9 . _ -)"
            )));
        }
        Ok(Self(s))
    }

    /// The validated identifier as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// How often the task runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Frequency {
    /// Every day at the spec's `time_of_day`.
    Daily,
    /// Once a week, on `day`, at the spec's `time_of_day`.
    Weekly {
        /// Day of week the weekly run fires on.
        day: Weekday,
    },
}

/// A complete description of a scheduled task: what command to run, and when.
///
/// Constructed server-side (the Tauri command layer) — the fields are public
/// so a caller builds one directly, then hands it to a [`crate::Scheduler`].
/// The `program`/`args`/`env` triple is passed to the OS as an argument
/// vector (never a shell string) by every backend; the validation below plus
/// per-backend escaping keep that safe even for paths containing spaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleSpec {
    /// Stable identifier, unique per logical task on the machine.
    pub task_id: TaskId,
    /// Human-facing description the OS may show (e.g. Task Scheduler's
    /// Description field). White-label copy — no stack names (ADR-086).
    pub label: String,
    /// How often to run.
    pub frequency: Frequency,
    /// Local time-of-day to run at (minute precision is all any backend uses).
    pub time_of_day: NaiveTime,
    /// The executable to run — the bundled `zaaheen` CLI (ADR-093). An absolute
    /// path; may contain spaces (`C:\Program Files\...`).
    pub program: PathBuf,
    /// Arguments passed to `program`, one element per argv slot (not a shell
    /// string). E.g. `["consolidate", "run", "--phi4-model", "<path>"]`.
    pub args: Vec<String>,
    /// Environment variables set for the scheduled run (e.g.
    /// `LANCE_MEM_POOL_SIZE`). Never secrets — the master key is read from the
    /// OS keychain at run time (ADR-SEC-005).
    pub env: Vec<(String, String)>,
}

impl ScheduleSpec {
    /// Validate every field that feeds an OS-level serialisation.
    ///
    /// The injection-safety gate (ADR-SEC-005). Rejects an empty program
    /// path, and any control character in the label, an argument, or an
    /// environment value — a control character (newline, carriage return,
    /// NUL, tab, …) is the vector for breaking out of a launchd plist string,
    /// a crontab line, or a systemd unit line. Environment-variable *names*
    /// are additionally constrained to `A-Z a-z 0-9 _` (which also rejects an
    /// embedded `=`). Spaces and ordinary punctuation in *paths* are allowed —
    /// those are legitimate and handled by per-backend quoting.
    pub fn validate(&self) -> SchedulerResult<()> {
        if self.program.as_os_str().is_empty() {
            return Err(SchedulerError::InvalidSpec(
                "program path must not be empty".into(),
            ));
        }
        reject_control_chars("label", &self.label)?;
        for (i, arg) in self.args.iter().enumerate() {
            reject_control_chars(&format!("arg[{i}]"), arg)?;
        }
        for (key, value) in &self.env {
            validate_env_key(key)?;
            reject_control_chars(&format!("env[{key}]"), value)?;
        }
        Ok(())
    }
}

/// A per-user task with NO schedule, started only on request (ADR-102: the
/// vault keeper, started when the first AI app needs it).
///
/// Same injection-safety gate as [`ScheduleSpec`]: validated before any OS
/// call, `program`/`args` passed as an argument vector, never a shell string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnDemandTask {
    /// Stable identifier. Carries the user's SID for the keeper so two
    /// Windows users on one PC never share (or overwrite) a task.
    pub task_id: TaskId,
    /// Description the OS shows. Also the version marker: an existing task
    /// whose description differs is replaced.
    pub label: String,
    /// Absolute path of the program to start.
    pub program: PathBuf,
    /// Arguments, one per argv slot.
    pub args: Vec<String>,
}

impl OnDemandTask {
    /// The same gate as [`ScheduleSpec::validate`].
    pub fn validate(&self) -> SchedulerResult<()> {
        if self.program.as_os_str().is_empty() {
            return Err(SchedulerError::InvalidSpec(
                "program path must not be empty".into(),
            ));
        }
        reject_control_chars("label", &self.label)?;
        reject_control_chars("program", &self.program.to_string_lossy())?;
        for (i, arg) in self.args.iter().enumerate() {
            reject_control_chars(&format!("arg[{i}]"), arg)?;
        }
        Ok(())
    }
}

/// Reject any control character in `value`, naming `field` in the error.
fn reject_control_chars(field: &str, value: &str) -> SchedulerResult<()> {
    if let Some(c) = value.chars().find(|c| c.is_control()) {
        return Err(SchedulerError::InvalidSpec(format!(
            "{field} contains a control character (U+{:04X}); \
             refusing to build an OS task from it",
            c as u32
        )));
    }
    Ok(())
}

/// Validate an environment-variable name: non-empty and `A-Z a-z 0-9 _` only
/// (which also rejects an embedded `=`, the key/value separator).
fn validate_env_key(key: &str) -> SchedulerResult<()> {
    if key.is_empty() {
        return Err(SchedulerError::InvalidSpec(
            "environment variable name must not be empty".into(),
        ));
    }
    if let Some(bad) = key
        .chars()
        .find(|&c| !(c.is_ascii_alphanumeric() || c == '_'))
    {
        return Err(SchedulerError::InvalidSpec(format!(
            "environment variable name {key:?} contains an illegal character {bad:?} \
             (allowed: A-Z a-z 0-9 _)"
        )));
    }
    Ok(())
}

/// The OS's current view of a scheduled task.
///
/// Deliberately minimal: whether the task is registered, plus an optional
/// best-effort human-readable detail the backend may fill in (e.g. the next
/// run time the OS reports). Rich last-run/next-run history is tracked by the
/// application layer from its own persisted state, not scraped from the OS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleStatus {
    /// Whether the OS currently has this task registered.
    pub registered: bool,
    /// Optional backend-specific detail; `None` when unavailable.
    pub detail: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn realistic_spec() -> ScheduleSpec {
        ScheduleSpec {
            task_id: TaskId::new("com.zaaheen.maintenance").unwrap(),
            label: "Zaaheen automatic maintenance".into(),
            frequency: Frequency::Daily,
            time_of_day: NaiveTime::from_hms_opt(3, 0, 0).unwrap(),
            // A path with a space — the common Windows install location — must
            // pass validation; quoting is the backend's job.
            program: PathBuf::from(r"C:\Program Files\Zaaheen\zaaheen.exe"),
            args: vec![
                "consolidate".into(),
                "run".into(),
                "--phi4-model".into(),
                r"C:\Users\sam\AppData\Roaming\Zaaheen App\models\phi4.gguf".into(),
            ],
            env: vec![("LANCE_MEM_POOL_SIZE".into(), "268435456".into())],
        }
    }

    #[test]
    fn task_id_accepts_the_canonical_identifier() {
        assert_eq!(
            TaskId::new("com.zaaheen.maintenance").unwrap().as_str(),
            "com.zaaheen.maintenance"
        );
        assert!(TaskId::new("vault-maintenance_v2").is_ok());
    }

    #[test]
    fn task_id_rejects_shell_and_path_metacharacters() {
        // Every one of these could break out of a task name / plist Label /
        // unit name or a shell command line if it survived to a backend.
        for bad in [
            "a b",  // whitespace
            "a;b",  // command separator
            "a&b",  // background / chain
            "a|b",  // pipe
            "a/b",  // path separator
            r"a\b", // windows path separator
            "a$b",  // shell expansion
            "a`b",  // command substitution
            "a\"b", // double quote
            "a'b",  // single quote
            "a>b",  // redirect
            "a<b",  // redirect
            "a*b",  // glob
            "a\nb", // newline injection
            "a\tb", // tab
            "a\0b", // NUL
            "a=b",  // key/value separator
            "a:b",  // drive / launchctl domain separator
            "a(b",  // paren
            "café", // non-ASCII
        ] {
            assert!(
                TaskId::new(bad).is_err(),
                "task id {bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn task_id_rejects_empty_and_overlong() {
        assert!(TaskId::new("").is_err());
        assert!(TaskId::new("a".repeat(TASK_ID_MAX_LEN)).is_ok());
        assert!(TaskId::new("a".repeat(TASK_ID_MAX_LEN + 1)).is_err());
    }

    #[test]
    fn realistic_maintenance_spec_validates() {
        // The real spec — including a program path and an argument path that
        // both contain spaces — must pass.
        realistic_spec().validate().unwrap();
    }

    #[test]
    fn validate_rejects_newline_in_an_argument() {
        // A newline in an argument is the crontab/unit/plist line-break
        // injection vector — the single most important thing this gate stops.
        let mut spec = realistic_spec();
        spec.args
            .push("--phi4-model\nDELETE FROM everything".into());
        assert!(matches!(
            spec.validate(),
            Err(SchedulerError::InvalidSpec(_))
        ));
    }

    #[test]
    fn validate_rejects_nul_and_carriage_return_in_an_argument() {
        for injected in ["path\0extra", "path\rmore"] {
            let mut spec = realistic_spec();
            spec.args.push(injected.into());
            assert!(
                spec.validate().is_err(),
                "argument {injected:?} must be rejected"
            );
        }
    }

    #[test]
    fn validate_rejects_control_char_in_the_label() {
        let mut spec = realistic_spec();
        spec.label = "line one\nline two".into();
        assert!(spec.validate().is_err());
    }

    #[test]
    fn validate_rejects_bad_env_key_but_allows_a_normal_one() {
        // '=' in a name would corrupt the key/value framing.
        let mut spec = realistic_spec();
        spec.env.push(("BAD=NAME".into(), "x".into()));
        assert!(spec.validate().is_err());

        // A control char in a value is also rejected.
        let mut spec = realistic_spec();
        spec.env.push(("GOOD_NAME".into(), "val\nue".into()));
        assert!(spec.validate().is_err());

        // An empty name is rejected.
        let mut spec = realistic_spec();
        spec.env.push((String::new(), "x".into()));
        assert!(spec.validate().is_err());
    }

    #[test]
    fn validate_rejects_empty_program() {
        let mut spec = realistic_spec();
        spec.program = PathBuf::new();
        assert!(spec.validate().is_err());
    }

    #[test]
    fn frequency_round_trips_through_serde() {
        // The Tauri IPC boundary hands Frequency back and forth; confirm both
        // variants survive a JSON round trip with the internal tag.
        let daily = serde_json::to_string(&Frequency::Daily).unwrap();
        assert_eq!(
            serde_json::from_str::<Frequency>(&daily).unwrap(),
            Frequency::Daily
        );
        let weekly = Frequency::Weekly { day: Weekday::Sun };
        let json = serde_json::to_string(&weekly).unwrap();
        assert_eq!(serde_json::from_str::<Frequency>(&json).unwrap(), weekly);
    }
}
