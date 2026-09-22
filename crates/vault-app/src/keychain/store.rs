//! The credential store behind the vault key (ADR-SEC-029 D1, D2, D3, Z1).
//!
//! Two credentials in the OS store:
//! - **main** — service [`super::PRODUCTION_NAMESPACE`] (or a test
//!   namespace), user `vault_id`: the key every process reads;
//! - **spare** — service `<namespace>.migrating`, same user: a verified copy
//!   that exists only while the key is being moved to Local persistence
//!   (D5). No `vault_id` can produce the spare's target name, because the
//!   Windows target is `<user>.<service>` and only the spare's service ends
//!   in `.migrating`.
//!
//! The lifecycle logic (`super::lifecycle`) sees the store only through
//! [`KeyStore`], so every failure and every crash point can be scripted in
//! the tests; [`WindowsKeyStore`] is the one real implementation.

use vault_core::VaultError;
use zeroize::Zeroizing;

/// Suffix appended to the namespace for the spare credential's service.
#[cfg(windows)]
pub(crate) const SPARE_SERVICE_SUFFIX: &str = ".migrating";

/// Which of the two credentials.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Slot {
    /// The key every process reads.
    Main,
    /// The copy held only while the key moves to Local persistence.
    Spare,
}

/// What a read found. The bytes are wiped when dropped (Z1).
// Only the Windows store builds `Present` outside the tests; elsewhere V0.2
// has no key store (see `platform_store`).
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(crate) enum Stored {
    /// The store answered "no such credential" (`Error::NoEntry`, D3).
    Absent,
    /// A credential exists; its bytes, of whatever length.
    Present(Zeroizing<Vec<u8>>),
}

/// A credential-store failure. Never carries stored bytes (Z1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoreError(pub(crate) String);

impl From<StoreError> for VaultError {
    fn from(e: StoreError) -> Self {
        VaultError::KeychainProvenance(e.0)
    }
}

/// The two credentials, as the lifecycle logic needs them.
pub(crate) trait KeyStore {
    /// Read a credential. `Absent` only for the store's "no entry" (D3).
    fn read(&self, slot: Slot) -> Result<Stored, StoreError>;

    /// Whether a credential is stored with Local persistence; `None` when
    /// there is no credential.
    fn is_local(&self, slot: Slot) -> Result<Option<bool>, StoreError>;

    /// Write `key` with Local persistence (D2), replacing any credential of
    /// the same name in place.
    fn write_local(&self, slot: Slot, key: &[u8; 32]) -> Result<(), StoreError>;

    /// Delete a credential: `true` if one was removed, `false` if there was
    /// none. Deleting needs no read first (E2).
    fn delete(&self, slot: Slot) -> Result<bool, StoreError>;
}

/// The spare credential's service name for `namespace`.
#[cfg(windows)]
pub(crate) fn spare_service(namespace: &str) -> String {
    format!("{namespace}{SPARE_SERVICE_SUFFIX}")
}

/// Windows Credential Manager, through this module's own store instance —
/// never keyring-core's process-global default (D1).
#[cfg(windows)]
pub(crate) struct WindowsKeyStore {
    store: std::sync::Arc<keyring_core::CredentialStore>,
    namespace: String,
    vault_id: String,
}

#[cfg(windows)]
impl WindowsKeyStore {
    /// Open the store for the key `(namespace, vault_id)`.
    pub(crate) fn open(namespace: &str, vault_id: &str) -> Result<Self, StoreError> {
        let store = windows_native_keyring_store::Store::new().map_err(store_error)?;
        Ok(Self {
            store,
            namespace: namespace.to_owned(),
            vault_id: vault_id.to_owned(),
        })
    }

    fn entry(&self, slot: Slot) -> Result<keyring_core::Entry, StoreError> {
        let service = match slot {
            Slot::Main => self.namespace.clone(),
            Slot::Spare => spare_service(&self.namespace),
        };
        // Persistence only takes effect when a secret is written (the
        // library's `lib.rs:40-45`); reads and deletes ignore it.
        let modifiers = std::collections::HashMap::from([("persistence", "Local")]);
        self.store
            .build(&service, &self.vault_id, Some(&modifiers))
            .map_err(store_error)
    }
}

#[cfg(windows)]
impl KeyStore for WindowsKeyStore {
    fn read(&self, slot: Slot) -> Result<Stored, StoreError> {
        match self.entry(slot)?.get_secret() {
            Ok(bytes) => Ok(Stored::Present(Zeroizing::new(bytes))),
            Err(keyring_core::Error::NoEntry) => Ok(Stored::Absent),
            Err(e) => Err(store_error(e)),
        }
    }

    fn is_local(&self, slot: Slot) -> Result<Option<bool>, StoreError> {
        match self.entry(slot)?.get_attributes() {
            Ok(attributes) => Ok(Some(
                attributes.get("persistence").map(String::as_str) == Some("Local"),
            )),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(store_error(e)),
        }
    }

    fn write_local(&self, slot: Slot, key: &[u8; 32]) -> Result<(), StoreError> {
        self.entry(slot)?.set_secret(key).map_err(store_error)
    }

    fn delete(&self, slot: Slot) -> Result<bool, StoreError> {
        match self.entry(slot)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring_core::Error::NoEntry) => Ok(false),
            Err(e) => Err(store_error(e)),
        }
    }
}

/// The store for this platform: Windows Credential Manager, or — elsewhere,
/// where V0.2 has no key store yet — an error, so every caller (erasure
/// included) fails loudly rather than reporting a success that did not
/// happen.
pub(crate) fn platform_store(
    namespace: &str,
    vault_id: &str,
) -> Result<Box<dyn KeyStore>, StoreError> {
    #[cfg(windows)]
    {
        Ok(Box::new(WindowsKeyStore::open(namespace, vault_id)?))
    }
    #[cfg(not(windows))]
    {
        let _ = (namespace, vault_id);
        Err(StoreError(format!(
            "the vault key store is Windows-only in this version (current platform: {})",
            std::env::consts::OS
        )))
    }
}

/// A keyring error as text, never with the stored bytes two of its variants
/// carry (Z1; keyring-core `error.rs:44, 50`).
#[cfg(windows)]
pub(crate) fn store_error(e: keyring_core::Error) -> StoreError {
    use zeroize::Zeroize;
    StoreError(match e {
        keyring_core::Error::BadEncoding(mut raw)
        | keyring_core::Error::BadDataFormat(mut raw, _) => {
            raw.zeroize();
            "the stored value is malformed".to_owned()
        }
        other => other.to_string(),
    })
}
