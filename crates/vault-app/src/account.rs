//! Building this computer's Zaaheen account (ADR-104 + ADR-SEC-022;
//! `SIGNIN-DESIGN.md` §8.26 §2 and §8.33).
//!
//! `vault-account` holds the sign-in, the lease and the local record but
//! takes every account-specific value as an argument. This module supplies
//! them, and it is the only place that knows where they come from.
//!
//! **Where the values come from: the build environment, never the repo.**
//! The issuer, the OAuth client id and the account service's origin are
//! account identifiers, and this repository is public (§8.26's header). The
//! lease public keys are not secret, but a build must not mix the sandbox
//! pair with the production one (§8.31), so they travel the same way. Each is
//! read with `option_env!`, which bakes the value in at compile time; a build
//! made without them has no sign-in at all rather than a half-configured one.
//!
//! **The account folder** (`%LOCALAPPDATA%\com.zaaheen.app\account`) is
//! created here and restricted to its owner before anything is written to it
//! (ADR-SEC-019, and §8.27: "`AccountDir::open` refuses a folder that does
//! not exist").

#[cfg(test)]
mod account_tests;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use vault_account::{
    Account, AccountConfig, AccountDir, AccountTimings, CheckoutClient, LeaseClient, LeaseKey,
    LeaseVerifier, OAuthClient, TokenStore,
};
use vault_mcp::{Gate, InFlight};

use crate::entitlement::{AccountCheck, SystemClock};
use crate::keeper::acl;

/// The folder inside the local app data directory that holds the account's
/// four files (§8.26 §4).
pub const ACCOUNT_DIR_NAME: &str = "account";

/// The build-time settings, in the order [`AccountSettings::from_values`]
/// takes them. Used for the error messages, so a broken build says which one
/// is wrong.
pub const SETTING_NAMES: [&str; 7] = [
    "ZAAHEEN_ACCOUNT_ISSUER",
    "ZAAHEEN_ACCOUNT_CLIENT_ID",
    "ZAAHEEN_ACCOUNT_API",
    "ZAAHEEN_LEASE_PRIMARY_KID",
    "ZAAHEEN_LEASE_PRIMARY_KEY",
    "ZAAHEEN_LEASE_BACKUP_KID",
    "ZAAHEEN_LEASE_BACKUP_KEY",
];

/// A client id that passes `vault-account`'s rules, used when only the origin
/// beside it is being checked.
const PLACEHOLDER_CLIENT_ID: &str = "placeholder";

/// The error for the setting at `index` in [`SETTING_NAMES`].
fn setting(index: usize) -> AccountSetupError {
    AccountSetupError::Setting(SETTING_NAMES[index])
}

/// The seven values this build was compiled with, in [`SETTING_NAMES`] order.
/// `option_env!` is resolved at compile time, so this reads nothing at run
/// time. One definition, so the names cannot drift apart.
fn build_env_values() -> [Option<&'static str>; 7] {
    [
        option_env!("ZAAHEEN_ACCOUNT_ISSUER"),
        option_env!("ZAAHEEN_ACCOUNT_CLIENT_ID"),
        option_env!("ZAAHEEN_ACCOUNT_API"),
        option_env!("ZAAHEEN_LEASE_PRIMARY_KID"),
        option_env!("ZAAHEEN_LEASE_PRIMARY_KEY"),
        option_env!("ZAAHEEN_LEASE_BACKUP_KID"),
        option_env!("ZAAHEEN_LEASE_BACKUP_KEY"),
    ]
}

/// Whether this build carries a sign-in at all, without validating anything or
/// touching the disk.
///
/// The relay asks this (§8.26 §6.3): it must not fail on a build with a broken
/// setting — only the keeper refuses to start for that — and nothing that can
/// fail may run before it serves stdio.
pub fn has_account_settings() -> bool {
    build_env_values().iter().any(Option::is_some)
}

/// One key: its id at `kid_index` and its 64 hex characters at `kid_index+1`.
fn lease_key(kid: &str, key: &str, kid_index: usize) -> Result<LeaseKeySetting, AccountSetupError> {
    LeaseKey::new(kid, [0u8; 32]).map_err(|_| setting(kid_index))?;
    let bytes = hex::decode(key).map_err(|_| setting(kid_index + 1))?;
    let public: [u8; 32] = bytes.try_into().map_err(|_| setting(kid_index + 1))?;
    Ok(LeaseKeySetting {
        kid: kid.to_owned(),
        public,
    })
}

/// Why the account could not be built.
#[derive(Debug, thiserror::Error)]
pub enum AccountSetupError {
    /// A value baked in at build time is malformed. Names the field, never
    /// the value.
    #[error("the account setting `{0}` is not valid")]
    Setting(&'static str),
    /// The account folder could not be created or restricted.
    #[error("the account folder could not be prepared: {0}")]
    Folder(#[from] std::io::Error),
    /// `vault-account` refused something (config, keys, the credential store).
    #[error(transparent)]
    Account(#[from] vault_account::AccountError),
}

/// Everything account-specific a build carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountSettings {
    issuer: String,
    client_id: String,
    api_origin: String,
    primary: LeaseKeySetting,
    backup: LeaseKeySetting,
}

/// One lease public key: its id and its 32 bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
struct LeaseKeySetting {
    kid: String,
    public: [u8; 32],
}

impl AccountSettings {
    /// The values baked in at build time, or `None` when this build carries
    /// none (then the app has no sign-in; see the module docs).
    ///
    /// # Errors
    ///
    /// [`AccountSetupError::Setting`] when a value is present but malformed:
    /// a build that carries a broken setting must fail loudly, not sign in
    /// against the wrong instance.
    pub fn from_build_env() -> Result<Option<Self>, AccountSetupError> {
        Self::from_values(build_env_values())
    }

    /// The seven values, in [`SETTING_NAMES`] order. All absent means this
    /// build has no sign-in; anything else must be complete and valid, so a
    /// half-configured build fails loudly instead of signing in against the
    /// wrong instance.
    ///
    /// # Errors
    ///
    /// [`AccountSetupError::Setting`], naming the field that failed. The
    /// value itself is never in the message.
    fn from_values(values: [Option<&str>; 7]) -> Result<Option<Self>, AccountSetupError> {
        if values.iter().all(Option::is_none) {
            return Ok(None);
        }
        let mut given = [""; 7];
        for (i, value) in values.iter().enumerate() {
            let value = value.unwrap_or_default();
            if value.is_empty() {
                return Err(setting(i));
            }
            given[i] = value;
        }

        // The issuer and the client id are checked by `vault-account` itself,
        // so there is one definition of valid. The issuer is checked alone
        // first, to name the right field.
        let (issuer, client_id, api_origin) = (given[0], given[1], given[2]);
        AccountConfig::new(issuer, PLACEHOLDER_CLIENT_ID).map_err(|_| setting(0))?;
        AccountConfig::new(issuer, client_id).map_err(|_| setting(1))?;
        // The account service's origin follows the same rules as the issuer.
        AccountConfig::new(api_origin, PLACEHOLDER_CLIENT_ID).map_err(|_| setting(2))?;

        let primary = lease_key(given[3], given[4], 3)?;
        let backup = lease_key(given[5], given[6], 5)?;
        // What `LeaseVerifier::new` would refuse later, refused here, where
        // the message can name the setting that is wrong.
        if primary.kid == backup.kid {
            return Err(setting(5));
        }
        if primary.public == backup.public {
            return Err(setting(6));
        }

        Ok(Some(Self {
            issuer: issuer.to_owned(),
            client_id: client_id.to_owned(),
            api_origin: api_origin.to_owned(),
            primary,
            backup,
        }))
    }

    /// The account service's origin, e.g. `https://api.zaaheen.com`.
    pub fn api_origin(&self) -> &str {
        &self.api_origin
    }

    /// The issuer this build trusts.
    pub fn issuer(&self) -> &str {
        &self.issuer
    }
}

/// The account folder for `local_app_data`, created and restricted to its
/// owner if it was not there already.
///
/// # Errors
///
/// [`AccountSetupError::Folder`] when it cannot be created. A failure to
/// restrict it is logged, not fatal: the same posture as the vault folder
/// (the files inside are useless without the account's own token store).
pub fn open_account_dir(local_app_data: &Path) -> Result<AccountDir, AccountSetupError> {
    let path = account_dir_path(local_app_data);
    std::fs::create_dir_all(&path)?;
    // Before anything is written into it (ADR-SEC-019). A failure here is
    // logged, not fatal: the same posture the vault folder takes.
    if let Err(e) = acl::harden_vault_dir(&path) {
        tracing::warn!(error = %e, "the account folder's permissions were not tightened");
    }
    Ok(AccountDir::open(path)?)
}

/// The account itself, over this computer's folder and credential store.
///
/// # Errors
///
/// [`AccountSetupError::Account`] when `vault-account` refuses a value or the
/// credential store cannot be opened.
pub fn build_account(
    settings: &AccountSettings,
    dir: AccountDir,
) -> Result<Account, AccountSetupError> {
    let config = AccountConfig::new(&settings.issuer, &settings.client_id)?;
    let oauth = OAuthClient::new(config)?;
    let api = LeaseClient::new(&settings.api_origin, env!("CARGO_PKG_VERSION"))?;
    let verifier = LeaseVerifier::new(
        LeaseKey::new(&settings.primary.kid, settings.primary.public)?,
        LeaseKey::new(&settings.backup.kid, settings.backup.public)?,
    )?;
    let store = TokenStore::platform()?;
    // The checkout client lives on `Account` so that `/v1/checkout` is called
    // from inside the type that owns the token lifecycle: an access token is
    // produced only by rotating the refresh token under `refresh.lock`, and a
    // second way to get one would either duplicate those rules or bypass them
    // (S3 step 4b).
    let checkout = CheckoutClient::new(&settings.api_origin)?;
    Ok(
        Account::new(dir, store, oauth, api, verifier, AccountTimings::default())
            .with_checkout(checkout),
    )
}

/// Everything the desktop needs to run the account: sign in, sign out, check
/// the state, subscribe, refresh.
///
/// `None` when this build carries no account settings, exactly as
/// [`build_check`] does — then the desktop shows no account panel because
/// there is no account.
///
/// # Errors
///
/// As [`build_gate`]: broken build-time settings, a folder that cannot be
/// prepared, or a credential store that cannot be opened.
pub fn build_account_ops(
    local_app_data: &Path,
) -> Result<Option<crate::account_ops::AccountOps>, AccountSetupError> {
    let Some(settings) = AccountSettings::from_build_env()? else {
        return Ok(None);
    };
    let dir = open_account_dir(local_app_data)?;
    let config = AccountConfig::new(&settings.issuer, &settings.client_id)?;
    let account = build_account(&settings, dir)?;
    Ok(Some(crate::account_ops::AccountOps::new(
        Arc::new(account),
        config,
        Arc::new(SystemClock),
    )))
}

/// Where the account folder lives under `local_app_data`.
pub fn account_dir_path(local_app_data: &Path) -> PathBuf {
    local_app_data.join(ACCOUNT_DIR_NAME)
}

/// Whether somebody is signed in on this computer, for a reader that must not
/// touch the folder (§8.26 §6.3): `Some(false)` only when nobody is.
///
/// A relay uses this to answer the sign-in message itself rather than start a
/// keeper that could only say the same thing. It never creates or hardens the
/// folder — only the keeper and the desktop write here (§8.26 §4) — and a
/// folder it could not read gives `None`, because telling somebody to sign in
/// when the true answer is "could not confirm" would be the wrong message.
pub fn sign_in_marker(account_dir: &Path) -> Option<bool> {
    marker_verdict(AccountDir::open(account_dir).and_then(|dir| dir.read_marker()))
}

/// The decision, given whatever the folder said. Split from the I/O above so
/// the "could not read it" branch can be proven without an unreadable folder,
/// which no portable test can make.
fn marker_verdict(read: std::io::Result<Option<String>>) -> Option<bool> {
    match read {
        Ok(marker) => Some(marker.is_some()),
        // No folder at all, or a path that is not one: nobody has ever signed
        // in on this computer, which is a definite answer.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(false),
        Err(_) => None,
    }
}

/// The subscription gate for this build, or `None` when the build carries no
/// account settings — then the vault serves as it did before this arc.
///
/// One gate per process: its `InFlight` counts every server the keeper makes,
/// which is what a mode change waits on (§8.26 §6.2).
///
/// # Errors
///
/// A build that carries broken settings, or a folder that cannot be prepared,
/// or a credential store that cannot be opened. The caller decides whether to
/// refuse to start or to serve ungated; the keeper refuses, because a gate
/// that silently disappears is worse than a keeper that says why.
pub fn build_gate(local_app_data: &Path) -> Result<Option<Gate>, AccountSetupError> {
    Ok(build_check(local_app_data)?.map(|check| Gate::new(check, InFlight::new())))
}

/// The check itself, for a caller that needs more than a gate.
///
/// The keeper does: it asks the check which mode to start in before it opens
/// the vault, and the same check then both serves calls and answers each tick
/// (`Subscription`, §8.35). `None` means this build carries no account
/// settings, so there is no sign-in at all.
///
/// # Errors
///
/// As [`build_gate`]: broken build-time settings, a folder that cannot be
/// prepared, or a credential store that cannot be opened.
pub fn build_check(local_app_data: &Path) -> Result<Option<Arc<AccountCheck>>, AccountSetupError> {
    let Some(settings) = AccountSettings::from_build_env()? else {
        tracing::info!("this build carries no account settings; the vault serves ungated");
        return Ok(None);
    };
    let dir = open_account_dir(local_app_data)?;
    let account = build_account(&settings, dir)?;
    tracing::info!(
        issuer = settings.issuer(),
        api = settings.api_origin(),
        "the subscription gate is on"
    );
    Ok(Some(Arc::new(AccountCheck::new(
        Arc::new(account),
        Arc::new(SystemClock),
    ))))
}
