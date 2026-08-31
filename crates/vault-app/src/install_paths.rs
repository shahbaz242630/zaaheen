//! Where an *installed* Zaaheen keeps its files (ADR-101).
//!
//! ## Why this module exists
//!
//! The desktop app never needed it: Tauri hands it `app_data_dir()` and
//! `BaseDirectory::Resource`. `vault-cli` has neither, so every path was a
//! required argument — and the MCP config snippet the app tells users to paste
//! promised `zaaheen mcp serve` with no arguments at all. Running that produced
//!
//! ```text
//! error: the following required arguments were not provided:
//!   --bge-model <PATH> --bge-tokenizer <PATH> --ort-lib <PATH>
//! ```
//!
//! The snippet was not wrong about what it should be. `zaaheen mcp serve`
//! *should* just work — the binary knows where it was installed and the vault
//! has exactly one home. This module is the missing knowledge, so the promise
//! the app already makes becomes true.
//!
//! ## The layouts mirrored here
//!
//! These are NOT independent choices — every one restates a location the
//! desktop app already resolves, and the two MUST agree or the CLI opens a
//! different vault than the app and the user's memories appear to vanish:
//!
//! | | app (`vault-tauri/src/main.rs`) | here |
//! |---|---|---|
//! | data dir | `app.path().app_data_dir()` | [`data_dir`] |
//! | metadata DB | `data_dir.join("vault.db")` | [`vault_db_in`] |
//! | vectors | `data_dir.join("lance")` | [`vector_dir_in`] |
//! | graph | `data_dir.join("graph.duckdb")` | [`graph_db_in`] |
//! | models | `data_dir.join("models")` | [`models_dir_in`] |
//! | embedder | `resolve("models/model.onnx", Resource)` | [`bge_model_in`] |
//! | ONNX Runtime | `resolve(dylib_filename_for_os(), Resource)` | [`ort_lib_in`] |
//!
//! `installer_contract.rs` pins [`APP_IDENTIFIER`] against `tauri.conf.json`,
//! so the one value that could silently drift cannot.
//!
//! ## Resources resolve as siblings of the executable
//!
//! Tauri's `BaseDirectory::Resource` is the directory holding the binary, and
//! the installer places `zaaheen.exe` there beside `models/` and `libs/`. So
//! `current_exe().parent()` is the same directory the app resolves against —
//! the identical reasoning `vault-maintenance`'s `sibling_vault_cli` already
//! relies on, and it is resistant to `PATH` shadowing for the same reason.
//!
//! ## These are DEFAULTS, never overrides
//!
//! Every caller applies them only where the user supplied nothing. An explicit
//! `--vault-db`, or the `VAULT_*` environment variables, still win. A developer
//! running out of `target/debug` has no `models/` sibling and no installed data
//! directory; they pass paths explicitly exactly as before, and nothing here
//! changes that.

use std::path::{Path, PathBuf};

/// Reverse-DNS identifier that names the app's data directory.
///
/// **Single source of truth is `vault-tauri/tauri.conf.json`'s `identifier`.**
/// This constant restates it for the non-Tauri binaries, and
/// `installer_contract.rs::cli_default_paths_use_the_bundle_identifier` fails
/// the build if the two ever diverge. Changing it moves every user's vault, so
/// it is not a value to edit casually (ADR-SEC-018 covers what that costs).
pub const APP_IDENTIFIER: &str = "com.zaaheen.app";

/// Directory name holding the bundled embedder, and the downloaded models.
const MODELS_SUBDIR: &str = "models";

/// The embedder ONNX file, relative to the resource directory.
const BGE_MODEL_RELATIVE: &str = "models/model.onnx";

/// The embedder tokenizer, relative to the resource directory.
const BGE_TOKENIZER_RELATIVE: &str = "models/tokenizer.json";

/// Per-OS ONNX Runtime library path, relative to the resource directory.
///
/// Mirrors `vault_tauri::dylib_filename_for_os`. Kept as a `match` on
/// [`std::env::consts::OS`] rather than `cfg!` so an unsupported platform is a
/// `None` the caller can report, not a silently wrong path.
fn ort_lib_relative() -> Option<&'static str> {
    match std::env::consts::OS {
        "windows" => Some("libs/onnxruntime.dll"),
        "macos" => Some("libs/libonnxruntime.dylib"),
        "linux" => Some("libs/libonnxruntime.so"),
        _ => None,
    }
}

/// The per-user data directory an installed Zaaheen writes to.
///
/// Mirrors Tauri's `app_data_dir()` per platform:
/// - Windows: `%APPDATA%\com.zaaheen.app` (Roaming, matching Tauri)
/// - macOS: `~/Library/Application Support/com.zaaheen.app`
/// - Linux: `$XDG_DATA_HOME/com.zaaheen.app`, else `~/.local/share/...`
///
/// Returns `None` when the environment does not name a home — a stripped
/// service account, say. Callers treat that as "no default available" and
/// require an explicit path, which is the honest outcome: guessing a data
/// directory is how a second, empty vault gets created.
pub fn data_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    };
    base.filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.join(APP_IDENTIFIER))
}

/// The directory holding this executable — Tauri's `BaseDirectory::Resource`.
///
/// Resolved from [`std::env::current_exe`] rather than a `PATH` lookup, so a
/// different `zaaheen` earlier in `PATH` cannot redirect us.
pub fn resource_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
}

/// `<data_dir>/vault.db` — the SQLCipher metadata database.
pub fn vault_db_in(data_dir: &Path) -> PathBuf {
    data_dir.join("vault.db")
}

/// `<data_dir>/lance` — the vector store directory.
pub fn vector_dir_in(data_dir: &Path) -> PathBuf {
    data_dir.join("lance")
}

/// `<data_dir>/graph.duckdb` — the graph database file.
pub fn graph_db_in(data_dir: &Path) -> PathBuf {
    data_dir.join("graph.duckdb")
}

/// `<data_dir>/models` — where downloaded models land (ADR-100).
pub fn models_dir_in(data_dir: &Path) -> PathBuf {
    data_dir.join(MODELS_SUBDIR)
}

/// `<resource_dir>/models/model.onnx` — the bundled embedder.
pub fn bge_model_in(resource_dir: &Path) -> PathBuf {
    resource_dir.join(BGE_MODEL_RELATIVE)
}

/// `<resource_dir>/models/tokenizer.json` — the bundled embedder tokenizer.
pub fn bge_tokenizer_in(resource_dir: &Path) -> PathBuf {
    resource_dir.join(BGE_TOKENIZER_RELATIVE)
}

/// `<resource_dir>/libs/<onnxruntime dylib>`, or `None` on an OS we do not ship.
pub fn ort_lib_in(resource_dir: &Path) -> Option<PathBuf> {
    ort_lib_relative().map(|rel| resource_dir.join(rel))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_is_reverse_dns_and_names_zaaheen() {
        // Non-vacuity for the installer_contract guard: an empty or truncated
        // constant would satisfy a `contains` check against almost anything.
        assert!(
            APP_IDENTIFIER.starts_with("com.zaaheen"),
            "identifier must stay under the zaaheen reverse-DNS prefix; got {APP_IDENTIFIER:?}"
        );
        assert!(
            APP_IDENTIFIER.len() > "com.zaaheen".len(),
            "identifier must be more than the bare prefix; got {APP_IDENTIFIER:?}"
        );
    }

    #[test]
    fn data_dir_layout_matches_what_the_desktop_app_resolves() {
        // The literals here are deliberately re-typed rather than referencing
        // the helpers: this test's whole job is to catch a helper being
        // "tidied" into a different filename. Binding the constant would make
        // it agree with any change, which is the opposite of a pin.
        let d = Path::new("/tmp/zaaheen-data");
        assert_eq!(vault_db_in(d), d.join("vault.db"));
        assert_eq!(vector_dir_in(d), d.join("lance"));
        assert_eq!(graph_db_in(d), d.join("graph.duckdb"));
        assert_eq!(models_dir_in(d), d.join("models"));
    }

    #[test]
    fn resource_layout_matches_the_tauri_resource_mapping() {
        // Mirrors tauri.conf.json's `bundle.resources` map, which installs
        // onnxruntime -> libs/, and model.onnx + tokenizer.json -> models/.
        let r = Path::new("/opt/zaaheen");
        assert_eq!(bge_model_in(r), r.join("models/model.onnx"));
        assert_eq!(bge_tokenizer_in(r), r.join("models/tokenizer.json"));
    }

    #[test]
    fn ort_lib_is_resolved_for_every_platform_we_ship() {
        // ort_lib_in returns Option because an unshipped OS must surface as a
        // reportable None, never a plausible-looking wrong path. On the three
        // platforms we build for, it must be Some and sit under libs/.
        let r = Path::new("/opt/zaaheen");
        match ort_lib_relative() {
            Some(rel) => {
                assert!(
                    rel.starts_with("libs/"),
                    "the runtime library ships under libs/; got {rel:?}"
                );
                assert_eq!(ort_lib_in(r), Some(r.join(rel)));
            }
            None => panic!(
                "no ONNX Runtime path for {} - add it here and to \
                 vault_tauri::dylib_filename_for_os together",
                std::env::consts::OS
            ),
        }
    }

    #[test]
    fn data_dir_ends_with_the_identifier_when_the_environment_names_a_home() {
        // Environment-dependent by nature, so this asserts the SHAPE rather
        // than an absolute path: wherever the platform puts it, the leaf must
        // be the bundle identifier, because that leaf is what decides which
        // vault the CLI opens.
        if let Some(d) = data_dir() {
            assert!(
                d.ends_with(APP_IDENTIFIER),
                "data dir must live under the bundle identifier; got {}",
                d.display()
            );
        }
    }
}
