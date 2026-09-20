//! Building this computer's account (`SIGNIN-DESIGN.md` §8.26 §2, §8.33).
//!
//! Nothing here reaches the network. The folder tests use a temporary
//! directory; the credential store is only touched by the Windows-only test
//! at the end, which opens it and writes nothing.

use super::*;

/// A valid set of values, in [`SETTING_NAMES`] order. The keys are made-up
/// bytes, not anyone's: 32 bytes of hex.
const PRIMARY_KEY: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const BACKUP_KEY: &str = "2222222222222222222222222222222222222222222222222222222222222222";

fn good() -> [Option<&'static str>; 7] {
    [
        Some("https://accounts.example.com"),
        Some("client_abc123"),
        Some("https://api.example.com"),
        Some("sandbox-p1"),
        Some(PRIMARY_KEY),
        Some("sandbox-b1"),
        Some(BACKUP_KEY),
    ]
}

fn with(index: usize, value: Option<&'static str>) -> [Option<&'static str>; 7] {
    let mut values = good();
    values[index] = value;
    values
}

fn setting_error(result: Result<Option<AccountSettings>, AccountSetupError>) -> String {
    match result {
        Err(AccountSetupError::Setting(name)) => name.to_string(),
        other => panic!("expected a rejected setting, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// The build-time settings
// ---------------------------------------------------------------------------

/// A build with none of them has no sign-in at all. That is what every build
/// made before this arc is, so it must be a plain "not configured", not an
/// error and not a half-configured account.
#[test]
fn a_build_with_no_account_settings_has_no_sign_in() {
    let none: [Option<&str>; 7] = [None; 7];
    assert_eq!(AccountSettings::from_values(none).unwrap(), None);
}

#[test]
fn every_value_is_kept_when_all_of_them_are_valid() {
    let settings = AccountSettings::from_values(good())
        .expect("valid settings")
        .expect("configured");
    assert_eq!(settings.issuer(), "https://accounts.example.com");
    assert_eq!(settings.api_origin(), "https://api.example.com");
}

/// Half-configured is the dangerous one: it could sign in against one
/// instance and verify leases with another instance's key.
#[test]
fn a_build_missing_any_one_setting_is_refused_by_name() {
    for (index, name) in SETTING_NAMES.iter().enumerate() {
        let missing = AccountSettings::from_values(with(index, None));
        assert_eq!(&setting_error(missing), name, "missing {name}");
        let empty = AccountSettings::from_values(with(index, Some("")));
        assert_eq!(&setting_error(empty), name, "empty {name}");
    }
}

/// The issuer is compared to the callback's `iss` by exact string equality
/// (RFC 9207, §8.26 §3), so only the canonical origin form is allowed.
#[test]
fn an_issuer_that_is_not_a_bare_https_origin_is_refused() {
    for bad in [
        "https://accounts.example.com/",
        "https://accounts.example.com/oauth",
        "http://accounts.example.com",
        "https://Accounts.Example.com",
        "accounts.example.com",
    ] {
        let result = AccountSettings::from_values(with(0, Some(bad)));
        assert_eq!(&setting_error(result), SETTING_NAMES[0], "{bad}");
    }
}

#[test]
fn an_api_origin_that_is_not_a_bare_https_origin_is_refused() {
    for bad in ["https://api.example.com/", "http://api.example.com", "api"] {
        let result = AccountSettings::from_values(with(2, Some(bad)));
        assert_eq!(&setting_error(result), SETTING_NAMES[2], "{bad}");
    }
}

#[test]
fn a_client_id_outside_the_allowed_characters_is_refused() {
    for bad in ["client abc", "client/abc", "client.abc"] {
        let result = AccountSettings::from_values(with(1, Some(bad)));
        assert_eq!(&setting_error(result), SETTING_NAMES[1], "{bad}");
    }
}

/// A lease key is exactly 32 bytes, written as 64 hex characters. A shorter
/// one would verify nothing; a longer one is a copy-paste of the wrong thing.
#[test]
fn a_lease_key_must_be_sixty_four_hex_characters() {
    for (index, name) in [(4, SETTING_NAMES[4]), (6, SETTING_NAMES[6])] {
        for bad in [
            "11",
            "111111111111111111111111111111111111111111111111111111111111111",
            "11111111111111111111111111111111111111111111111111111111111111111",
            "zzzz111111111111111111111111111111111111111111111111111111111111",
        ] {
            let result = AccountSettings::from_values(with(index, Some(bad)));
            assert_eq!(&setting_error(result), name, "{name} = {bad}");
        }
    }
}

#[test]
fn a_key_id_outside_the_allowed_characters_is_refused() {
    for (index, name) in [(3, SETTING_NAMES[3]), (5, SETTING_NAMES[5])] {
        for bad in ["Sandbox-P1", "sandbox_p1", "sandbox p1"] {
            let result = AccountSettings::from_values(with(index, Some(bad)));
            assert_eq!(&setting_error(result), name, "{name} = {bad}");
        }
    }
}

/// `LeaseVerifier` refuses two keys that share an id or a key, which would
/// mean the backup could never take over. Catch it at build time instead.
#[test]
fn the_backup_key_must_differ_from_the_primary() {
    let same_kid = AccountSettings::from_values(with(5, Some("sandbox-p1")));
    assert_eq!(&setting_error(same_kid), SETTING_NAMES[5]);
    let same_key = AccountSettings::from_values(with(6, Some(PRIMARY_KEY)));
    assert_eq!(&setting_error(same_key), SETTING_NAMES[6]);
}

// ---------------------------------------------------------------------------
// The account folder
// ---------------------------------------------------------------------------

#[test]
fn the_account_folder_sits_inside_the_local_app_data_folder() {
    let path = account_dir_path(Path::new("C:\\Users\\sam\\AppData\\Local\\com.zaaheen.app"));
    assert!(path.ends_with(ACCOUNT_DIR_NAME));
}

/// §8.27: `AccountDir::open` refuses a folder that is not there, so this
/// creates it first.
#[test]
fn the_account_folder_is_created_when_it_is_missing() {
    let home = tempfile::tempdir().expect("a temporary folder");
    let dir = open_account_dir(home.path()).expect("the folder is prepared");
    let path = account_dir_path(home.path());
    assert!(path.is_dir(), "the account folder was not created");
    drop(dir);
}

#[test]
fn preparing_the_account_folder_twice_changes_nothing() {
    let home = tempfile::tempdir().expect("a temporary folder");
    let _first = open_account_dir(home.path()).expect("first");
    let _second = open_account_dir(home.path()).expect("second");
    assert!(account_dir_path(home.path()).is_dir());
}

/// ADR-SEC-019: the folder is restricted to its owner before anything is
/// written into it. The marker is what `harden_vault_dir` leaves behind.
#[cfg(windows)]
#[test]
fn the_account_folder_is_restricted_to_its_owner() {
    let home = tempfile::tempdir().expect("a temporary folder");
    let _dir = open_account_dir(home.path()).expect("the folder is prepared");
    let marker = account_dir_path(home.path()).join(crate::keeper::acl::ACL_MARKER);
    assert!(
        marker.exists(),
        "the account folder was not restricted to its owner"
    );
}

// ---------------------------------------------------------------------------
// The account itself
// ---------------------------------------------------------------------------

/// Windows only: this opens Credential Manager (and writes nothing). V0.2
/// ships Windows only, as the vault key does.
#[cfg(windows)]
#[test]
fn the_account_is_built_from_the_settings_and_the_folder() {
    let home = tempfile::tempdir().expect("a temporary folder");
    let dir = open_account_dir(home.path()).expect("the folder is prepared");
    let settings = AccountSettings::from_values(good())
        .expect("valid settings")
        .expect("configured");
    let account = build_account(&settings, dir).expect("the account is built");
    // Nobody is signed in in a fresh folder, and reading that touches no
    // network and no store.
    let runtime = tokio::runtime::Runtime::new().expect("a runtime");
    let status = runtime
        .block_on(vault_account::Account::status(&account, 1_700_000_000))
        .expect("status reads");
    assert_eq!(status, vault_account::Status::SignedOut);
}

// ---------------------------------------------------------------------------
// The sign-in marker, read the way a relay must read it (§8.26 §6.3, §8.35)
//
// A relay never writes in the account folder, so it must not create or harden
// one either. And it may only short-circuit when it KNOWS nobody is signed in:
// a folder it could not read is not an empty one, and telling somebody to sign
// in when the real answer is "could not confirm" would be the wrong message.
// ---------------------------------------------------------------------------

#[test]
fn a_marker_means_somebody_is_signed_in() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = open_account_dir(tmp.path()).unwrap();
    let lock = dir
        .lock(std::time::Duration::from_secs(1))
        .unwrap()
        .expect("the lock is free");
    dir.write_marker(&lock, "user_2abc").unwrap();
    assert_eq!(sign_in_marker(&account_dir_path(tmp.path())), Some(true));
}

#[test]
fn an_account_folder_with_no_marker_is_a_signed_out_computer() {
    let tmp = tempfile::TempDir::new().unwrap();
    let _dir = open_account_dir(tmp.path()).unwrap();
    assert_eq!(sign_in_marker(&account_dir_path(tmp.path())), Some(false));
}

/// The commonest signed-out case by far: a fresh install where nobody has ever
/// signed in, so the folder does not exist yet. A definite answer — and
/// reading it must not bring the folder into being.
#[test]
fn a_missing_account_folder_is_a_signed_out_computer() {
    let tmp = tempfile::TempDir::new().unwrap();
    let never_created = account_dir_path(tmp.path());
    assert!(!never_created.exists());
    assert_eq!(sign_in_marker(&never_created), Some(false));
    assert!(
        !never_created.exists(),
        "reading the marker must not create the account folder"
    );
}

/// A path that is not a folder cannot hold a marker, so nobody is signed in
/// there either. `AccountDir::open` reports this as `NotFound`, which is the
/// right reading: there is no account folder.
#[test]
fn a_path_that_is_not_a_folder_is_a_signed_out_computer() {
    let tmp = tempfile::TempDir::new().unwrap();
    let in_the_way = account_dir_path(tmp.path());
    std::fs::write(&in_the_way, b"not a folder").unwrap();
    assert_eq!(sign_in_marker(&in_the_way), Some(false));
}

/// The branch that keeps the message honest. Split from the I/O so it can be
/// proven without an unreadable folder, which no portable test can make.
#[test]
fn a_folder_it_could_not_read_leaves_the_decision_to_the_keeper() {
    let denied = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
    assert_eq!(
        marker_verdict(Err(denied)),
        None,
        "an unreadable account folder must not be reported as signed out"
    );
    // And the two answers it may give.
    assert_eq!(marker_verdict(Ok(Some("user_2abc".into()))), Some(true));
    assert_eq!(marker_verdict(Ok(None)), Some(false));
    assert_eq!(
        marker_verdict(Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no folder"
        ))),
        Some(false)
    );
}
