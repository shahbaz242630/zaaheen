//! How a `zaaheen` process ends (ADR-SEC-048, session 109).
//!
//! On macOS, ONNX Runtime's own C++ teardown aborts the process at exit
//! (`libc++abi: ... mutex lock failed: Invalid argument`, SIGABRT from
//! `exit` → `__cxa_finalize_ranges`; microsoft/onnxruntime#24579, ADR-033):
//! its logging mutex is destroyed before the environment that locks it, and
//! the `ort` crate keeps that environment in a static that is never
//! released. So every program that loaded the search model crashed as it
//! closed: the keeper at every idle exit, and the nightly tidy-up's child
//! after its work was saved, which made the runner record the night as
//! failed (launch checklist A12b and A9; GitHub Mac run 39775700327).
//!
//! On macOS the process therefore ends with `_exit`, which skips the C and
//! C++ exit handlers and, at the end of `main`, the tokio runtime's drop.
//! What that gives up: work still in flight (a blocking task, a tokio
//! stdout write) is abandoned, as in a crash; everything committed stays
//! committed. Nothing of ours depends on the handlers: Rust runs no
//! destructors at exit either, the log file is written unbuffered
//! (`vault_app::logging`), stderr is unbuffered, and std's stdout is flushed
//! here first. Every other platform keeps `std::process::exit`.
//!
//! This is the crate's one `unsafe` block, and every exit of the program
//! goes through this module; both are pinned by the tests below.

use std::process::ExitCode;

/// End the process now with `code`, standard streams flushed.
pub fn now(code: i32) -> ! {
    flush_standard_streams();
    end(code)
}

/// The end of `main`: on macOS the process ends here (see the module docs);
/// elsewhere `code` is returned for `main` to exit with as before.
pub fn finish(code: u8) -> ExitCode {
    #[cfg(target_os = "macos")]
    {
        now(i32::from(code))
    }
    #[cfg(not(target_os = "macos"))]
    {
        ExitCode::from(code)
    }
}

fn flush_standard_streams() {
    use std::io::Write;
    // Nothing to do with a failure here: the process is ending either way.
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn end(code: i32) -> ! {
    // SAFETY: `_exit` takes no pointers and has no preconditions; it ends
    // the process at once. Skipping the C and C++ exit handlers is the
    // point (module docs, ADR-SEC-048).
    unsafe { libc::_exit(code) }
}

#[cfg(not(target_os = "macos"))]
fn end(code: i32) -> ! {
    std::process::exit(code)
}

#[cfg(test)]
mod tests {
    /// The program's sources, as compiled into the `zaaheen` binary.
    const SOURCES: &[(&str, &str)] = &[
        ("main.rs", include_str!("main.rs")),
        ("keeper.rs", include_str!("keeper.rs")),
        ("exit.rs", include_str!("exit.rs")),
    ];

    /// Code lines only: comment lines dropped, then everything from the
    /// first test module on cut off (this one names the patterns it hunts
    /// for). Cut at a test MODULE, so a lone `#[cfg(test)]` item mid-file
    /// cannot silently shorten the scan.
    fn code_of(source: &str) -> String {
        let code = source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        match code.split_once("#[cfg(test)]\nmod ") {
            Some((before, _)) => before.to_string(),
            None => code,
        }
    }

    /// ADR-SEC-048: the one `unsafe` block is `_exit`, in this module, on
    /// macOS only. A second one anywhere in the program fails here.
    #[test]
    fn the_only_unsafe_code_is_the_macos_exit() {
        let unsafe_block = concat!("unsafe", " {");
        let allow = concat!("allow(", "unsafe_code)");
        for (name, source) in SOURCES {
            let code = code_of(source);
            let blocks = code.matches(unsafe_block).count();
            let allows = code.matches(allow).count();
            if *name == "exit.rs" {
                assert_eq!(blocks, 1, "exit.rs: exactly one unsafe block");
                assert_eq!(allows, 1, "exit.rs: exactly one allow");
                assert!(
                    code.contains("#[cfg(target_os = \"macos\")]\n#[allow(unsafe_code)]\nfn end("),
                    "the unsafe exit must be the macOS-only `end`"
                );
                assert!(code.contains(concat!("unsafe", " { libc::_exit(code) }")));
            } else {
                assert_eq!(blocks, 0, "{name}: unsafe code belongs in exit.rs only");
                assert_eq!(allows, 0, "{name}: unsafe code belongs in exit.rs only");
            }
        }
    }

    /// Every way the program ends goes through this module, so the macOS
    /// crash cannot come back through an exit that skipped it.
    #[test]
    fn every_exit_goes_through_this_module() {
        // No `(`: also catches `use std::process::exit;` then `exit(1)`.
        let direct_exit = concat!("process::", "exit");
        for (name, source) in SOURCES.iter().filter(|(n, _)| *n != "exit.rs") {
            assert!(
                !code_of(source).contains(direct_exit),
                "{name}: end the process with exit::now, not std::process::exit"
            );
        }
        let main = code_of(include_str!("main.rs"));
        let body = main
            .split_once("async fn main() -> ExitCode {")
            .map(|(_, rest)| rest.split_once("\n}\n").map_or(rest, |(b, _)| b))
            .unwrap_or_default();
        assert!(
            body.contains("exit::finish(") && !body.contains("ExitCode::SUCCESS"),
            "main must end through exit::finish"
        );
    }
}
