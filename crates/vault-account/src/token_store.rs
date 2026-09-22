//! Where the refresh token lives: the OS credential store (Windows Credential
//! Manager), through **this crate's own** store instance (§8.26 §2, §3).
//!
//! # Why its own store
//!
//! `keyring-core` keeps one process-global default store, and
//! `vault-app/src/keychain.rs` sets and unsets it around every master-key read
//! (§8.26 §1, v2 §1). A second user of that global would race it. So this
//! module builds its entries directly from an `Arc<CredentialStore>` it owns
//! (`CredentialStoreApi::build`) and never reads or writes the global.
//!
//! # Where the token goes (§8.26 §3)
//!
//! - Service `com.zaaheen.account`, user `refresh-token`, so on Windows the
//!   credential is `refresh-token.com.zaaheen.account`, well apart from the
//!   vault key's `default.com.zaaheen.v0.2`. Sign-in never touches the vault
//!   key.
//! - **`persistence=Local`**: the Windows store otherwise defaults to
//!   Enterprise, which roams the credential to other machines with the user's
//!   profile. The token stays on this computer.
//!
//! # The signed-in address (ADR-SEC-027)
//!
//! A second credential beside the token, user `signed-in-email`, holds the
//! signed-in `sub` and address as `<sub>\n<email>`, so "Signed in as
//! <email>" still has an address on the next app open. Same store, same Local
//! persistence, encrypted by the OS like the token (BRD §11.5.1: no plaintext
//! on disk). It is only ever read back for the `sub` that is signed in now,
//! and only if it still passes the checks it passed on the way in.
//!
//! # Failure rules (§8.26 §3, §4)
//!
//! - No credential → `Ok(None)`: nothing stored.
//! - Any credential-store failure → [`AccountError::Keychain`], which is
//!   **transient**. A locked or busy store never reads as "signed out".
//! - A stored value that is not a token we could have written (not UTF-8,
//!   or breaking the token rules) → `Ok(None)` with a warning. The store
//!   itself worked; there is simply no usable token, and signing in again
//!   overwrites it.
//! - Deleting a credential that is not there succeeds, so sign-out can always
//!   "make sure it is gone".
//!
//! The calls are short synchronous Win32 calls; async callers run them under
//! `tokio::task::spawn_blocking` (BRD §2.8), which is why [`TokenStore`] is
//! cheap to clone.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use keyring_core::{CredentialStore, Entry, Error};
use zeroize::{Zeroize, Zeroizing};

use crate::error::{AccountError, AccountResult};
use crate::oauth::{RefreshToken, UserInfo};

/// Credential service name. Only the Windows store and the tests name it;
/// elsewhere nothing would use it (V0.2's store is Windows-only).
#[cfg(any(windows, test))]
pub const SERVICE: &str = "com.zaaheen.account";

/// Credential user name.
pub const USER: &str = "refresh-token";

/// Credential user name for the signed-in address (ADR-SEC-027).
pub const ADDRESS_USER: &str = "signed-in-email";

/// The refresh token's home in the OS credential store.
#[derive(Clone)]
pub struct TokenStore {
    store: Arc<CredentialStore>,
    persistence: Option<&'static str>,
    service: String,
}

impl TokenStore {
    /// The production store: Windows Credential Manager, Local persistence.
    ///
    /// # Errors
    ///
    /// [`AccountError::Keychain`] if the store cannot be opened;
    /// [`AccountError::InvalidConfig`] on platforms without a supported store
    /// (V0.2 is Windows-only, as for the vault key).
    pub fn platform() -> AccountResult<Self> {
        #[cfg(windows)]
        {
            Self::windows_local_named(SERVICE.to_owned())
        }
        #[cfg(not(windows))]
        {
            Err(AccountError::InvalidConfig(format!(
                "the account token store is Windows-only in this version (current platform: {})",
                std::env::consts::OS
            )))
        }
    }

    /// Windows Credential Manager under `service`, Local persistence.
    #[cfg(windows)]
    fn windows_local_named(service: String) -> AccountResult<Self> {
        let store = windows_native_keyring_store::Store::new().map_err(keychain)?;
        Ok(Self {
            store,
            persistence: Some("Local"),
            service,
        })
    }

    /// A store backed by `store`, with no creation modifiers. For tests,
    /// against `keyring_core::mock`, which accepts no modifiers.
    #[cfg(test)]
    pub(crate) fn with_store(store: Arc<CredentialStore>) -> Self {
        Self {
            store,
            persistence: None,
            service: SERVICE.to_owned(),
        }
    }

    /// Read the stored refresh token.
    ///
    /// # Errors
    ///
    /// [`AccountError::Keychain`] (transient) if the store fails.
    pub fn load(&self) -> AccountResult<Option<RefreshToken>> {
        match self.entry()?.get_password() {
            Ok(stored) => {
                let token = RefreshToken::from_stored(Zeroizing::new(stored));
                if token.is_none() {
                    tracing::warn!("stored refresh token is malformed; treating it as absent");
                }
                Ok(token)
            }
            Err(Error::NoEntry) => Ok(None),
            Err(Error::BadEncoding(mut raw) | Error::BadDataFormat(mut raw, _)) => {
                raw.zeroize();
                tracing::warn!("stored refresh token is not text; treating it as absent");
                Ok(None)
            }
            Err(e) => Err(keychain(e)),
        }
    }

    /// Store `token`, replacing any previous one. `Ok` means the credential
    /// store accepted the write; §8.26 §15 needs that before the old token
    /// is considered spent.
    ///
    /// # Errors
    ///
    /// [`AccountError::Keychain`] (transient) if the store fails. The
    /// previously stored token is then still in place.
    pub fn save(&self, token: &RefreshToken) -> AccountResult<()> {
        self.entry()?.set_password(token.expose()).map_err(keychain)
    }

    /// Remove the stored token. Removing a token that is not there succeeds.
    ///
    /// # Errors
    ///
    /// [`AccountError::Keychain`] (transient) if the store fails.
    pub fn delete(&self) -> AccountResult<()> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(Error::NoEntry) => Ok(()),
            Err(e) => Err(keychain(e)),
        }
    }

    /// Remember who signed in, for "Signed in as <email>" on a later open
    /// (ADR-SEC-027): one credential holding the `sub` and the address, so an
    /// address can never be shown under somebody else's sign-in.
    ///
    /// # Errors
    ///
    /// [`AccountError::Keychain`] (transient) if the store fails. The caller
    /// logs it: an address is a label, and a sign-in never fails over one.
    pub fn save_address(&self, user: &UserInfo) -> AccountResult<()> {
        // `sub` is printable ASCII and the address has no control characters
        // (`UserInfo::checked`), so the newline between them is unambiguous.
        self.entry_for(ADDRESS_USER)?
            .set_password(&format!("{}\n{}", user.sub, user.email))
            .map_err(keychain)
    }

    /// The remembered address, when it belongs to `sub` and still passes the
    /// checks it passed on the way in; otherwise `None`.
    ///
    /// # Errors
    ///
    /// [`AccountError::Keychain`] (transient) if the store fails.
    pub fn load_address(&self, sub: &str) -> AccountResult<Option<String>> {
        match self.entry_for(ADDRESS_USER)?.get_password() {
            Ok(stored) => {
                let address = stored
                    .split_once('\n')
                    .and_then(|(s, e)| UserInfo::checked(s.to_owned(), e.to_owned()))
                    .filter(|who| who.sub == sub)
                    .map(|who| who.email);
                Ok(address)
            }
            Err(Error::NoEntry) => Ok(None),
            // Not text: nothing we could have written. The store worked, so
            // this is "no address", not a failure.
            Err(Error::BadEncoding(_) | Error::BadDataFormat(..)) => Ok(None),
            Err(e) => Err(keychain(e)),
        }
    }

    /// Forget the remembered address. Forgetting one that is not there
    /// succeeds.
    ///
    /// # Errors
    ///
    /// [`AccountError::Keychain`] (transient) if the store fails.
    pub fn delete_address(&self) -> AccountResult<()> {
        match self.entry_for(ADDRESS_USER)?.delete_credential() {
            Ok(()) | Err(Error::NoEntry) => Ok(()),
            Err(e) => Err(keychain(e)),
        }
    }

    /// Build our token entry from this store instance.
    fn entry(&self) -> AccountResult<Entry> {
        self.entry_for(USER)
    }

    /// Build the entry for `user` from this store instance (never the global
    /// default), with the Local-persistence modifier when the store takes
    /// one.
    fn entry_for(&self, user: &str) -> AccountResult<Entry> {
        let modifiers = self
            .persistence
            .map(|p| HashMap::from([("persistence", p)]));
        self.store
            .build(&self.service, user, modifiers.as_ref())
            .map_err(keychain)
    }
}

/// A credential-store failure. keyring-core's messages name the failing
/// operation or attribute, never a stored value; the two variants that carry
/// stored bytes are handled in [`TokenStore::load`] and never reach here.
fn keychain(e: Error) -> AccountError {
    AccountError::Keychain(match e {
        Error::BadEncoding(_) | Error::BadDataFormat(..) => "stored value is not text".into(),
        other => other.to_string(),
    })
}

impl fmt::Debug for TokenStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenStore")
            .field("service", &self.service)
            .field("persistence", &self.persistence)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    // `build` on the concrete mock store needs the trait in scope; on
    // `Arc<CredentialStore>` (a trait object) it does not.
    use keyring_core::api::CredentialStoreApi;
    use keyring_core::mock;

    use super::*;

    fn token(value: &str) -> RefreshToken {
        RefreshToken::from_stored(Zeroizing::new(value.into())).unwrap()
    }

    fn loaded(store: &TokenStore) -> Option<String> {
        store.load().unwrap().map(|t| t.expose().to_owned())
    }

    /// A token store over a fresh mock, plus the mock for injecting errors.
    fn fresh() -> (TokenStore, Arc<mock::Store>) {
        let mock = mock::Store::new().unwrap();
        let store: Arc<CredentialStore> = mock.clone();
        (TokenStore::with_store(store), mock)
    }

    /// Make the next call on our credential fail with `err`.
    fn fail_next(mock: &Arc<mock::Store>, err: Error) {
        let entry = mock.build(SERVICE, USER, None).unwrap();
        let cred: &mock::Cred = entry.as_any().downcast_ref().unwrap();
        cred.set_error(err);
    }

    /// Put a raw value where the token lives, bypassing the token rules.
    fn plant(mock: &Arc<mock::Store>, raw: &str) {
        mock.build(SERVICE, USER, None)
            .unwrap()
            .set_password(raw)
            .unwrap();
    }

    fn platform_error(msg: &str) -> keyring_core::error::PlatformError {
        Box::new(std::io::Error::other(msg.to_owned()))
    }

    // ---- the basic contract ---------------------------------------------------------

    #[test]
    fn an_empty_store_holds_no_token() {
        let (store, _) = fresh();
        assert_eq!(loaded(&store), None);
    }

    #[test]
    fn a_saved_token_loads_back() {
        let (store, _) = fresh();
        store.save(&token("rt_first_1234")).unwrap();
        assert_eq!(loaded(&store).as_deref(), Some("rt_first_1234"));
    }

    #[test]
    fn saving_replaces_the_previous_token() {
        let (store, _) = fresh();
        store.save(&token("rt_old")).unwrap();
        store.save(&token("rt_rotated")).unwrap();
        assert_eq!(loaded(&store).as_deref(), Some("rt_rotated"));
    }

    #[test]
    fn delete_removes_the_token_and_is_idempotent() {
        let (store, _) = fresh();
        store.save(&token("rt_1")).unwrap();
        store.delete().unwrap();
        assert_eq!(loaded(&store), None);
        store.delete().unwrap();
    }

    #[test]
    fn clones_share_the_same_credential() {
        let (store, _) = fresh();
        let other = store.clone();
        store.save(&token("rt_shared")).unwrap();
        assert_eq!(loaded(&other).as_deref(), Some("rt_shared"));
    }

    // ---- a failing store is never "signed out" ---------------------------------------

    #[test]
    fn a_failing_store_is_a_transient_error_not_an_empty_one() {
        for err in [
            Error::PlatformFailure(platform_error("store busy")),
            Error::NoStorageAccess(platform_error("store locked")),
        ] {
            let (store, mock) = fresh();
            store.save(&token("rt_kept")).unwrap();
            fail_next(&mock, err);
            let got = store.load();
            assert!(
                matches!(&got, Err(AccountError::Keychain(_))),
                "{:?}",
                got.map(|t| t.is_some())
            );
            assert!(got.err().is_some_and(|e| e.is_transient()));
            // The token is still there once the store recovers.
            assert_eq!(loaded(&store).as_deref(), Some("rt_kept"));
        }
    }

    #[test]
    fn a_failed_save_keeps_the_previous_token() {
        let (store, mock) = fresh();
        store.save(&token("rt_previous")).unwrap();
        fail_next(
            &mock,
            Error::PlatformFailure(platform_error("write failed")),
        );
        let err = store.save(&token("rt_new")).unwrap_err();
        assert!(matches!(err, AccountError::Keychain(_)), "{err:?}");
        assert_eq!(loaded(&store).as_deref(), Some("rt_previous"));
    }

    #[test]
    fn a_failed_delete_is_reported_not_swallowed() {
        let (store, mock) = fresh();
        store.save(&token("rt_1")).unwrap();
        fail_next(
            &mock,
            Error::PlatformFailure(platform_error("delete failed")),
        );
        assert!(matches!(store.delete(), Err(AccountError::Keychain(_))));
        assert_eq!(loaded(&store).as_deref(), Some("rt_1"));
    }

    #[test]
    fn keychain_errors_never_carry_the_token() {
        let (store, mock) = fresh();
        fail_next(&mock, Error::TooLong("password".into(), 2560));
        let err = store.save(&token("rt_SECRET_value")).unwrap_err();
        assert!(!err.to_string().contains("rt_SECRET_value"));
        assert!(!format!("{err:?}").contains("rt_SECRET_value"));
    }

    // ---- a stored value that is not a token ------------------------------------------

    #[test]
    fn a_stored_value_breaking_the_token_rules_reads_as_no_token() {
        for raw in ["", "has spaces", "line\nbreak", "tökén"] {
            let (store, mock) = fresh();
            plant(&mock, raw);
            assert_eq!(loaded(&store), None, "{raw:?}");
        }
    }

    #[test]
    fn a_stored_value_that_is_not_utf8_reads_as_no_token() {
        let (store, mock) = fresh();
        store.save(&token("rt_1")).unwrap();
        fail_next(&mock, Error::BadEncoding(vec![0xff, 0xfe, 0x00]));
        assert_eq!(loaded(&store), None);
    }

    // ---- isolation from the vault key -------------------------------------------------

    #[test]
    fn the_process_global_default_store_is_never_used() {
        // Nothing in this crate sets the global, so any use of it would fail
        // with NoDefaultStore; these calls succeeding proves the direct path.
        assert!(keyring_core::get_default_store().is_none());
        let (store, _) = fresh();
        store.save(&token("rt_direct")).unwrap();
        assert_eq!(loaded(&store).as_deref(), Some("rt_direct"));
        store.delete().unwrap();
        assert!(keyring_core::get_default_store().is_none());
    }

    #[test]
    fn the_credential_name_is_not_the_vault_keys() {
        // vault-app keeps the master key at service "com.zaaheen.v0.2", user
        // "default" (keychain.rs PRODUCTION_NAMESPACE / VAULT_ID).
        assert_ne!(format!("{USER}.{SERVICE}"), "default.com.zaaheen.v0.2");
        assert_ne!(SERVICE, "com.zaaheen.v0.2");
    }

    #[test]
    fn debug_shows_no_secret() {
        let (store, _) = fresh();
        store.save(&token("rt_SECRET_value")).unwrap();
        assert!(!format!("{store:?}").contains("rt_SECRET_value"));
    }

    // ---- the signed-in address (ADR-SEC-027) ------------------------------------------

    fn person(sub: &str, email: &str) -> UserInfo {
        UserInfo::checked(sub.into(), email.into()).expect("a valid test identity")
    }

    /// What sits in the address credential, bypassing every rule.
    fn raw_address(mock: &Arc<mock::Store>) -> Option<String> {
        mock.build(SERVICE, ADDRESS_USER, None)
            .unwrap()
            .get_password()
            .ok()
    }

    #[test]
    fn an_address_is_read_back_for_the_person_it_was_stored_for() {
        let (store, mock) = fresh();
        store
            .save_address(&person("user_1", "sam@example.com"))
            .unwrap();
        assert_eq!(
            store.load_address("user_1").unwrap().as_deref(),
            Some("sam@example.com")
        );
        assert_eq!(
            raw_address(&mock).as_deref(),
            Some("user_1\nsam@example.com"),
            "one credential holding the sub and the address"
        );
    }

    /// The whole point of storing the `sub` with it.
    #[test]
    fn an_address_is_never_read_back_for_somebody_else() {
        let (store, _) = fresh();
        store
            .save_address(&person("user_1", "sam@example.com"))
            .unwrap();
        assert_eq!(store.load_address("user_2").unwrap(), None);
        assert_eq!(store.load_address("").unwrap(), None);
    }

    #[test]
    fn the_address_and_the_token_are_separate_credentials() {
        let (store, _) = fresh();
        store.save(&token("rt_1")).unwrap();
        store
            .save_address(&person("user_1", "sam@example.com"))
            .unwrap();

        store.delete().unwrap();
        assert_eq!(
            store.load_address("user_1").unwrap().as_deref(),
            Some("sam@example.com"),
            "deleting the token must not take the address with it"
        );

        store.save(&token("rt_2")).unwrap();
        store.delete_address().unwrap();
        assert_eq!(loaded(&store).as_deref(), Some("rt_2"));
        assert_eq!(store.load_address("user_1").unwrap(), None);
    }

    #[test]
    fn forgetting_the_address_is_idempotent() {
        let (store, mock) = fresh();
        store
            .save_address(&person("user_1", "sam@example.com"))
            .unwrap();
        store.delete_address().unwrap();
        assert_eq!(raw_address(&mock), None);
        store.delete_address().unwrap();
    }

    /// Whatever is in the credential is held to the rules it passed on the
    /// way in. A value we could not have written is no address at all.
    #[test]
    fn a_damaged_address_is_no_address() {
        let long = format!("user_1\n{}@example.com", "a".repeat(400));
        for raw in [
            "",
            "user_1",
            "user_1\n",
            "\nsam@example.com",
            "user_1\nsam@exa\u{7}mple.com",
            "user_1\nsam@example.com\nsecond line",
            "user 1\nsam@example.com",
            long.as_str(),
        ] {
            let (store, mock) = fresh();
            mock.build(SERVICE, ADDRESS_USER, None)
                .unwrap()
                .set_password(raw)
                .unwrap();
            let sub = raw.split('\n').next().unwrap_or_default();
            assert_eq!(
                store.load_address(sub).unwrap(),
                None,
                "a damaged value was read back as an address (case {})",
                raw.len()
            );
        }
    }

    /// Make the next call on the address credential fail with `err`.
    fn fail_next_address(mock: &Arc<mock::Store>, err: Error) {
        let entry = mock.build(SERVICE, ADDRESS_USER, None).unwrap();
        let cred: &mock::Cred = entry.as_any().downcast_ref().unwrap();
        cred.set_error(err);
    }

    #[test]
    fn an_address_that_is_not_text_is_no_address() {
        let (store, mock) = fresh();
        fail_next_address(&mock, Error::BadEncoding(vec![0xff, 0xfe]));
        assert_eq!(store.load_address("user_1").unwrap(), None);
    }

    #[test]
    fn a_failing_store_is_reported_not_read_as_no_address() {
        let (store, mock) = fresh();
        store
            .save_address(&person("user_1", "sam@example.com"))
            .unwrap();
        fail_next_address(&mock, Error::PlatformFailure(platform_error("store busy")));
        assert!(matches!(
            store.load_address("user_1"),
            Err(AccountError::Keychain(_))
        ));
    }

    #[test]
    fn keychain_errors_never_carry_the_address() {
        let (store, mock) = fresh();
        fail_next_address(&mock, Error::TooLong("password".into(), 2560));
        let err = store
            .save_address(&person("user_1", "private.person@example.com"))
            .unwrap_err();
        assert!(!err.to_string().contains("private.person"));
        assert!(!format!("{err:?}").contains("private.person"));
    }

    #[test]
    fn the_address_credential_is_not_the_tokens_or_the_vault_keys() {
        assert_ne!(ADDRESS_USER, USER);
        assert_ne!(
            format!("{ADDRESS_USER}.{SERVICE}"),
            "default.com.zaaheen.v0.2"
        );
    }
}

/// Against the real Windows Credential Manager, under a throwaway service
/// name that is removed afterwards. vault-app's keychain tests do the same on
/// the Windows CI runner.
#[cfg(all(test, windows))]
mod windows_live {
    use super::*;

    /// Removes the throwaway credential even if an assertion fails.
    struct Cleanup(TokenStore);

    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = self.0.delete();
        }
    }

    fn throwaway() -> TokenStore {
        let mut nonce = [0u8; 8];
        getrandom::getrandom(&mut nonce).unwrap();
        let suffix: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
        TokenStore::windows_local_named(format!("com.zaaheen.test.account.{suffix}")).unwrap()
    }

    #[test]
    fn the_token_is_stored_locally_under_its_own_name() {
        let store = throwaway();
        let guard = Cleanup(store.clone());
        let token = RefreshToken::from_stored(Zeroizing::new("rt_live_check_1234".into())).unwrap();
        store.save(&token).unwrap();

        let entry = store.store.build(&store.service, USER, None).unwrap();
        let attributes: HashMap<String, String> = entry.get_attributes().unwrap();
        assert_eq!(attributes["persistence"], "Local", "must not roam");
        assert_eq!(
            attributes["target_name"],
            format!("{USER}.{}", store.service)
        );
        assert_eq!(
            store
                .load()
                .unwrap()
                .map(|t| t.expose().to_owned())
                .as_deref(),
            Some("rt_live_check_1234")
        );

        store.delete().unwrap();
        assert!(store.load().unwrap().is_none());
        drop(guard);
    }

    /// ADR-SEC-027: the address is encrypted by the OS like the token, and
    /// like the token it must not roam to other machines with the profile.
    #[test]
    fn the_address_is_stored_locally_under_its_own_name() {
        let store = throwaway();
        let _token_guard = Cleanup(store.clone());
        let address_guard = AddressCleanup(store.clone());
        let who = UserInfo::checked("user_live_1".into(), "live.check@example.com".into())
            .expect("a valid identity");
        store.save_address(&who).unwrap();

        let entry = store
            .store
            .build(&store.service, ADDRESS_USER, None)
            .unwrap();
        let attributes: HashMap<String, String> = entry.get_attributes().unwrap();
        assert_eq!(attributes["persistence"], "Local", "must not roam");
        assert_eq!(
            attributes["target_name"],
            format!("{ADDRESS_USER}.{}", store.service)
        );
        assert_eq!(
            store.load_address("user_live_1").unwrap().as_deref(),
            Some("live.check@example.com")
        );

        store.delete_address().unwrap();
        assert_eq!(store.load_address("user_live_1").unwrap(), None);
        drop(address_guard);
    }

    /// Removes the throwaway address even if an assertion fails.
    struct AddressCleanup(TokenStore);

    impl Drop for AddressCleanup {
        fn drop(&mut self) {
            let _ = self.0.delete_address();
        }
    }

    #[test]
    fn the_production_store_opens() {
        let store = TokenStore::platform().unwrap();
        assert_eq!(store.service, SERVICE);
        assert_eq!(store.persistence, Some("Local"));
    }
}
