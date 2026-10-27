//! Where a file the person asked to save may go: the one rule for the memory
//! export and the log export (s115 tech-debt item 2).
//!
//! Absolute, in a folder that exists, no NUL, and never a network share. A
//! relative path would resolve against whatever directory the app was
//! launched from, and the person would never find the file. A UNC path
//! (`\\host\share\...`) is "absolute" and its parent can be a real directory,
//! so without its own check the file would be written over SMB to another
//! machine: the memory export promises it "sends nothing anywhere", and a log
//! export holds the same diagnostic detail it promises stays on this computer.

use std::path::PathBuf;

/// The destination as a path, or `None` when it breaks the rule. Each caller
/// answers its own stable error code.
pub fn local_destination(destination: &str) -> Option<PathBuf> {
    if destination.trim().is_empty() || destination.contains('\0') {
        return None;
    }
    if destination.starts_with("\\\\") || destination.starts_with("//") {
        return None;
    }
    let path = PathBuf::from(destination);
    if !path.is_absolute() {
        return None;
    }
    match path.parent() {
        Some(dir) if dir.is_dir() => Some(path),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_network_share_is_never_a_destination() {
        for unc in [
            r"\\attacker-host\share\file.txt",
            "//attacker-host/share/file.txt",
            r"\\?\UNC\host\share\file.txt",
        ] {
            assert!(
                local_destination(unc).is_none(),
                "{unc:?} would leave this computer"
            );
        }
    }

    #[test]
    fn relative_empty_and_nul_bearing_destinations_are_refused() {
        for bad in ["file.txt", "", "   ", "/tmp/a\0b.txt"] {
            assert!(local_destination(bad).is_none(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn a_file_in_an_existing_folder_is_accepted() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let dest = tmp.path().join("file.txt");
        assert_eq!(local_destination(&dest.to_string_lossy()), Some(dest));
        let missing = tmp.path().join("nope").join("file.txt");
        assert!(local_destination(&missing.to_string_lossy()).is_none());
    }
}
