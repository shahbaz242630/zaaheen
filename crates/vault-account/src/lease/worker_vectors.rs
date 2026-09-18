//! The account Worker's leases, checked by this crate (S2, §8.28).
//!
//! `workers/account/test/vectors/lease-v1.json` holds leases made by an
//! independent Ed25519 implementation (Node's OpenSSL). The Worker's own
//! tests must sign the same bytes under workerd; these tests must verify
//! them. So the Worker and the app cannot drift apart on the wire, the
//! domain prefix, the field names or the key encoding without a test on one
//! side failing.
//!
//! The keys are RFC 8032 §7.1's published test keys, pinned below, so the
//! file cannot be quietly regenerated under keys nobody can check.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde_json::Value;

use super::*;

const VECTORS: &str = include_str!("../../../../workers/account/test/vectors/lease-v1.json");

/// RFC 8032 §7.1 TEST 1 and TEST 2 public keys, as published.
const RFC8032_TEST_1_PUBLIC: &str =
    "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";
const RFC8032_TEST_2_PUBLIC: &str =
    "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c";

fn vectors() -> Value {
    serde_json::from_str(VECTORS).expect("the vector file is JSON")
}

fn public_key(v: &Value, kid: &str) -> [u8; 32] {
    let text = v["public_keys"][kid].as_str().expect("a public key string");
    let bytes = URL_SAFE_NO_PAD.decode(text).expect("base64url");
    <[u8; 32]>::try_from(bytes.as_slice()).expect("32 bytes")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn verifier(v: &Value) -> LeaseVerifier {
    LeaseVerifier::new(
        LeaseKey::new("primary", public_key(v, "primary")).expect("kid"),
        LeaseKey::new("backup", public_key(v, "backup")).expect("kid"),
    )
    .expect("two different keys")
}

#[test]
fn the_vector_keys_are_the_published_rfc_8032_test_keys() {
    let v = vectors();
    assert!(hex(&public_key(&v, "primary")) == RFC8032_TEST_1_PUBLIC);
    assert!(hex(&public_key(&v, "backup")) == RFC8032_TEST_2_PUBLIC);
}

#[test]
fn every_worker_lease_verifies_and_carries_the_fields_it_was_signed_with() {
    let v = vectors();
    let verifier = verifier(&v);
    let cases = v["cases"].as_array().expect("cases");
    assert!(cases.len() == 6, "the vector file lost or gained cases");
    for case in cases {
        let name = case["name"].as_str().expect("name");
        let p = &case["payload"];
        let wire = case["wire"].as_str().expect("wire");
        let sub = p["sub"].as_str().expect("sub");

        // The signed bytes are exactly the recorded JSON text.
        let (payload_b64, _) = wire.split_once('.').expect("two parts");
        let signed = URL_SAFE_NO_PAD.decode(payload_b64).expect("base64url");
        assert!(
            signed == case["payload_json"].as_str().expect("text").as_bytes(),
            "{name}: signed bytes differ from payload_json"
        );

        let lease = verifier
            .verify(wire.as_bytes(), sub)
            .unwrap_or_else(|_| panic!("{name}: the app refused the Worker's lease"));
        let state = match p["state"].as_str().expect("state") {
            "trial" => LeaseState::Trial,
            "active" => LeaseState::Active,
            "payment_failed" => LeaseState::PaymentFailed,
            "ended" => LeaseState::Ended,
            _ => panic!("{name}: unknown state in the vector file"),
        };
        assert!(lease.state() == state, "{name}: state");
        assert!(
            lease.kid() == p["kid"].as_str().expect("kid"),
            "{name}: kid"
        );
        assert!(lease.wire() == wire.as_bytes(), "{name}: wire");
        assert!(
            lease.trial_ends_at() == p["trial_ends_at"].as_i64(),
            "{name}: trial_ends_at"
        );
        assert!(
            lease.active_until() == p["active_until"].as_i64(),
            "{name}: active_until"
        );
        assert!(
            lease.issued_at() == p["issued_at"].as_i64().expect("issued_at"),
            "{name}: issued_at"
        );
        assert!(
            lease.client_time() == p["client_time"].as_i64().expect("client_time"),
            "{name}: client_time"
        );
        let days = p["offline_days"].as_u64().expect("offline_days");
        assert!(
            u64::from(lease.offline_days()) == days,
            "{name}: offline_days"
        );
    }
}

#[test]
fn a_worker_lease_is_still_refused_for_another_user_or_after_tampering() {
    let v = vectors();
    let verifier = verifier(&v);
    let wire = v["cases"][0]["wire"].as_str().expect("wire");
    assert!(verifier
        .verify(wire.as_bytes(), "user_someone_else")
        .is_err());

    let (payload_b64, sig_b64) = wire.split_once('.').expect("two parts");
    let mut payload = URL_SAFE_NO_PAD.decode(payload_b64).expect("base64url");
    let last = payload.len() - 2; // inside the final number, before the '}'
    payload[last] = if payload[last] == b'9' { b'8' } else { b'9' };
    let tampered = format!("{}.{sig_b64}", URL_SAFE_NO_PAD.encode(payload));
    let sub = v["cases"][0]["payload"]["sub"].as_str().expect("sub");
    assert!(verifier.verify(tampered.as_bytes(), sub).is_err());
}
