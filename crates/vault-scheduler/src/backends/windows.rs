//! Windows backend — registers a per-user task with Task Scheduler.
//!
//! We register via a **Task Scheduler XML definition** (`schtasks /Create
//! /XML`) rather than a `/TR` command string. The XML separates the executable
//! (`<Command>`) from its arguments (`<Arguments>`) into distinct fields, so
//! there is no single command string for a malicious argument to break out of;
//! it also lets us set the two things a `/TR` string cannot express:
//!
//! - `RunLevel=LeastPrivilege` + `LogonType=InteractiveToken` — the task runs
//!   as the current interactive user with **no elevation**, so registering it
//!   never triggers a UAC prompt.
//! - `StartWhenAvailable=true` — Windows' own missed-run catch-up: if the
//!   machine was asleep or off at the scheduled time, the task runs at the next
//!   opportunity instead of being skipped.
//!
//! The pure XML/argument builder ([`build_task_xml`] and its helpers) is
//! compiled and unit-tested on every platform; only [`WindowsScheduler`],
//! which shells out to `schtasks.exe`, is `#[cfg(windows)]`.
//!
//! **Environment variables:** Task Scheduler has no per-task environment
//! mechanism, so a Windows task inherits the launching user's environment.
//! `ScheduleSpec::env` is therefore NOT injected here — the one variable the
//! maintenance run needs (`LANCE_MEM_POOL_SIZE`) is provisioned as a per-user
//! environment variable by the installer (ADR-091), which the scheduled
//! `zaaheen` CLI inherits. If a future spec needs a variable that is not in the
//! user environment, that is a distinct piece of work (a launcher shim), called
//! out rather than silently dropped.

use super::xml_escape;
use crate::spec::{Frequency, ScheduleSpec};

/// The Task Scheduler 2.0 XML namespace.
const TASK_XML_NS: &str = "http://schemas.microsoft.com/windows/2004/02/mit/task";

/// Build the Task Scheduler XML definition for `spec`.
///
/// Deterministic (no wall-clock read): the trigger's `StartBoundary` uses a
/// fixed base date with the spec's time-of-day, and a daily/weekly recurrence
/// carries the actual schedule — Task Scheduler computes the next occurrence
/// from the recurrence, not from the base date. Determinism keeps the builder
/// unit-testable by exact string comparison.
pub(crate) fn build_task_xml(spec: &ScheduleSpec) -> String {
    let start_boundary = format!("2000-01-01T{}", spec.time_of_day.format("%H:%M:%S"));
    let schedule = match spec.frequency {
        Frequency::Daily => "      <ScheduleByDay>\n\
             \x20       <DaysInterval>1</DaysInterval>\n\
             \x20     </ScheduleByDay>"
            .to_string(),
        Frequency::Weekly { day } => format!(
            "      <ScheduleByWeek>\n\
             \x20       <WeeksInterval>1</WeeksInterval>\n\
             \x20       <DaysOfWeek><{day}/></DaysOfWeek>\n\
             \x20     </ScheduleByWeek>",
            day = weekday_element(day)
        ),
    };

    let command = xml_escape(&spec.program.to_string_lossy());
    let arguments = xml_escape(&join_arguments(&spec.args));
    let description = xml_escape(&spec.label);

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\n\
         <Task version=\"1.2\" xmlns=\"{ns}\">\n\
         \x20 <RegistrationInfo>\n\
         \x20   <Description>{description}</Description>\n\
         \x20 </RegistrationInfo>\n\
         \x20 <Triggers>\n\
         \x20   <CalendarTrigger>\n\
         \x20     <StartBoundary>{start_boundary}</StartBoundary>\n\
         \x20     <Enabled>true</Enabled>\n\
         {schedule}\n\
         \x20   </CalendarTrigger>\n\
         \x20 </Triggers>\n\
         \x20 <Principals>\n\
         \x20   <Principal id=\"Author\">\n\
         \x20     <LogonType>InteractiveToken</LogonType>\n\
         \x20     <RunLevel>LeastPrivilege</RunLevel>\n\
         \x20   </Principal>\n\
         \x20 </Principals>\n\
         \x20 <Settings>\n\
         \x20   <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>\n\
         \x20   <StartWhenAvailable>true</StartWhenAvailable>\n\
         \x20   <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>\n\
         \x20   <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>\n\
         \x20   <ExecutionTimeLimit>PT2H</ExecutionTimeLimit>\n\
         \x20   <Enabled>true</Enabled>\n\
         \x20 </Settings>\n\
         \x20 <Actions Context=\"Author\">\n\
         \x20   <Exec>\n\
         \x20     <Command>{command}</Command>\n\
         \x20     <Arguments>{arguments}</Arguments>\n\
         \x20   </Exec>\n\
         \x20 </Actions>\n\
         </Task>\n",
        ns = TASK_XML_NS,
    )
}

/// Build the XML for an on-demand task (ADR-102): no trigger at all, started
/// only by `schtasks /Run`.
///
/// Settings that differ from the nightly task, each for a reason:
/// - no `<Triggers>` — nothing starts it except an explicit request;
/// - `ExecutionTimeLimit` `PT0S` (unlimited) — the default of 72 hours would
///   kill a long-lived keeper after three days;
/// - `Priority` 6 — the default 7 is background CPU/IO priority, which would
///   make every memory read slow;
/// - `MultipleInstancesPolicy` `IgnoreNew` — a start request while one is
///   running is dropped, so requests can be repeated harmlessly.
///
/// `label` becomes the `Description`, which doubles as a version marker the
/// caller checks to decide whether an existing task must be replaced.
pub(crate) fn build_on_demand_task_xml(task: &crate::OnDemandTask) -> String {
    let command = xml_escape(&task.program.to_string_lossy());
    let arguments = xml_escape(&join_arguments(&task.args));
    let description = xml_escape(&task.label);

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\n\
         <Task version=\"1.2\" xmlns=\"{ns}\">\n\
         \x20 <RegistrationInfo>\n\
         \x20   <Description>{description}</Description>\n\
         \x20 </RegistrationInfo>\n\
         \x20 <Principals>\n\
         \x20   <Principal id=\"Author\">\n\
         \x20     <LogonType>InteractiveToken</LogonType>\n\
         \x20     <RunLevel>LeastPrivilege</RunLevel>\n\
         \x20   </Principal>\n\
         \x20 </Principals>\n\
         \x20 <Settings>\n\
         \x20   <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>\n\
         \x20   <AllowStartOnDemand>true</AllowStartOnDemand>\n\
         \x20   <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>\n\
         \x20   <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>\n\
         \x20   <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>\n\
         \x20   <Priority>6</Priority>\n\
         \x20   <Enabled>true</Enabled>\n\
         \x20 </Settings>\n\
         \x20 <Actions Context=\"Author\">\n\
         \x20   <Exec>\n\
         \x20     <Command>{command}</Command>\n\
         \x20     <Arguments>{arguments}</Arguments>\n\
         \x20   </Exec>\n\
         \x20 </Actions>\n\
         </Task>\n",
        ns = TASK_XML_NS,
    )
}

/// Decode `schtasks /Query /XML` output, which may arrive as UTF-16LE (with or
/// without a BOM) or as 8-bit text depending on how the output is redirected.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn decode_schtasks_output(bytes: &[u8]) -> String {
    let utf16 = bytes.starts_with(&[0xFF, 0xFE])
        || (bytes.len() >= 4 && bytes.iter().skip(1).step_by(2).take(32).all(|b| *b == 0));
    if utf16 {
        let body = bytes.strip_prefix(&[0xFF, 0xFE]).unwrap_or(bytes);
        let units: Vec<u16> = body
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// Whether a registered on-demand task (its `schtasks /Query /XML` output)
/// already has exactly this definition: same version label, program and
/// arguments. Compared on DECODED element text, because Task Scheduler
/// re-serialises the XML it stores — a quote we wrote as `&quot;` comes back
/// as a literal `"` — and a byte comparison would then call every task stale
/// and re-create it on every start request.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn on_demand_task_matches(registered_xml: &str, task: &crate::OnDemandTask) -> bool {
    element_text(registered_xml, "Description").as_deref() == Some(task.label.as_str())
        && element_text(registered_xml, "Command").as_deref()
            == Some(task.program.to_string_lossy().as_ref())
        // An absent element is an empty argument list.
        && element_text(registered_xml, "Arguments").unwrap_or_default()
            == join_arguments(&task.args)
}

/// The decoded text of the first `<name>...</name>` element, if any.
fn element_text(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = xml.find(&open)? + open.len();
    let len = xml[start..].find(&close)?;
    Some(xml_unescape(xml[start..start + len].trim()))
}

/// Undo XML's five predefined entities (`&amp;` last, so `&amp;quot;` stays
/// the literal text `&quot;`).
fn xml_unescape(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// Map a [`chrono::Weekday`] to its Task Scheduler `DaysOfWeek` element name.
fn weekday_element(day: chrono::Weekday) -> &'static str {
    use chrono::Weekday::*;
    match day {
        Mon => "Monday",
        Tue => "Tuesday",
        Wed => "Wednesday",
        Thu => "Thursday",
        Fri => "Friday",
        Sat => "Saturday",
        Sun => "Sunday",
    }
}

/// Join `args` into a single Windows command-line string, quoting each argument
/// per the `CommandLineToArgvW` rules so a path with spaces round-trips as ONE
/// argument. The result is XML-escaped by the caller before it enters the
/// `<Arguments>` element.
fn join_arguments(args: &[String]) -> String {
    args.iter()
        .map(|a| windows_quote_arg(a))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Quote a single argument per the `CommandLineToArgvW` algorithm (see
/// Microsoft's "Everyone quotes command line arguments the wrong way").
///
/// An argument with no space, tab, or double-quote needs no quoting (bare
/// backslashes are literal there). Otherwise it is wrapped in double quotes,
/// with backslash runs that precede a quote (or the closing quote) doubled, and
/// embedded quotes escaped.
fn windows_quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_string();
    }
    let chars: Vec<char> = arg.chars().collect();
    let mut out = String::from('"');
    let mut i = 0;
    while i < chars.len() {
        let mut backslashes = 0;
        while i < chars.len() && chars[i] == '\\' {
            backslashes += 1;
            i += 1;
        }
        if i == chars.len() {
            // Trailing backslashes precede the closing quote: double them.
            out.push_str(&"\\".repeat(backslashes * 2));
        } else if chars[i] == '"' {
            // Backslashes before a quote are doubled; the quote is escaped.
            out.push_str(&"\\".repeat(backslashes * 2 + 1));
            out.push('"');
            i += 1;
        } else {
            out.push_str(&"\\".repeat(backslashes));
            out.push(chars[i]);
            i += 1;
        }
    }
    out.push('"');
    out
}

#[cfg(windows)]
pub(crate) use imp::{start_on_demand, WindowsScheduler};

#[cfg(windows)]
mod imp {
    use std::io::Write;
    // `creation_flags` lives on this extension trait; it is the only way to
    // pass CREATE_NO_WINDOW without an external bindings crate.
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    use super::{
        build_on_demand_task_xml, build_task_xml, decode_schtasks_output, on_demand_task_matches,
    };
    use crate::error::{SchedulerError, SchedulerResult};
    use crate::spec::{OnDemandTask, ScheduleSpec, ScheduleStatus, TaskId};
    use crate::Scheduler;

    /// `CREATE_NO_WINDOW` — run a console child with no console window.
    ///
    /// From the Win32 process-creation flags. Declared here rather than pulled
    /// from a binding crate: it is one stable constant, and the alternative is
    /// a new dependency for a single `u32`.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// Build a `schtasks.exe` command that shows no console window
    /// (ADR-SEC-015 amendment 1).
    ///
    /// # Why every call site must go through this
    ///
    /// `schtasks.exe` is a console program. The desktop app is linked for the
    /// Windows subsystem and therefore has no console of its own, so Windows
    /// creates a **new console window** for any console child it spawns.
    ///
    /// That made a black window flash on the user's screen every time the
    /// Maintenance tab was opened (`get_maintenance_schedule` calls
    /// [`Scheduler::status`], which queries `schtasks`) and every time the
    /// schedule was changed. Same defect as the scheduled run itself, in the
    /// crate whose whole job is talking to the OS scheduler.
    ///
    /// Found while adding the startup re-registration in ADR-SEC-015
    /// amendment 1 — which would otherwise have flashed a console on **every**
    /// app launch, turning an occasional annoyance into a permanent one.
    fn schtasks_command() -> Command {
        let mut command = Command::new("schtasks");
        command.creation_flags(CREATE_NO_WINDOW);
        command
    }

    /// Task Scheduler backend driving `schtasks.exe`.
    pub(crate) struct WindowsScheduler;

    impl Scheduler for WindowsScheduler {
        fn register(&self, spec: &ScheduleSpec) -> SchedulerResult<()> {
            // Injection-safety gate first — never build an artefact from an
            // unvalidated spec.
            spec.validate()?;

            let xml = build_task_xml(spec);
            let xml_path = write_task_xml_utf16(spec.task_id.as_str(), &xml)?;

            // /F overwrites an existing task of the same name — register is a
            // "make it so" operation, matching the trait contract.
            let result = run_schtasks(&[
                "/Create",
                "/TN",
                spec.task_id.as_str(),
                "/XML",
                &xml_path.to_string_lossy(),
                "/F",
            ]);

            // Best-effort cleanup of the temp XML regardless of outcome; it can
            // carry a program path but never a secret.
            let _ = std::fs::remove_file(&xml_path);
            result.map(|_| ())
        }

        fn unregister(&self, task_id: &TaskId) -> SchedulerResult<()> {
            // Idempotent: if the task is not present, deletion is a no-op
            // success. Query first so a genuine delete failure stays an error.
            if !self.status(task_id)?.registered {
                return Ok(());
            }
            run_schtasks(&["/Delete", "/TN", task_id.as_str(), "/F"]).map(|_| ())
        }

        fn status(&self, task_id: &TaskId) -> SchedulerResult<ScheduleStatus> {
            // `schtasks /Query` exits non-zero when the task does not exist; we
            // read that as "not registered" rather than a backend failure.
            match schtasks_command()
                .args(["/Query", "/TN", task_id.as_str()])
                .output()
            {
                Ok(output) if output.status.success() => Ok(ScheduleStatus {
                    registered: true,
                    detail: None,
                }),
                Ok(_) => Ok(ScheduleStatus {
                    registered: false,
                    detail: None,
                }),
                Err(err) => Err(SchedulerError::Io(err)),
            }
        }
    }

    /// Make sure the on-demand task exists with this definition, then start
    /// it. See [`crate::backends::start_on_demand`]; `task` is validated there.
    pub(crate) fn start_on_demand(task: &OnDemandTask) -> SchedulerResult<()> {
        let id = task.task_id.as_str();
        let existing = schtasks_command()
            .args(["/Query", "/TN", id, "/XML"])
            .output()
            .map_err(SchedulerError::Io)?;
        // Arguments are compared too: the log directory in them can change
        // while the label and program stay the same.
        let up_to_date = existing.status.success()
            && on_demand_task_matches(&decode_schtasks_output(&existing.stdout), task);

        let mut create_error = None;
        if !up_to_date {
            let xml = build_on_demand_task_xml(task);
            // A per-process file: several relays start at once, and a shared
            // name would let one truncate another's definition mid-read.
            let file_name = format!("vault-scheduler-{id}-{}.xml", std::process::id());
            let path = write_task_xml_utf16_named(&file_name, &xml)?;
            let path_str = path.to_string_lossy().into_owned();
            let mut args = vec!["/Create", "/TN", id, "/XML", path_str.as_str()];
            if existing.status.success() {
                // A stale definition (an older build, a moved install) is
                // replaced rather than left pointing at the wrong program.
                args.push("/F");
            }
            if let Err(e) = run_schtasks(&args) {
                // Another relay may have created it a moment ago; the start
                // below is the real test.
                create_error = Some(e);
            }
            let _ = std::fs::remove_file(&path);
        }

        match run_schtasks(&["/Run", "/TN", id]) {
            Ok(_) => Ok(()),
            Err(run_error) => Err(create_error.unwrap_or(run_error)),
        }
    }

    /// Run `schtasks.exe` with `args`, returning its stdout on success or a
    /// [`SchedulerError::BackendFailed`] carrying stderr on a non-zero exit.
    fn run_schtasks(args: &[&str]) -> SchedulerResult<String> {
        let output = schtasks_command()
            .args(args)
            .output()
            .map_err(SchedulerError::Io)?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            Err(SchedulerError::BackendFailed(format!(
                "schtasks {args:?} exited with {}: {}",
                output.status,
                // schtasks writes its ERROR: lines to stdout on some hosts,
                // stderr on others — include whichever is non-empty.
                if stderr.trim().is_empty() {
                    stdout.trim()
                } else {
                    stderr.trim()
                }
            )))
        }
    }

    /// Write `xml` to a temp file as UTF-16LE with a BOM — the encoding
    /// `schtasks /XML` accepts most reliably across Windows versions. Returns
    /// the path; the caller removes it after registration.
    fn write_task_xml_utf16(task_id: &str, xml: &str) -> SchedulerResult<std::path::PathBuf> {
        // task_id is charset-validated (A-Z a-z 0-9 . _ -), so it is a safe
        // filename component with no separators.
        write_task_xml_utf16_named(&format!("vault-scheduler-{task_id}.xml"), xml)
    }

    /// As [`write_task_xml_utf16`], under a caller-chosen temp file name built
    /// only from validated components.
    fn write_task_xml_utf16_named(
        file_name: &str,
        xml: &str,
    ) -> SchedulerResult<std::path::PathBuf> {
        let mut path = std::env::temp_dir();
        path.push(file_name);

        let mut bytes = Vec::with_capacity(xml.len() * 2 + 2);
        bytes.extend_from_slice(&[0xFF, 0xFE]); // UTF-16LE BOM
        for unit in xml.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }

        let mut file = std::fs::File::create(&path).map_err(SchedulerError::Io)?;
        file.write_all(&bytes).map_err(SchedulerError::Io)?;
        Ok(path)
    }

    #[cfg(test)]
    mod tests {
        use super::CREATE_NO_WINDOW;

        /// Pin the magic number itself, by reading the constant rather than
        /// scanning for its text.
        ///
        /// A wrong value compiles, runs, and silently restores the console
        /// window this whole change exists to remove — there is no failure
        /// anyone would notice except a user seeing a black rectangle.
        #[test]
        fn the_no_window_flag_is_the_documented_win32_constant() {
            assert_eq!(CREATE_NO_WINDOW, 0x0800_0000);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::TaskId;
    use chrono::{NaiveTime, Weekday};
    use std::path::PathBuf;

    fn daily_spec() -> ScheduleSpec {
        ScheduleSpec {
            task_id: TaskId::new("com.zaaheen.maintenance").unwrap(),
            label: "Zaaheen automatic maintenance".into(),
            frequency: Frequency::Daily,
            time_of_day: NaiveTime::from_hms_opt(3, 0, 0).unwrap(),
            program: PathBuf::from(r"C:\Program Files\Zaaheen\zaaheen.exe"),
            args: vec![
                "consolidate".into(),
                "run".into(),
                "--phi4-model".into(),
                r"C:\Users\sam\AppData\Roaming\Zaaheen App\models\phi4.gguf".into(),
            ],
            env: vec![],
        }
    }

    #[test]
    fn daily_xml_has_daily_recurrence_and_the_scheduled_time() {
        let xml = build_task_xml(&daily_spec());
        assert!(xml.contains("<ScheduleByDay>"));
        assert!(xml.contains("<DaysInterval>1</DaysInterval>"));
        assert!(xml.contains("<StartBoundary>2000-01-01T03:00:00</StartBoundary>"));
        assert!(!xml.contains("<ScheduleByWeek>"));
    }

    #[test]
    fn weekly_xml_names_the_day() {
        let mut spec = daily_spec();
        spec.frequency = Frequency::Weekly { day: Weekday::Sun };
        let xml = build_task_xml(&spec);
        assert!(xml.contains("<ScheduleByWeek>"));
        assert!(xml.contains("<DaysOfWeek><Sunday/></DaysOfWeek>"));
        assert!(!xml.contains("<ScheduleByDay>"));
    }

    #[test]
    fn xml_runs_per_user_without_elevation_and_catches_up() {
        let xml = build_task_xml(&daily_spec());
        assert!(xml.contains("<LogonType>InteractiveToken</LogonType>"));
        assert!(xml.contains("<RunLevel>LeastPrivilege</RunLevel>"));
        // StartWhenAvailable is the OS-level missed-run catch-up.
        assert!(xml.contains("<StartWhenAvailable>true</StartWhenAvailable>"));
    }

    #[test]
    fn program_and_arguments_are_separate_fields() {
        let xml = build_task_xml(&daily_spec());
        assert!(xml.contains("<Command>C:\\Program Files\\Zaaheen\\zaaheen.exe</Command>"));
        // The phi4 path has a space, so it must be a single quoted argument.
        // Keep a space in this fixture: the quoting rule is the whole point of
        // the assertion, and a space-free path would let it pass vacuously.
        assert!(xml.contains(
            "&quot;C:\\Users\\sam\\AppData\\Roaming\\Zaaheen App\\models\\phi4.gguf&quot;"
        ));
        assert!(xml.contains("consolidate run --phi4-model"));
    }

    #[test]
    fn build_is_deterministic() {
        assert_eq!(build_task_xml(&daily_spec()), build_task_xml(&daily_spec()));
    }

    #[test]
    fn a_label_with_markup_cannot_break_out_of_the_element() {
        let mut spec = daily_spec();
        spec.label = "Bad</Description><script>".into();
        let xml = build_task_xml(&spec);
        assert!(!xml.contains("<script>"));
        assert!(xml.contains("&lt;script&gt;"));
    }

    #[test]
    fn arg_quoting_leaves_simple_args_bare_and_quotes_spaces() {
        assert_eq!(windows_quote_arg("consolidate"), "consolidate");
        assert_eq!(windows_quote_arg("--phi4-model"), "--phi4-model");
        assert_eq!(windows_quote_arg(r"C:\a b\c"), r#""C:\a b\c""#);
    }

    #[test]
    fn arg_quoting_escapes_embedded_quotes_and_trailing_backslashes() {
        // A quote inside the argument is backslash-escaped.
        assert_eq!(windows_quote_arg(r#"a"b"#), r#""a\"b""#);
        // Trailing backslashes before the closing quote are doubled so they do
        // not escape it.
        assert_eq!(windows_quote_arg(r"a b\"), r#""a b\\""#);
    }

    /// ADR-SEC-015 amendment 1: no `schtasks` call may spawn a visible console.
    ///
    /// A source scan rather than a behavioural test, because the defect is
    /// invisible to any assertion we can make about a `Command` — the window
    /// appears at spawn time, on a real desktop, and nothing observable comes
    /// back. The only reliable guard is that every call site routes through
    /// the one builder that sets the flag.
    ///
    /// This is the same shape as `prompt_guard_coverage.rs`: scan our own
    /// source and fail the build if the pattern is bypassed.
    /// Drop `//` comments so prose about the pattern is not mistaken for the
    /// pattern.
    ///
    /// The first version of the guard below failed against its own explanatory
    /// comment — exactly what `prompt_guard_coverage.rs` hit in session 32,
    /// where a comment quoting the guarded construct tripped the guard. The
    /// lesson recorded there was to make the matcher comment-aware rather than
    /// excuse the file, because excusing it is how a guard quietly stops
    /// guarding.
    ///
    /// Deliberately naive: it also truncates at a `//` inside a string literal
    /// (this file has one, in the XML namespace URL). That is harmless here —
    /// the discarded remainder never contains the scanned construct — and the
    /// alternative is a Rust lexer in a test.
    fn strip_line_comments(src: &str) -> String {
        src.lines()
            .map(|line| match line.find("//") {
                Some(i) => &line[..i],
                None => line,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The body of `schtasks_command`, so an assertion about the builder cannot
    /// be satisfied by this test file's own text.
    fn builder_body(code: &str) -> &str {
        let start = code
            .find("fn schtasks_command()")
            .expect("the builder must exist");
        let rest = &code[start..];
        let end = rest.find("\n    }").expect("the builder must close");
        &rest[..end]
    }

    #[test]
    fn every_schtasks_invocation_suppresses_its_console_window() {
        let code = strip_line_comments(include_str!("windows.rs"));

        let direct = code.matches("Command::new(\"schtasks\")").count();
        assert_eq!(
            direct, 1,
            "expected exactly ONE schtasks command construction (the one inside \
             `schtasks_command()`), found {direct}.\n\
             More than one: a call site bypassed the builder, inherits no \
             creation flags, and Windows gives that child a console window -- \
             the defect ADR-SEC-015 exists to remove, reappearing in the crate \
             that talks to the scheduler.\n\
             Zero: the scanned construct no longer matches the code, so this \
             guard has gone vacuous and must be re-pointed at whatever \
             replaced it."
        );

        // Scoped to the builder's own body. Asserting against the whole file
        // would be satisfied by the string literal on this very line.
        assert!(
            builder_body(&code).contains("creation_flags(CREATE_NO_WINDOW)"),
            "`schtasks_command()` no longer sets CREATE_NO_WINDOW, so every \
             schtasks call is back to showing a console window"
        );
    }

    #[test]
    fn the_comment_stripper_does_not_confuse_prose_for_code() {
        // Guards the guard. If stripping broke, the count assertion above
        // would start failing against explanatory comments -- and the tempting
        // fix would be to delete the comments rather than fix the matcher.
        let sample = "let a = Command::new(\"schtasks\");\n// see Command::new(\"schtasks\")\n";
        assert_eq!(
            strip_line_comments(sample)
                .matches("Command::new(\"schtasks\")")
                .count(),
            1,
            "the commented mention must not be counted"
        );
    }

    // ------------------------------------------------------------------
    //   On-demand keeper task (ADR-102)
    // ------------------------------------------------------------------

    fn keeper_task() -> crate::OnDemandTask {
        crate::OnDemandTask {
            task_id: TaskId::new("com.zaaheen.keeper.S-1-5-21-1111-2222-3333-1001").unwrap(),
            label: "zaaheen-keeper-task-v1".into(),
            program: PathBuf::from(r"C:\Program Files\Zaaheen\zaaheen-maintenance.exe"),
            args: vec![
                "keeper".into(),
                "--log-dir".into(),
                r"C:\Users\sam\AppData\Roaming\com.zaaheen.app\logs".into(),
            ],
        }
    }

    #[test]
    fn the_keeper_task_has_no_trigger_and_starts_only_on_request() {
        let xml = build_on_demand_task_xml(&keeper_task());
        assert!(
            !xml.contains("<Triggers>"),
            "an on-demand task must never start by itself"
        );
        assert!(xml.contains("<AllowStartOnDemand>true</AllowStartOnDemand>"));
    }

    #[test]
    fn the_keeper_is_never_killed_by_a_time_limit_and_runs_at_normal_priority() {
        let xml = build_on_demand_task_xml(&keeper_task());
        assert!(
            xml.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"),
            "the 72-hour default would kill the keeper after three days"
        );
        assert!(
            xml.contains("<Priority>6</Priority>"),
            "the default priority 7 is background CPU and IO, which makes reads slow"
        );
    }

    #[test]
    fn repeated_start_requests_are_harmless_and_batteries_do_not_block_it() {
        let xml = build_on_demand_task_xml(&keeper_task());
        assert!(xml.contains("<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>"));
        assert!(xml.contains("<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>"));
        assert!(xml.contains("<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>"));
    }

    #[test]
    fn the_keeper_task_runs_as_the_user_without_elevation() {
        let xml = build_on_demand_task_xml(&keeper_task());
        assert!(xml.contains("<LogonType>InteractiveToken</LogonType>"));
        assert!(xml.contains("<RunLevel>LeastPrivilege</RunLevel>"));
    }

    #[test]
    fn the_keeper_task_carries_its_version_marker_and_separate_arguments() {
        let xml = build_on_demand_task_xml(&keeper_task());
        assert!(xml.contains("<Description>zaaheen-keeper-task-v1</Description>"));
        assert!(
            xml.contains("<Command>C:\\Program Files\\Zaaheen\\zaaheen-maintenance.exe</Command>")
        );
        assert!(xml.contains("<Arguments>keeper --log-dir C:\\Users\\sam"));
    }

    #[test]
    fn schtasks_output_decodes_from_utf16_and_from_8_bit_text() {
        let text = "<Task><Description>zaaheen-keeper-task-v1</Description></Task>";
        let utf16: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut with_bom = vec![0xFF, 0xFE];
        with_bom.extend_from_slice(&utf16);
        assert_eq!(decode_schtasks_output(&with_bom), text);
        assert_eq!(decode_schtasks_output(&utf16), text);
        assert_eq!(decode_schtasks_output(text.as_bytes()), text);
    }

    /// The task we write reads back as up to date — and still does after
    /// Task Scheduler re-serialises it with literal quotes, as it does for a
    /// user name with a space. Otherwise every start request would re-create
    /// the task.
    #[test]
    fn a_registered_task_is_recognised_as_up_to_date_after_re_serialisation() {
        let mut task = keeper_task();
        task.args[2] = r"C:\Users\Sam Smith\AppData\Local\com.zaaheen.app\logs".into();
        let written = build_on_demand_task_xml(&task);
        assert!(on_demand_task_matches(&written, &task));

        let as_stored = written.replace("&quot;", "\"");
        assert!(
            as_stored.contains("--log-dir \"C:\\Users\\Sam Smith"),
            "the fixture must exercise a quoted argument"
        );
        assert!(on_demand_task_matches(&as_stored, &task));
    }

    /// A changed label, program or argument list means the stored task is
    /// stale and must be replaced — arguments included, since the log
    /// directory can move while the other two stay put.
    #[test]
    fn a_stale_task_is_not_up_to_date() {
        let registered = build_on_demand_task_xml(&keeper_task());

        let mut newer = keeper_task();
        newer.label = "zaaheen-keeper-task-v2".into();
        assert!(!on_demand_task_matches(&registered, &newer));

        let mut moved = keeper_task();
        moved.program = PathBuf::from(r"D:\Zaaheen\zaaheen-maintenance.exe");
        assert!(!on_demand_task_matches(&registered, &moved));

        let mut new_logs = keeper_task();
        new_logs.args[2] = r"C:\Users\sam\AppData\Local\com.zaaheen.app\logs".into();
        assert!(!on_demand_task_matches(&registered, &new_logs));

        assert!(!on_demand_task_matches("<Task/>", &keeper_task()));
    }

    #[test]
    fn an_on_demand_task_with_a_control_character_or_no_program_is_rejected() {
        let mut t = keeper_task();
        t.args.push("x\ny".into());
        assert!(t.validate().is_err());

        let mut t = keeper_task();
        t.program = PathBuf::new();
        assert!(t.validate().is_err());

        assert!(keeper_task().validate().is_ok());
    }
}
