//! PKCE (RFC 7636) and the OAuth `state` value.
//!
//! §8.26 §3 / v2 §3.2: `code_verifier` is 32 random bytes, the challenge is
//! `S256`, and `state` is 32 random bytes. Both are encoded base64url without
//! padding, which gives 43 characters from RFC 7636's unreserved set, the
//! minimum verifier length the RFC allows and 256 bits of entropy each.
//!
//! No primitive is implemented here (SP-5): SHA-256 is `sha2`, the encoding is
//! `base64`, the randomness is the OS source via `getrandom`.

use std::fmt;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::error::{AccountError, AccountResult};

/// Bytes of randomness behind a verifier or a `state`.
pub(crate) const RANDOM_BYTES: usize = 32;

/// A PKCE verifier and its S256 challenge. The verifier is wiped on drop and
/// never printed.
pub(crate) struct Pkce {
    verifier: Zeroizing<String>,
    challenge: String,
}

impl Pkce {
    /// Fresh verifier from the OS random source, with its challenge.
    ///
    /// # Errors
    ///
    /// [`AccountError::Random`] if the OS random source fails.
    pub(crate) fn generate() -> AccountResult<Self> {
        let bytes = random_bytes()?;
        // As a slice: `encode(*bytes)` would copy the secret out of its
        // wiped-on-drop wrapper.
        let verifier = Zeroizing::new(URL_SAFE_NO_PAD.encode(bytes.as_slice()));
        let challenge = s256_challenge(&verifier);
        Ok(Self {
            verifier,
            challenge,
        })
    }

    /// The `code_challenge` sent in the authorization URL.
    pub(crate) fn challenge(&self) -> &str {
        &self.challenge
    }

    /// The `code_verifier`, for tests; the flow itself takes it with
    /// [`Pkce::into_verifier`].
    #[cfg(test)]
    pub(crate) fn verifier(&self) -> &str {
        &self.verifier
    }

    /// Give up the verifier once the flow is decided.
    pub(crate) fn into_verifier(self) -> Zeroizing<String> {
        self.verifier
    }
}

impl fmt::Debug for Pkce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pkce")
            .field("verifier", &"<redacted>")
            .field("challenge", &self.challenge)
            .finish()
    }
}

/// `BASE64URL(SHA256(ASCII(verifier)))`, RFC 7636 §4.2.
pub(crate) fn s256_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// 32 random bytes, base64url without padding (43 characters). Used for the
/// OAuth `state`.
///
/// # Errors
///
/// [`AccountError::Random`] if the OS random source fails.
pub(crate) fn random_token() -> AccountResult<String> {
    Ok(URL_SAFE_NO_PAD.encode(random_bytes()?.as_slice()))
}

fn random_bytes() -> AccountResult<Zeroizing<[u8; RANDOM_BYTES]>> {
    let mut bytes = Zeroizing::new([0u8; RANDOM_BYTES]);
    getrandom::getrandom(&mut *bytes).map_err(|e| AccountError::Random(e.to_string()))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_unreserved(s: &str) -> bool {
        s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    }

    #[test]
    fn s256_matches_the_rfc_7636_appendix_b_vector() {
        // Copied from the RFC text (rfc-editor.org/rfc/rfc7636.txt, lines
        // 942 and 962), not from memory.
        assert_eq!(
            s256_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn generated_verifier_is_43_unreserved_chars_and_matches_its_challenge() {
        let pkce = Pkce::generate().unwrap();
        assert_eq!(pkce.verifier().len(), 43);
        assert!(is_unreserved(pkce.verifier()));
        assert_eq!(pkce.challenge(), s256_challenge(pkce.verifier()));
        assert_eq!(pkce.challenge().len(), 43);
    }

    #[test]
    fn every_generation_is_fresh() {
        let a = Pkce::generate().unwrap();
        let b = Pkce::generate().unwrap();
        assert_ne!(a.verifier(), b.verifier());
        let s1 = random_token().unwrap();
        let s2 = random_token().unwrap();
        assert_ne!(s1, s2);
        assert_eq!(s1.len(), 43);
        assert!(is_unreserved(&s1));
    }

    #[test]
    fn debug_never_prints_the_verifier() {
        let pkce = Pkce::generate().unwrap();
        let printed = format!("{pkce:?}");
        assert!(!printed.contains(pkce.verifier()));
        assert!(printed.contains("<redacted>"));
    }
}
