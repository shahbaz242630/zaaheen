//! The seal, checked again at the click (s115, ADR-SEC-051).
//!
//! The updater checks an update's seal once, when it downloads it, and never
//! again before it installs. We keep the verified file on disk (not ~155 MB
//! in memory all day), so a file another program swapped while it waited
//! must be caught here: the same check, against the same public key, run on
//! the bytes about to be installed.
//!
//! The same rules as the updater's own check (`verify_signature` in
//! tauri-plugin-updater 2.12.0), with one stricter: the version sealed in the
//! signature's trusted comment must be exactly the version offered, text for
//! text, and must be there at all.

use base64::Engine;
use minisign_verify::{PublicKey, Signature};

/// Why a seal was refused. Logged, never shown.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// The key, the signature or their encoding could not be read.
    Unreadable(&'static str),
    /// The bytes are not what our key signed.
    NotOurSeal,
    /// Sealed by our key, but with no version, or another version.
    WrongVersion,
}

/// Check `bytes` against the base64 `signature` (the `.sig` file's text, as
/// the manifest carries it) and the base64 `pubkey` compiled into the app,
/// and that the signature was made for `version`.
pub fn verify(bytes: &[u8], signature: &str, pubkey: &str, version: &str) -> Result<(), Refused> {
    let pubkey = PublicKey::decode(&decode(pubkey)?).map_err(|_| Refused::Unreadable("key"))?;
    let signature =
        Signature::decode(&decode(signature)?).map_err(|_| Refused::Unreadable("signature"))?;
    pubkey
        .verify(bytes, &signature, true)
        .map_err(|_| Refused::NotOurSeal)?;
    // Only now is the trusted comment ours: the signature covers it, and the
    // line above is what checked that.
    match signed_version(signature.trusted_comment()) {
        Some(signed) if signed == version => Ok(()),
        _ => Err(Refused::WrongVersion),
    }
}

/// The `version:` field of a trusted comment (`timestamp:…\tfile:…\tversion:…`).
pub fn signed_version(trusted_comment: &str) -> Option<&str> {
    trusted_comment
        .split('\t')
        .find_map(|field| field.strip_prefix("version:"))
}

fn decode(text: &str) -> Result<String, Refused> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .map_err(|_| Refused::Unreadable("base64"))?;
    String::from_utf8(bytes).map_err(|_| Refused::Unreadable("text"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The throwaway rehearsal key's public half (the one in tauri.conf.json
    /// until the founder makes the real key).
    const PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDc5MDMyMkE0QkRFNzkwMkIKUldRcmtPZTlwQ0lEZWYxNWZGSWdCaVRJYThxSE9GNTMybDFwMEZJRW4zNkIwYW9jcXV6S1E5akMK";

    /// A file signed with the throwaway key by tauri-cli 2.11.0, which writes
    /// no version into the trusted comment.
    const FIXTURE: &[u8] = include_bytes!("../../../tests/fixtures/update/fixture.bin");
    const FIXTURE_SIG: &str = include_str!("../../../tests/fixtures/update/fixture.bin.sig");

    #[test]
    fn our_seal_on_the_right_bytes_passes_the_signature_and_then_needs_a_version() {
        // The signature is good, so the refusal is the version rule, not the seal.
        assert_eq!(
            verify(FIXTURE, FIXTURE_SIG, PUBKEY, "0.3.8"),
            Err(Refused::WrongVersion)
        );
    }

    #[test]
    fn a_changed_file_is_not_our_seal() {
        let mut swapped = FIXTURE.to_vec();
        swapped[0] ^= 1;
        assert_eq!(
            verify(&swapped, FIXTURE_SIG, PUBKEY, "0.3.8"),
            Err(Refused::NotOurSeal)
        );
    }

    #[test]
    fn another_key_is_not_our_seal() {
        // A second throwaway key (ID 304CF67CE83FCCA4, made s115 for this test).
        const OTHER: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDMwNENGNjdDRTgzRkNDQTQKUldTa3pEL29mUFpNTUgzaFMyUW05UnB0QjdXbm1nSzl3UjNJZGxtRG9sRDByMExjdnZPS3huYjkK";
        assert_eq!(
            verify(FIXTURE, FIXTURE_SIG, OTHER, "0.3.8"),
            Err(Refused::NotOurSeal)
        );
    }

    #[test]
    fn unreadable_input_is_refused() {
        assert!(matches!(
            verify(FIXTURE, "not base64!", PUBKEY, "0.3.8"),
            Err(Refused::Unreadable(_))
        ));
        assert!(matches!(
            verify(FIXTURE, FIXTURE_SIG, "", "0.3.8"),
            Err(Refused::Unreadable(_))
        ));
    }

    #[test]
    fn the_signed_version_is_read_from_its_own_field_only() {
        assert_eq!(
            signed_version("timestamp:1700000000\tfile:Zaaheen_0.3.9_x64.msi\tversion:0.3.9"),
            Some("0.3.9")
        );
        assert_eq!(signed_version("timestamp:1\tfile:app-version:2.msi"), None);
        assert_eq!(signed_version("timestamp:1\tfile:x.msi"), None);
    }
}
