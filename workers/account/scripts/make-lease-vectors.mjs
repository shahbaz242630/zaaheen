// Writes test/vectors/lease-v1.json: signed leases that three independent
// Ed25519 implementations must agree on byte for byte:
//   - this script (Node's OpenSSL) makes them;
//   - the Worker's tests (workerd's WebCrypto) must sign the same bytes;
//   - vault-account's tests (dryoc, Rust) must verify them.
// Run once with `node scripts/make-lease-vectors.mjs`; the output is committed.
//
// The keys are the RFC 8032 §7.1 test keys (TEST 1 and TEST 2): public
// knowledge, never a key any shipped app trusts. Only public keys go in the
// file; the Worker's test holds the two seeds.
import { createPrivateKey, createPublicKey, sign } from "node:crypto";
import { writeFileSync } from "node:fs";

const PKCS8_ED25519_PREFIX = "302e020100300506032b657004220420";
const SEEDS = {
  primary: "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
  backup: "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
};
const DOMAIN = Buffer.from("zaaheen-lease-v1\0", "utf8");
const FIELD_ORDER = [
  "v",
  "kid",
  "sub",
  "state",
  "trial_ends_at",
  "active_until",
  "issued_at",
  "client_time",
  "offline_days",
];

const b64url = (bytes) => Buffer.from(bytes).toString("base64url");

const keys = Object.fromEntries(
  Object.entries(SEEDS).map(([kid, seed]) => {
    const der = Buffer.from(PKCS8_ED25519_PREFIX + seed, "hex");
    const privateKey = createPrivateKey({ key: der, format: "der", type: "pkcs8" });
    const x = createPublicKey(privateKey).export({ format: "jwk" }).x;
    return [kid, { privateKey, public_b64url: x }];
  }),
);

const SUB = "user_2abcDEF";
const cases = [
  {
    name: "active, primary key",
    payload: { v: 1, kid: "primary", sub: SUB, state: "active", trial_ends_at: 1760000000, active_until: 1762600000, issued_at: 1760500000, client_time: 1760499990, offline_days: 30 },
  },
  {
    name: "trial",
    payload: { v: 1, kid: "primary", sub: SUB, state: "trial", trial_ends_at: 1762592000, issued_at: 1760000000, client_time: 1760000007, offline_days: 30 },
  },
  {
    name: "payment failed",
    payload: { v: 1, kid: "primary", sub: SUB, state: "payment_failed", trial_ends_at: 1760000000, active_until: 1761000000, issued_at: 1760500000, client_time: 1760400000, offline_days: 30 },
  },
  {
    name: "ended after paying (subscription ended)",
    payload: { v: 1, kid: "primary", sub: SUB, state: "ended", trial_ends_at: 1760000000, active_until: 1762600000, issued_at: 1763000000, client_time: 1763000100, offline_days: 30 },
  },
  {
    name: "ended without paying (trial ended)",
    payload: { v: 1, kid: "primary", sub: SUB, state: "ended", trial_ends_at: 1762592000, issued_at: 1763000000, client_time: 1762913600, offline_days: 30 },
  },
  {
    name: "active, backup key",
    payload: { v: 1, kid: "backup", sub: SUB, state: "active", trial_ends_at: 1760000000, active_until: 1762600000, issued_at: 1760500000, client_time: 1760500000, offline_days: 30 },
  },
];

const out = {
  note: "TEST KEYS ONLY (RFC 8032 section 7.1, TEST 1 = primary, TEST 2 = backup). Made by scripts/make-lease-vectors.mjs. Checked by workers/account/test/lease.test.ts and crates/vault-account/src/lease/tests.rs.",
  domain: "zaaheen-lease-v1\\0",
  public_keys: Object.fromEntries(Object.entries(keys).map(([kid, k]) => [kid, k.public_b64url])),
  cases: cases.map(({ name, payload }) => {
    const ordered = {};
    for (const field of FIELD_ORDER) {
      if (payload[field] !== undefined) ordered[field] = payload[field];
    }
    const payload_json = JSON.stringify(ordered);
    const bytes = Buffer.from(payload_json, "utf8");
    const sig = sign(null, Buffer.concat([DOMAIN, bytes]), keys[payload.kid].privateKey);
    return { name, payload: ordered, payload_json, wire: `${b64url(bytes)}.${b64url(sig)}` };
  }),
};

writeFileSync(new URL("../test/vectors/lease-v1.json", import.meta.url), JSON.stringify(out, null, 2) + "\n");
console.log(`wrote ${out.cases.length} cases`);
