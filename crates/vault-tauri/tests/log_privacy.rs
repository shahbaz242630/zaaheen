//! Memory content must never reach a log file (ADR-SEC-014).
//!
//! # Why this is a source-level test
//!
//! ADR-SEC-014 adds an on-disk log so a beta tester's problem can actually be
//! diagnosed. A log file is plaintext on disk by construction — which is
//! exactly the shape of ADR-SEC-007, where `reports/personal.report.json` sat
//! readable with no key, holding verbatim fact text.
//!
//! Writing memory content into a log would recreate that leak with a different
//! filename, and it would be worse in one respect: the sealed REPORT at least
//! looked like a vault artifact. A log file looks harmless, gets attached to
//! support emails, and gets pasted into issue trackers.
//!
//! A behavioural test cannot cover this. It could only prove that the log
//! lines it happens to trigger are clean, and the risk is the call site nobody
//! thought about — added next month, by someone reasonably trying to debug a
//! retrieval problem by logging the query and the fact it matched.
//!
//! So this scans the workspace's own source for `tracing` call sites that
//! record memory content, and fails the build. Same enforcement pattern as
//! `vault-consolidator/tests/prompt_guard_coverage.rs`, and for the same
//! reason: ADR-SEC-007's lesson was that the durable fix is not the patch, it
//! is the check that makes the next omission impossible.

use std::fs;
use std::path::{Path, PathBuf};

/// Field names that would put user memory text into a log line.
///
/// These are the field names actually used across this workspace for memory
/// text and user queries. Recording any of them on a `tracing` event writes
/// the user's private content to a plaintext file.
const FORBIDDEN_FIELDS: &[&str] = &[
    "content =",
    "content=",
    "fact =",
    "fact=",
    "query_text =",
    "query_text=",
    "merged_text =",
    "merged_text=",
    "memory_content =",
    "memory_content=",
    "query =",
    "query=",
];

/// `tracing` macros whose arguments become log output.
const TRACING_MACROS: &[&str] = &[
    "tracing::trace!",
    "tracing::debug!",
    "tracing::info!",
    "tracing::warn!",
    "tracing::error!",
    "trace!(",
    "debug!(",
    "info!(",
    "warn!(",
    "error!(",
];

fn workspace_crates_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR = crates/vault-tauri
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/ is the parent of this crate")
        .to_path_buf()
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            // `target/` under any crate is build output, not our source.
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// A `tracing` call spanning one or more lines, flattened for scanning.
///
/// `tracing` events are routinely written across several lines by rustfmt, so
/// a line-at-a-time scan would miss a field sitting two lines below the macro.
/// This walks forward from a macro invocation to its closing `);`, bounded so a
/// malformed match cannot run away through the whole file.
fn tracing_call_bodies(text: &str) -> Vec<(usize, String)> {
    const MAX_CALL_LINES: usize = 25;
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        // Comments describing logging are not logging.
        if trimmed.starts_with("//") {
            continue;
        }
        if !TRACING_MACROS.iter().any(|m| line.contains(m)) {
            continue;
        }
        let mut body = String::new();
        for line in lines.iter().skip(i).take(MAX_CALL_LINES) {
            let l = line.trim_start();
            if !l.starts_with("//") {
                body.push_str(line);
                body.push('\n');
            }
            if line.contains(");") {
                break;
            }
        }
        out.push((i + 1, body));
    }
    out
}

#[test]
fn no_tracing_call_records_memory_content() {
    let crates = workspace_crates_dir();
    let mut files = Vec::new();
    rust_files(&crates, &mut files);
    files.sort();

    assert!(
        !files.is_empty(),
        "found no .rs files under {} — this test would pass vacuously",
        crates.display()
    );

    let mut violations: Vec<String> = Vec::new();
    let mut scanned = 0usize;

    for file in &files {
        // This test's own source names every forbidden field, and test code
        // does not ship. Skipping `tests/` keeps the check on production code
        // without an allow-list that could later hide a real call site.
        if file.components().any(|c| c.as_os_str() == "tests") {
            continue;
        }
        let Ok(text) = fs::read_to_string(file) else {
            continue;
        };
        for (line, body) in tracing_call_bodies(&text) {
            scanned += 1;
            for field in FORBIDDEN_FIELDS {
                if body.contains(field) {
                    violations.push(format!(
                        "{}:{line} records `{field}`",
                        file.strip_prefix(&crates).unwrap_or(file).display()
                    ));
                }
            }
        }
    }

    // A check that inspects nothing is worse than no check: it reads as a
    // passing control. If the macro list stops matching how this workspace
    // logs, fail loudly rather than going quietly green.
    assert!(
        scanned >= 20,
        "expected to scan many tracing calls, found {scanned} — the patterns \
         this test greps for have probably changed. Update TRACING_MACROS \
         deliberately; do not delete this assertion."
    );

    assert!(
        violations.is_empty(),
        "ADR-SEC-014 violation — these log lines would write user memory \
         content to a PLAINTEXT file on disk:\n  {}\n\nLog what happened, never \
         what was remembered: ids, counts, durations, error kinds and boundary \
         names are fine; memory text, fact content and query text are not. \
         This is the ADR-SEC-007 defect class wearing a different filename — a \
         log file looks harmless, gets attached to support emails, and gets \
         pasted into issue trackers.",
        violations.join("\n  ")
    );
}

#[test]
fn the_scanner_sees_multi_line_calls_and_skips_comments() {
    // The scanner is load-bearing, so it gets its own test. Without this, a
    // future tidy-up to a single-line scan would silently stop catching the
    // most common shape: a rustfmt-wrapped multi-line event.
    let multi =
        "    tracing::info!(\n        target: \"x\",\n        content = %m.content,\n    );\n";
    let bodies = tracing_call_bodies(multi);
    assert_eq!(bodies.len(), 1, "one call found");
    assert!(
        bodies[0].1.contains("content ="),
        "a field two lines below the macro must still be seen"
    );

    // A comment mentioning a forbidden field is prose, not logging — the
    // false positive that broke prompt_guard_coverage.rs on its first run.
    let commented = "    // never log content = the user's text\n    let x = 1;\n";
    assert!(
        tracing_call_bodies(commented).is_empty(),
        "a comment must not register as a tracing call"
    );

    // A clean call must not be flagged.
    let clean = "    tracing::info!(memory_id = %id, count = n, \"stored\");\n";
    let bodies = tracing_call_bodies(clean);
    assert_eq!(bodies.len(), 1);
    for field in FORBIDDEN_FIELDS {
        assert!(
            !bodies[0].1.contains(field),
            "a clean call must not match {field}"
        );
    }
}

// ---- `#[instrument]` arguments (security audit s89) ---------------------------------
//
// `#[instrument]` records every argument it does not skip as a span field,
// and every event inside the span carries those fields into the log line. Two
// audits found memory text recorded this way (`apply_merge`'s `merged_text`,
// the keyword search's `query`) and the graph store recording an entity's
// `name`: nothing leaked only because no event fired inside those spans yet.
// The macro scan above never saw them, because no macro names the field.

/// Argument names that carry memory text, user queries or names taken from
/// memories. An instrumented function with one of these must skip it (or use
/// `skip_all`); record its length or an id instead.
const CONTENT_ARGUMENTS: &[&str] = &[
    "content",
    "fact",
    "query",
    "query_text",
    "merged_text",
    "merged_reasoning",
    "memory_content",
    "text",
    "name",
    "prompt",
    "new_memory",
    "memory",
];

/// One instrumented function: the attribute text and its argument names.
struct Instrumented {
    line: usize,
    attribute: String,
    arguments: Vec<String>,
}

/// Finds each `#[instrument` / `#[tracing::instrument` attribute and the
/// argument names of the function under it. Bounded like the macro scan.
fn instrumented_functions(text: &str) -> Vec<Instrumented> {
    const MAX_LINES: usize = 40;
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if !(trimmed.starts_with("#[instrument") || trimmed.starts_with("#[tracing::instrument")) {
            continue;
        }
        let mut chunk = String::new();
        for l in lines.iter().skip(i).take(MAX_LINES) {
            if !l.trim_start().starts_with("//") {
                chunk.push_str(l);
                chunk.push('\n');
            }
        }
        let Some(fn_at) = find_fn_keyword(&chunk) else {
            continue;
        };
        let attribute = chunk[..fn_at].to_owned();
        let Some(open) = chunk[fn_at..].find('(') else {
            continue;
        };
        let mut depth = 0usize;
        let mut signature = String::new();
        for c in chunk[fn_at + open..].chars() {
            match c {
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                _ => {}
            }
            signature.push(c);
            if depth == 0 {
                break;
            }
        }
        out.push(Instrumented {
            line: i + 1,
            attribute,
            arguments: argument_names(&signature),
        });
    }
    out
}

/// Byte offset of the first `fn ` keyword (as a whole word) in `chunk`.
fn find_fn_keyword(chunk: &str) -> Option<usize> {
    let bytes = chunk.as_bytes();
    chunk
        .match_indices("fn ")
        .map(|(at, _)| at)
        .find(|&at| at == 0 || !(bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_'))
}

/// `name: Type` pairs at the top level of a parameter list.
fn argument_names(signature: &str) -> Vec<String> {
    let inner = signature.trim_start_matches('(').trim_end_matches(')');
    let mut names = Vec::new();
    let mut depth = 0i32;
    let mut part = String::new();
    for c in inner.chars().chain(std::iter::once(',')) {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            _ => {}
        }
        if c == ',' && depth == 0 {
            if let Some((name, _)) = part.split_once(':') {
                let name = name.trim().trim_start_matches("mut ").trim();
                if !name.is_empty() && name.chars().all(|ch| ch.is_alphanumeric() || ch == '_') {
                    names.push(name.to_owned());
                }
            }
            part.clear();
        } else {
            part.push(c);
        }
    }
    names
}

/// Whether the attribute keeps `argument` out of the span.
fn skips(attribute: &str, argument: &str) -> bool {
    if attribute.contains("skip_all") {
        return true;
    }
    let Some(start) = attribute.find("skip(") else {
        return false;
    };
    let rest = &attribute[start + "skip(".len()..];
    let list = rest.split(')').next().unwrap_or("");
    list.split(',').any(|s| s.trim() == argument)
}

#[test]
fn no_instrumented_function_records_memory_content() {
    let crates = workspace_crates_dir();
    let mut files = Vec::new();
    rust_files(&crates, &mut files);
    files.sort();

    let mut violations = Vec::new();
    let mut scanned = 0usize;
    for file in &files {
        if file.components().any(|c| c.as_os_str() == "tests") {
            continue;
        }
        let Ok(text) = fs::read_to_string(file) else {
            continue;
        };
        for f in instrumented_functions(&text) {
            scanned += 1;
            for argument in &f.arguments {
                if CONTENT_ARGUMENTS.contains(&argument.as_str()) && !skips(&f.attribute, argument)
                {
                    violations.push(format!(
                        "{}:{} records argument `{argument}`",
                        file.strip_prefix(&crates).unwrap_or(file).display(),
                        f.line
                    ));
                }
            }
        }
    }

    assert!(
        scanned >= 50,
        "expected to scan many #[instrument] functions, found {scanned}; the \
         attribute patterns this test looks for have probably changed"
    );
    assert!(
        violations.is_empty(),
        "ADR-SEC-014 violation: these #[instrument] spans record user memory \
         text, and every event inside them would write it to the plaintext log \
         file:\n  {}\n\nAdd the argument to `skip(...)` and record its length \
         or an id instead.",
        violations.join("\n  ")
    );
}

#[test]
fn the_instrument_scanner_sees_recorded_and_skipped_arguments() {
    let recorded = "    #[instrument(skip(self), fields(limit))]\n    pub async fn search(&self, query: &str, limit: usize) -> R {\n";
    let found = instrumented_functions(recorded);
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0].arguments,
        vec!["query".to_owned(), "limit".to_owned()]
    );
    assert!(
        !skips(&found[0].attribute, "query"),
        "query is recorded here"
    );

    let multi = "#[instrument(\n    skip(cluster, merged_text, storage),\n    fields(id = cluster.id)\n)]\npub async fn apply_merge(\n    cluster: &Cluster,\n    merged_text: &str,\n    storage: &Map<K, V>,\n) -> R {\n";
    let found = instrumented_functions(multi);
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0].arguments,
        vec![
            "cluster".to_owned(),
            "merged_text".to_owned(),
            "storage".to_owned()
        ]
    );
    assert!(skips(&found[0].attribute, "merged_text"));

    let all = "#[tracing::instrument(skip_all)]\nfn f(content: String) {}\n";
    let found = instrumented_functions(all);
    assert!(skips(&found[0].attribute, "content"));
}
