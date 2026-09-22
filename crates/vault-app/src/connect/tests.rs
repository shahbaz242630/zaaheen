//! ADR-106: asking an app to install Zaaheen through its own route. Nothing
//! here opens a program or reads the registry except the one Windows test
//! that asks about a scheme nobody registered.

use std::cell::RefCell;
use std::io::Read;

use super::*;
use crate::external_link::CURSOR_INSTALL_LINK;

const ICON: &[u8] = b"\x89PNG\r\n\x1a\n not really an icon";

// ── Cursor ────────────────────────────────────────────────────────────────

#[test]
fn a_cursor_that_is_not_there_is_never_opened() {
    let opened = RefCell::new(false);
    let outcome = connect_cursor_with(
        || false,
        |_| {
            *opened.borrow_mut() = true;
            Ok(())
        },
    );
    assert_eq!(outcome, ConnectOutcome::AppNotFound);
    assert!(!*opened.borrow(), "nothing is opened without Cursor");
}

#[test]
fn cursor_is_handed_exactly_the_fixed_install_link() {
    let opened = RefCell::new(String::new());
    let outcome = connect_cursor_with(
        || true,
        |link| {
            *opened.borrow_mut() = link.as_str().to_owned();
            Ok(())
        },
    );
    assert_eq!(outcome, ConnectOutcome::Asked);
    assert_eq!(*opened.borrow(), CURSOR_INSTALL_LINK);
}

#[test]
fn a_cursor_that_will_not_start_is_said_so() {
    let outcome = connect_cursor_with(|| true, |_| Err(LinkError::CouldNotOpen));
    assert_eq!(outcome, ConnectOutcome::CouldNotOpen);
}

// ── the Claude extension ──────────────────────────────────────────────────

fn entry(bundle: &[u8], name: &str) -> Vec<u8> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bundle)).expect("a zip");
    let mut file = archive
        .by_name(name)
        .unwrap_or_else(|_| panic!("no {name}"));
    let mut out = Vec::new();
    file.read_to_end(&mut out).unwrap();
    out
}

/// The extension runs the installed Zaaheen by its short name, exactly the
/// manual snippet's server, the form the session-55 live test showed Claude
/// running; it carries no program.
#[test]
fn the_claude_extension_runs_the_installed_zaaheen() {
    let bundle = claude_extension(ICON).unwrap();
    let manifest: serde_json::Value =
        serde_json::from_slice(&entry(&bundle, "manifest.json")).unwrap();
    assert_eq!(manifest["manifest_version"], "0.3");
    assert_eq!(manifest["display_name"], "Zaaheen");
    assert_eq!(manifest["server"]["type"], "binary");
    assert_eq!(
        manifest["server"]["mcp_config"],
        serde_json::json!({ "command": "zaaheen", "args": ["mcp", "serve"] })
    );
    assert_eq!(
        manifest["compatibility"]["platforms"],
        serde_json::json!(["win32"])
    );
    assert_eq!(manifest["icon"], "icon.png");
    assert_eq!(entry(&bundle, "icon.png"), ICON);
    let entry_point = manifest["server"]["entry_point"].as_str().unwrap();
    assert!(
        !entry(&bundle, entry_point).is_empty(),
        "the entry point is there"
    );

    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bundle[..])).unwrap();
    let names: Vec<String> = archive.file_names().map(str::to_owned).collect();
    assert_eq!(
        names.len(),
        3,
        "manifest, icon and the note, nothing else: {names:?}"
    );
    for i in 0..archive.len() {
        let file = archive.by_index(i).unwrap();
        assert!(
            !file.name().contains('\\'),
            "a backslash in {}",
            file.name()
        );
        assert!(
            !file.name().ends_with(".exe") && !file.name().ends_with(".dll"),
            "the extension carries a program: {}",
            file.name()
        );
    }
}

#[test]
fn the_same_icon_always_makes_the_same_extension() {
    assert_eq!(
        claude_extension(ICON).unwrap(),
        claude_extension(ICON).unwrap()
    );
}

/// What Claude shows the person names no part of our stack (ADR-086).
#[test]
fn the_extension_names_nothing_of_the_stack() {
    let manifest = String::from_utf8(entry(&claude_extension(ICON).unwrap(), "manifest.json"))
        .unwrap()
        .to_lowercase();
    for stack in [
        "lance",
        "sqlite",
        "sqlcipher",
        "duckdb",
        "qwen",
        "phi",
        "onnx",
        "bge",
        "llama",
        "tauri",
    ] {
        assert!(!manifest.contains(stack), "the manifest names {stack}");
    }
}

// ── connecting Claude ─────────────────────────────────────────────────────

struct Claude {
    installed: bool,
    opens_mcpb: bool,
    opens: bool,
}

fn connect(c: &Claude, downloads: Option<&Path>) -> (ConnectOutcome, Option<std::path::PathBuf>) {
    let opened = RefCell::new(None);
    let outcome = connect_claude_with(
        || c.installed,
        downloads,
        ICON,
        || c.opens_mcpb,
        |file| {
            *opened.borrow_mut() = Some(file.to_path_buf());
            if c.opens {
                Ok(())
            } else {
                Err(())
            }
        },
    );
    (outcome, opened.into_inner())
}

#[test]
fn no_claude_means_nothing_saved_and_nothing_opened() {
    let downloads = tempfile::tempdir().unwrap();
    let (outcome, opened) = connect(
        &Claude {
            installed: false,
            opens_mcpb: true,
            opens: true,
        },
        Some(downloads.path()),
    );
    assert_eq!(outcome, ConnectOutcome::AppNotFound);
    assert!(opened.is_none());
    assert_eq!(std::fs::read_dir(downloads.path()).unwrap().count(), 0);
}

/// Store Claude: Windows opens no `.mcpb`, so the file is saved and the page
/// says where to install it from; nothing is opened.
#[test]
fn where_windows_cannot_open_the_extension_it_is_saved_for_the_person() {
    let downloads = tempfile::tempdir().unwrap();
    let (outcome, opened) = connect(
        &Claude {
            installed: true,
            opens_mcpb: false,
            opens: true,
        },
        Some(downloads.path()),
    );
    assert_eq!(outcome, ConnectOutcome::Saved);
    assert!(opened.is_none());
    let saved = std::fs::read(downloads.path().join(CLAUDE_EXTENSION_FILE)).unwrap();
    assert_eq!(saved, claude_extension(ICON).unwrap());
    assert_eq!(
        std::fs::read_dir(downloads.path()).unwrap().count(),
        1,
        "only the extension, no part-file left"
    );
}

#[test]
fn where_windows_can_open_it_claude_is_handed_the_saved_file() {
    let downloads = tempfile::tempdir().unwrap();
    let (outcome, opened) = connect(
        &Claude {
            installed: true,
            opens_mcpb: true,
            opens: true,
        },
        Some(downloads.path()),
    );
    assert_eq!(outcome, ConnectOutcome::Asked);
    assert_eq!(opened, Some(downloads.path().join(CLAUDE_EXTENSION_FILE)));
}

/// It is saved either way, so a file that will not open still gets the steps.
#[test]
fn a_saved_extension_that_will_not_open_still_gets_the_steps() {
    let downloads = tempfile::tempdir().unwrap();
    let (outcome, _) = connect(
        &Claude {
            installed: true,
            opens_mcpb: true,
            opens: false,
        },
        Some(downloads.path()),
    );
    assert_eq!(outcome, ConnectOutcome::Saved);
    assert!(downloads.path().join(CLAUDE_EXTENSION_FILE).exists());
}

#[test]
fn an_older_copy_and_a_stray_part_file_are_replaced() {
    let downloads = tempfile::tempdir().unwrap();
    let file = downloads.path().join(CLAUDE_EXTENSION_FILE);
    std::fs::write(&file, b"an older extension").unwrap();
    std::fs::write(file.with_extension("mcpb.part"), b"half of one").unwrap();
    let (outcome, _) = connect(
        &Claude {
            installed: true,
            opens_mcpb: false,
            opens: false,
        },
        Some(downloads.path()),
    );
    assert_eq!(outcome, ConnectOutcome::Saved);
    assert_eq!(
        std::fs::read(&file).unwrap(),
        claude_extension(ICON).unwrap()
    );
    assert!(!file.with_extension("mcpb.part").exists());
}

#[test]
fn no_downloads_folder_means_could_not_save() {
    let (outcome, opened) = connect(
        &Claude {
            installed: true,
            opens_mcpb: true,
            opens: true,
        },
        None,
    );
    assert_eq!(outcome, ConnectOutcome::CouldNotSave);
    assert!(opened.is_none());
    let gone = tempfile::tempdir().unwrap().path().join("not-there");
    let (outcome, _) = connect(
        &Claude {
            installed: true,
            opens_mcpb: true,
            opens: true,
        },
        Some(&gone),
    );
    assert_eq!(outcome, ConnectOutcome::CouldNotSave);
}

// ── the codes and Windows ─────────────────────────────────────────────────

#[test]
fn every_outcome_has_its_own_code() {
    let codes: Vec<&str> = [
        ConnectOutcome::Asked,
        ConnectOutcome::Saved,
        ConnectOutcome::AppNotFound,
        ConnectOutcome::CouldNotSave,
        ConnectOutcome::CouldNotOpen,
    ]
    .into_iter()
    .map(ConnectOutcome::code)
    .collect();
    assert_eq!(
        codes,
        [
            "asked",
            "saved",
            "app_not_found",
            "could_not_save",
            "could_not_open"
        ]
    );
}

/// Reads the registry, changes nothing: nothing registered is not found.
#[cfg(windows)]
#[test]
fn an_unregistered_scheme_or_file_type_is_not_found() {
    assert!(!scheme_is_registered("zaaheen-test-no-such-scheme"));
    assert!(!file_type_is_registered(".zaaheen-test-no-such-type"));
}
