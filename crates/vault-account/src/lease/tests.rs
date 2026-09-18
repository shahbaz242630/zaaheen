//! Lease verification tests, including the opener's list for this step: bad
//! signature, wrong `sub`, both public keys, exact bytes before parse.
//! ("Expired" is the time model's, S1 step 4.) Keys are generated per test;
//! the real ones arrive with S2.

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use dryoc::classic::crypto_sign::{crypto_sign_detached, crypto_sign_keypair, SecretKey};
use serde_json::{json, Value};

use super::*;

const SUB: &str = "user_2abcDEF";

struct Keys {
    primary_pk: [u8; 32],
    primary_sk: SecretKey,
    backup_pk: [u8; 32],
    backup_sk: SecretKey,
    stranger_sk: SecretKey,
}

fn keys() -> Keys {
    let (primary_pk, primary_sk) = crypto_sign_keypair();
    let (backup_pk, backup_sk) = crypto_sign_keypair();
    let (_, stranger_sk) = crypto_sign_keypair();
    Keys {
        primary_pk,
        primary_sk,
        backup_pk,
        backup_sk,
        stranger_sk,
    }
}

fn verifier(k: &Keys) -> LeaseVerifier {
    LeaseVerifier::new(
        LeaseKey::new("primary", k.primary_pk).unwrap(),
        LeaseKey::new("backup", k.backup_pk).unwrap(),
    )
    .unwrap()
}

fn payload() -> Value {
    json!({
        "v": 1,
        "kid": "primary",
        "sub": SUB,
        "state": "active",
        "trial_ends_at": 1_760_000_000_i64,
        "active_until": 1_762_600_000_i64,
        "issued_at": 1_760_500_000_i64,
        "client_time": 1_760_499_990_i64,
        "offline_days": 30
    })
}

fn with(fields: &[(&str, Value)]) -> Value {
    let mut p = payload();
    for (name, value) in fields {
        p[*name] = value.clone();
    }
    p
}

fn without(field: &str) -> Value {
    let mut p = payload();
    p.as_object_mut().unwrap().remove(field);
    p
}

/// Sign `payload_bytes` under `domain` with `sk` and build the wire form.
fn sign_bytes(payload_bytes: &[u8], sk: &SecretKey, domain: &[u8]) -> String {
    let mut message = domain.to_vec();
    message.extend_from_slice(payload_bytes);
    let mut sig = [0u8; 64];
    crypto_sign_detached(&mut sig, &message, sk).unwrap();
    format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(payload_bytes),
        URL_SAFE_NO_PAD.encode(sig)
    )
}

fn wire(p: &Value, sk: &SecretKey) -> String {
    sign_bytes(p.to_string().as_bytes(), sk, LEASE_DOMAIN)
}

fn reason(result: AccountResult<Lease>) -> String {
    match result {
        Err(AccountError::LeaseRejected(why)) => why,
        other => panic!("expected LeaseRejected, got {other:?}"),
    }
}

// ---- both keys, and only those keys ------------------------------------------------

#[test]
fn a_lease_from_the_primary_key_verifies_and_keeps_its_exact_bytes() {
    let k = keys();
    let w = wire(&payload(), &k.primary_sk);
    let lease = verifier(&k).verify(w.as_bytes(), SUB).unwrap();
    assert_eq!(lease.wire(), w.as_bytes());
    assert_eq!(lease.kid(), "primary");
    assert_eq!(lease.sub(), SUB);
    assert_eq!(lease.state(), LeaseState::Active);
    assert_eq!(lease.trial_ends_at(), Some(1_760_000_000));
    assert_eq!(lease.active_until(), Some(1_762_600_000));
    assert_eq!(lease.issued_at(), 1_760_500_000);
    assert_eq!(lease.client_time(), 1_760_499_990);
    assert_eq!(lease.offline_days(), 30);
}

#[test]
fn a_lease_from_the_backup_key_verifies() {
    let k = keys();
    let w = wire(&with(&[("kid", json!("backup"))]), &k.backup_sk);
    let lease = verifier(&k).verify(w.as_bytes(), SUB).unwrap();
    assert_eq!(lease.kid(), "backup");
}

#[test]
fn a_lease_from_any_other_key_is_a_bad_signature() {
    let k = keys();
    let w = wire(&payload(), &k.stranger_sk);
    assert!(reason(verifier(&k).verify(w.as_bytes(), SUB)).contains("signature"));
}

#[test]
fn a_kid_naming_the_other_key_is_rejected() {
    let k = keys();
    // Genuinely signed by the backup key, but claiming to be the primary's.
    let w = wire(&payload(), &k.backup_sk);
    reason(verifier(&k).verify(w.as_bytes(), SUB));
    let w = wire(&with(&[("kid", json!("tertiary"))]), &k.primary_sk);
    reason(verifier(&k).verify(w.as_bytes(), SUB));
}

// ---- tampering ---------------------------------------------------------------------

#[test]
fn a_changed_payload_or_signature_is_a_bad_signature() {
    let k = keys();
    let v = verifier(&k);
    let good = wire(&payload(), &k.primary_sk);
    let (p, s) = good.split_once('.').unwrap();

    // Re-point the payload at a richer state, keeping the old signature.
    let forged_payload = URL_SAFE_NO_PAD.encode(
        with(&[("active_until", json!(4_000_000_000_i64))])
            .to_string()
            .as_bytes(),
    );
    let forged = format!("{forged_payload}.{s}");
    assert!(reason(v.verify(forged.as_bytes(), SUB)).contains("signature"));

    // Flip one bit of the signature.
    let mut sig = URL_SAFE_NO_PAD.decode(s).unwrap();
    sig[10] ^= 0x01;
    let flipped = format!("{p}.{}", URL_SAFE_NO_PAD.encode(sig));
    assert!(reason(v.verify(flipped.as_bytes(), SUB)).contains("signature"));
}

#[test]
fn the_signature_is_checked_before_the_payload_is_read() {
    let k = keys();
    // Not JSON at all, and not signed by us: the answer must be "signature",
    // proving nothing tried to parse it first.
    let junk = sign_bytes(b"{not json", &k.stranger_sk, LEASE_DOMAIN);
    assert!(reason(verifier(&k).verify(junk.as_bytes(), SUB)).contains("signature"));
    // Signed by us but not JSON: now it is the payload that fails.
    let junk = sign_bytes(b"{not json", &k.primary_sk, LEASE_DOMAIN);
    assert!(!reason(verifier(&k).verify(junk.as_bytes(), SUB)).contains("signature"));
}

#[test]
fn verification_uses_the_exact_bytes_and_never_re_serialises() {
    let k = keys();
    let v = verifier(&k);
    let pretty = serde_json::to_string_pretty(&payload()).unwrap();
    let compact = payload().to_string();
    assert_ne!(pretty, compact);

    // Signed over the pretty bytes, verified as the pretty bytes: fine.
    let w = sign_bytes(pretty.as_bytes(), &k.primary_sk, LEASE_DOMAIN);
    assert!(v.verify(w.as_bytes(), SUB).is_ok());

    // Signature over the pretty bytes attached to the same data written
    // compactly: rejected, although the two parse to the same object.
    let sig = sign_bytes(pretty.as_bytes(), &k.primary_sk, LEASE_DOMAIN);
    let (_, s) = sig.split_once('.').unwrap();
    let swapped = format!("{}.{s}", URL_SAFE_NO_PAD.encode(compact.as_bytes()));
    assert!(reason(v.verify(swapped.as_bytes(), SUB)).contains("signature"));
}

#[test]
fn the_domain_prefix_is_part_of_what_is_signed() {
    let k = keys();
    let v = verifier(&k);
    let bytes = payload().to_string();
    for domain in [&b""[..], b"zaaheen-lease-v2\0", b"zaaheen-lease-v1"] {
        let w = sign_bytes(bytes.as_bytes(), &k.primary_sk, domain);
        assert!(reason(v.verify(w.as_bytes(), SUB)).contains("signature"));
    }
}

// ---- whose lease -------------------------------------------------------------------

#[test]
fn a_lease_about_another_user_is_rejected() {
    let k = keys();
    let v = verifier(&k);
    let w = wire(&payload(), &k.primary_sk);
    for other in ["user_2zzz", "", "USER_2ABCDEF", "user_2abcDEF "] {
        reason(v.verify(w.as_bytes(), other));
    }
    for bad_sub in [json!(""), json!("user\n2"), json!(42), json!(null)] {
        let w = wire(&with(&[("sub", bad_sub.clone())]), &k.primary_sk);
        reason(v.verify(w.as_bytes(), SUB));
    }
}

#[test]
fn rejections_never_quote_the_lease() {
    let k = keys();
    let w = wire(&payload(), &k.primary_sk);
    let why = reason(verifier(&k).verify(w.as_bytes(), "user_OTHER_person"));
    assert!(!why.contains(SUB));
    assert!(!why.contains("user_OTHER_person"));
}

// ---- payload rules -----------------------------------------------------------------

#[test]
fn only_version_1_is_understood() {
    let k = keys();
    let v = verifier(&k);
    for p in [
        with(&[("v", json!(2))]),
        with(&[("v", json!(0))]),
        with(&[("v", json!("1"))]),
        without("v"),
    ] {
        reason(v.verify(wire(&p, &k.primary_sk).as_bytes(), SUB));
    }
}

#[test]
fn each_state_parses_and_unknown_names_are_rejected() {
    let k = keys();
    let v = verifier(&k);
    for (name, state) in [
        ("trial", LeaseState::Trial),
        ("active", LeaseState::Active),
        ("payment_failed", LeaseState::PaymentFailed),
        ("ended", LeaseState::Ended),
    ] {
        let w = wire(&with(&[("state", json!(name))]), &k.primary_sk);
        assert_eq!(v.verify(w.as_bytes(), SUB).unwrap().state(), state);
    }
    for bad in ["Active", "paid", "", "comped"] {
        let w = wire(&with(&[("state", json!(bad))]), &k.primary_sk);
        reason(v.verify(w.as_bytes(), SUB));
    }
}

#[test]
fn each_state_must_carry_its_own_deadline() {
    let k = keys();
    let v = verifier(&k);
    let lease = |fields: &[(&str, Value)]| {
        let mut p = with(fields);
        for (name, value) in fields {
            if value.is_null() {
                p.as_object_mut().unwrap().remove(*name);
            }
        }
        v.verify(wire(&p, &k.primary_sk).as_bytes(), SUB)
    };
    reason(lease(&[
        ("state", json!("trial")),
        ("trial_ends_at", Value::Null),
    ]));
    reason(lease(&[
        ("state", json!("active")),
        ("active_until", Value::Null),
    ]));
    reason(lease(&[
        ("state", json!("payment_failed")),
        ("active_until", Value::Null),
    ]));
    // A trial lease needs no paid period, a paid one no trial end, and an
    // ended one neither.
    let trial = lease(&[("state", json!("trial")), ("active_until", Value::Null)]).unwrap();
    assert_eq!(trial.active_until(), None);
    let active = lease(&[("state", json!("active")), ("trial_ends_at", Value::Null)]).unwrap();
    assert_eq!(active.trial_ends_at(), None);
    let ended = lease(&[
        ("state", json!("ended")),
        ("trial_ends_at", Value::Null),
        ("active_until", Value::Null),
    ])
    .unwrap();
    assert_eq!(ended.state(), LeaseState::Ended);
}

#[test]
fn a_json_null_deadline_counts_as_absent() {
    let k = keys();
    let w = wire(
        &with(&[("state", json!("active")), ("active_until", Value::Null)]),
        &k.primary_sk,
    );
    reason(verifier(&k).verify(w.as_bytes(), SUB));
}

#[test]
fn times_must_be_whole_numbers_and_the_required_ones_present() {
    let k = keys();
    let v = verifier(&k);
    for p in [
        with(&[("issued_at", json!("1760500000"))]),
        with(&[("issued_at", json!(1_760_500_000.5))]),
        with(&[("client_time", json!(true))]),
        with(&[("active_until", json!("soon"))]),
        with(&[("issued_at", json!(0))]),
        with(&[("issued_at", json!(-5))]),
        without("issued_at"),
        without("client_time"),
        without("offline_days"),
        without("kid"),
        without("sub"),
        without("state"),
    ] {
        reason(v.verify(wire(&p, &k.primary_sk).as_bytes(), SUB));
    }
}

#[test]
fn the_client_time_is_taken_as_signed_even_when_far_off() {
    // §8.26 §4: signed "as received, with no clamp".
    let k = keys();
    let w = wire(
        &with(&[("client_time", json!(1_000_000_000_i64))]),
        &k.primary_sk,
    );
    let lease = verifier(&k).verify(w.as_bytes(), SUB).unwrap();
    assert_eq!(lease.client_time(), 1_000_000_000);
}

#[test]
fn offline_days_must_be_sensible() {
    let k = keys();
    let v = verifier(&k);
    for bad in [
        json!(0),
        json!(-1),
        json!(MAX_OFFLINE_DAYS + 1),
        json!(30.5),
    ] {
        let w = wire(&with(&[("offline_days", bad.clone())]), &k.primary_sk);
        reason(v.verify(w.as_bytes(), SUB));
    }
    for ok in [1, 30, MAX_OFFLINE_DAYS] {
        let w = wire(&with(&[("offline_days", json!(ok))]), &k.primary_sk);
        assert_eq!(v.verify(w.as_bytes(), SUB).unwrap().offline_days(), ok);
    }
}

#[test]
fn unknown_fields_are_ignored_for_forward_compatibility() {
    let k = keys();
    let w = wire(
        &with(&[("added_by_a_newer_worker", json!({"x": 1}))]),
        &k.primary_sk,
    );
    assert!(verifier(&k).verify(w.as_bytes(), SUB).is_ok());
}

#[test]
fn a_duplicated_field_is_rejected() {
    let k = keys();
    let text = payload().to_string();
    // Same object, with a second "state" appended just before the closing
    // brace: whichever copy a parser kept, the lease is ambiguous.
    let doubled = format!("{},\"state\":\"ended\"}}", &text[..text.len() - 1]);
    let w = sign_bytes(doubled.as_bytes(), &k.primary_sk, LEASE_DOMAIN);
    reason(verifier(&k).verify(w.as_bytes(), SUB));
}

// ---- the wire itself ---------------------------------------------------------------

#[test]
fn malformed_wire_forms_are_rejected() {
    let k = keys();
    let v = verifier(&k);
    let good = wire(&payload(), &k.primary_sk);
    let (p, s) = good.split_once('.').unwrap();
    let raw_sig = URL_SAFE_NO_PAD.decode(s).unwrap();
    let padded_payload = STANDARD.encode(payload().to_string().as_bytes());
    for bad in [
        String::new(),
        p.to_string(),
        format!("{p}."),
        format!(".{s}"),
        format!("{p}.{s}.{s}"),
        format!("{p}={s}"),
        format!("{p}.{s}="),
        format!("{padded_payload}.{s}"),
        format!(" {good}"),
        format!("{good}\n"),
        format!("{p}.{}", URL_SAFE_NO_PAD.encode(&raw_sig[..63])),
        format!(
            "{p}.{}",
            URL_SAFE_NO_PAD.encode([raw_sig.as_slice(), &[0]].concat())
        ),
        good.replace('-', "+"),
    ] {
        if bad == good {
            continue;
        }
        reason(v.verify(bad.as_bytes(), SUB));
    }
    reason(v.verify("lease.é".as_bytes(), SUB));
}

#[test]
fn an_oversized_lease_is_rejected_before_anything_else() {
    let k = keys();
    let big = with(&[("pad", json!("x".repeat(MAX_LEASE_BYTES)))]);
    let w = wire(&big, &k.primary_sk);
    assert!(w.len() > MAX_LEASE_BYTES);
    reason(verifier(&k).verify(w.as_bytes(), SUB));
}

// ---- the key set -------------------------------------------------------------------

#[test]
fn key_ids_are_short_lowercase_names() {
    let k = keys();
    for ok in ["primary", "backup", "k-2026-09", "a"] {
        assert!(LeaseKey::new(ok, k.primary_pk).is_ok(), "{ok}");
    }
    for bad in ["", "Primary", "has space", "kid.1", "é", &"k".repeat(33)] {
        assert!(
            matches!(
                LeaseKey::new(bad, k.primary_pk),
                Err(AccountError::InvalidConfig(_))
            ),
            "{bad:?}"
        );
    }
}

#[test]
fn the_two_shipped_keys_must_differ() {
    let k = keys();
    let primary = LeaseKey::new("primary", k.primary_pk).unwrap();
    let same_kid = LeaseKey::new("primary", k.backup_pk).unwrap();
    let same_key = LeaseKey::new("backup", k.primary_pk).unwrap();
    assert!(LeaseVerifier::new(primary.clone(), same_kid).is_err());
    assert!(LeaseVerifier::new(primary, same_key).is_err());
}

#[test]
fn debug_output_names_keys_by_id_only() {
    let k = keys();
    let printed = format!("{:?}", verifier(&k));
    assert!(printed.contains("primary") && printed.contains("backup"));
    assert!(!printed.contains(&format!("{:?}", k.primary_pk[0..4].to_vec())));
}
