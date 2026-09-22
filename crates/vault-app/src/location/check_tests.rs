//! ADR-105 L4 — every refusal and acceptance, against a scripted machine.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

use super::check::*;
use super::Homes;

/// A machine described by the test.
#[derive(Default)]
struct Machine {
    vars: BTreeMap<String, PathBuf>,
    /// `None`: the sync-root list cannot be read.
    sync_roots: Option<Vec<PathBuf>>,
    dropbox: Vec<PathBuf>,
    /// Paths whose final form differs (a mapped drive, a redirected folder).
    redirect: BTreeMap<PathBuf, PathBuf>,
    missing: BTreeSet<PathBuf>,
    existing: BTreeSet<PathBuf>,
    read_only: bool,
    free: u64,
    install: Option<PathBuf>,
}

impl CheckEnv for Machine {
    fn folder_var(&self, name: &str) -> Option<PathBuf> {
        self.vars.get(name).cloned()
    }
    fn sync_roots(&self) -> Option<Vec<PathBuf>> {
        self.sync_roots.clone()
    }
    fn dropbox_roots(&self) -> Vec<PathBuf> {
        self.dropbox.clone()
    }
    fn canonical(&self, p: &Path) -> io::Result<PathBuf> {
        if self.missing.contains(p) {
            return Err(io::Error::from(io::ErrorKind::NotFound));
        }
        for (from, to) in &self.redirect {
            if let Ok(rest) = p.strip_prefix(from) {
                return Ok(to.join(rest));
            }
        }
        Ok(p.to_path_buf())
    }
    fn exists(&self, p: &Path) -> io::Result<bool> {
        Ok(self.existing.contains(p))
    }
    fn probe_writable(&self, _dir: &Path) -> io::Result<()> {
        if self.read_only {
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        } else {
            Ok(())
        }
    }
    fn available_space(&self, _p: &Path) -> io::Result<u64> {
        Ok(self.free)
    }
    fn install_dir(&self) -> Option<PathBuf> {
        self.install.clone()
    }
}

/// An absolute path on the system drive.
fn sys(rest: &str) -> PathBuf {
    let root = if cfg!(windows) { r"C:\" } else { "/" };
    Path::new(root).join(rest)
}

fn machine() -> Machine {
    let mut m = Machine {
        sync_roots: Some(Vec::new()),
        free: 100 * 1024 * 1024 * 1024,
        install: Some(sys("Program Files/Zaaheen")),
        ..Machine::default()
    };
    for (k, v) in [
        ("SystemRoot", sys("Windows")),
        ("ProgramFiles", sys("Program Files")),
        ("ProgramData", sys("ProgramData")),
        ("USERPROFILE", sys("Users/me")),
    ] {
        m.vars.insert(k.into(), v);
    }
    let drive = if cfg!(windows) { "C:" } else { "/" };
    m.vars.insert("SystemDrive".into(), PathBuf::from(drive));
    m
}

fn homes() -> Homes {
    Homes {
        local: sys("Users/me/AppData/Local/com.zaaheen.app"),
        roaming: sys("Users/me/AppData/Roaming/com.zaaheen.app"),
    }
}

fn check(m: &Machine, chosen: &Path, size: u64) -> Result<Checked, Refusal> {
    let h = homes();
    let current = h.roaming.clone();
    let models = h.roaming.join("models");
    let ctx = CheckContext {
        homes: &h,
        current_vault: Some(&current),
        models_dir: &models,
        vault_size: size,
    };
    check_folder(chosen, &ctx, m)
}

#[test]
fn an_ordinary_folder_on_the_system_drive_is_accepted_with_its_own_subfolder() {
    let chosen = sys("Users/me/Private");
    let ok = check(&machine(), &chosen, 1024).unwrap();
    assert_eq!(ok.target, chosen.join(VAULT_SUBFOLDER));
    assert!(ok.notes.is_empty(), "{:?}", ok.notes);
}

#[test]
fn a_relative_or_network_path_as_typed_is_refused() {
    assert_eq!(
        check(&machine(), Path::new("relative/folder"), 0),
        Err(Refusal::NotLocal)
    );
    if cfg!(windows) {
        assert_eq!(
            check(&machine(), Path::new(r"\\server\share\f"), 0),
            Err(Refusal::NotLocal)
        );
    }
}

#[cfg(windows)]
#[test]
fn a_mapped_network_drive_is_refused_by_where_it_really_is() {
    let mut m = machine();
    m.redirect.insert(
        PathBuf::from(r"Z:\"),
        PathBuf::from(r"\\?\UNC\server\share"),
    );
    assert_eq!(
        check(&m, Path::new(r"Z:\Memories"), 0),
        Err(Refusal::Network)
    );
}

#[test]
fn a_drive_root_is_refused() {
    let root = if cfg!(windows) {
        PathBuf::from(r"C:\")
    } else {
        PathBuf::from("/")
    };
    assert_eq!(check(&machine(), &root, 0), Err(Refusal::DriveRoot));
}

#[test]
fn windows_and_program_folders_are_refused() {
    for f in [
        "Windows/Temp",
        "Program Files/Other",
        "ProgramData/x",
        "Program Files/Zaaheen/x",
    ] {
        assert_eq!(
            check(&machine(), &sys(f), 0),
            Err(Refusal::SystemFolder),
            "{f}"
        );
    }
}

#[test]
fn zaaheens_own_folders_are_refused() {
    let h = homes();
    for f in [
        h.local.join("x"),
        h.roaming.join("y"),
        h.roaming.join("models").join("z"),
    ] {
        assert_eq!(
            check(&machine(), &f, 0),
            Err(Refusal::InsideAppFolders),
            "{}",
            f.display()
        );
    }
}

#[test]
fn onedrive_and_a_documents_folder_redirected_into_it_are_refused() {
    let mut m = machine();
    let onedrive = sys("Users/me/OneDrive");
    m.vars.insert("OneDrive".into(), onedrive.clone());
    assert_eq!(
        check(&m, &onedrive.join("Stuff"), 0),
        Err(Refusal::CloudSynced)
    );
    // Known Folder Move: Documents really lives inside OneDrive.
    m.redirect
        .insert(sys("Users/me/Documents"), onedrive.join("Documents"));
    assert_eq!(
        check(&m, &sys("Users/me/Documents"), 0),
        Err(Refusal::CloudSynced)
    );
}

#[test]
fn a_registered_sync_root_dropbox_icloud_and_google_drive_are_refused() {
    let mut m = machine();
    m.sync_roots = Some(vec![sys("Users/me/Contoso/SharePoint")]);
    m.dropbox = vec![sys("Users/me/Dropbox")];
    for f in [
        "Users/me/Contoso/SharePoint/Lib",
        "Users/me/Dropbox/x",
        "Users/me/iCloudDrive/x",
        "Users/me/Google Drive/x",
        "Users/me/Box/x",
    ] {
        assert_eq!(check(&m, &sys(f), 0), Err(Refusal::CloudSynced), "{f}");
    }
    // Google Drive for desktop's virtual drive.
    assert_eq!(
        check(&m, &sys("My Drive/Notes"), 0),
        Err(Refusal::CloudSynced)
    );
}

#[test]
fn when_the_sync_list_cannot_be_read_the_person_is_told_and_decides() {
    let mut m = machine();
    m.sync_roots = None;
    let ok = check(&m, &sys("Users/me/Private"), 0).unwrap();
    assert_eq!(ok.notes, vec![Note::CloudUnchecked]);
}

#[test]
fn an_existing_zaaheen_memories_folder_is_refused() {
    let mut m = machine();
    let chosen = sys("Users/me/Private");
    m.existing.insert(chosen.join(VAULT_SUBFOLDER));
    assert_eq!(check(&m, &chosen, 0), Err(Refusal::AlreadyExists));
}

#[test]
fn a_read_only_or_missing_folder_is_refused() {
    let mut m = machine();
    m.read_only = true;
    assert_eq!(
        check(&m, &sys("Users/me/Private"), 0),
        Err(Refusal::NotWritable)
    );
    let mut m = machine();
    m.missing.insert(sys("Users/me/Gone"));
    assert_eq!(
        check(&m, &sys("Users/me/Gone"), 0),
        Err(Refusal::Unreachable)
    );
}

#[test]
fn the_memories_must_fit_with_ten_percent_to_spare() {
    let mut m = machine();
    m.free = 1_000;
    assert_eq!(
        check(&m, &sys("Users/me/Private"), 1_000),
        Err(Refusal::NotEnoughSpace)
    );
    m.free = 1_100;
    assert!(check(&m, &sys("Users/me/Private"), 1_000).is_ok());
}

#[cfg(windows)]
#[test]
fn another_drive_gets_the_usb_note() {
    let ok = check(&machine(), Path::new(r"E:\Backup"), 0).unwrap();
    assert_eq!(ok.target, PathBuf::from(r"E:\Backup").join(VAULT_SUBFOLDER));
    assert_eq!(ok.notes, vec![Note::OtherDrive]);
}

#[test]
fn the_registry_listing_yields_every_user_sync_root() {
    let out = "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Explorer\\SyncRootManager\\OneDrive!S-1-5-21-1!Personal|X\r\n    DisplayNameResource    REG_SZ    OneDrive\r\n\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Explorer\\SyncRootManager\\OneDrive!S-1-5-21-1!Personal|X\\UserSyncRoots\r\n    S-1-5-21-1    REG_SZ    C:\\Users\\me\\OneDrive\r\n\r\n";
    assert_eq!(
        super::check::parse_sync_roots(out),
        vec![PathBuf::from(r"C:\Users\me\OneDrive")]
    );
}

#[test]
fn dropbox_info_json_paths_are_read() {
    let json = r#"{"personal": {"path": "C:\\Users\\me\\Dropbox", "host": 1}, "business": {"path": "D:\\Work Dropbox"}}"#;
    let mut roots = super::check::dropbox_paths(json);
    roots.sort();
    assert_eq!(
        roots,
        vec![
            PathBuf::from(r"C:\Users\me\Dropbox"),
            PathBuf::from(r"D:\Work Dropbox")
        ]
    );
    assert!(super::check::dropbox_paths("not json").is_empty());
}
