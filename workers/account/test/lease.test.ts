// Lease signing (SIGNIN-DESIGN.md §4, §8.27, §8.28). The Worker must produce
// exactly the bytes vault-account verifies, and must refuse to sign anything
// the app would reject: a lease the app refuses is a user locked out.
import { describe, expect, it } from "vitest";

import {
  LEASE_DOMAIN,
  LeaseError,
  type LeasePayload,
  importLeaseKey,
  payloadJson,
  signLease,
} from "../src/lease";
import vectors from "./vectors/lease-v1.json";

// RFC 8032 §7.1 TEST 1 (primary) and TEST 2 (backup): published test keys.
const SEEDS: Record<string, string> = {
  primary: "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
  backup: "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
};
const PKCS8_PREFIX = "302e020100300506032b657004220420";

function hexBytes(hex: string): Uint8Array {
  return Uint8Array.from(hex.match(/../g) ?? [], (h) => parseInt(h, 16));
}

function base64(bytes: Uint8Array): string {
  return btoa(String.fromCharCode(...bytes));
}

function pkcs8(kid: string): string {
  return base64(hexBytes(PKCS8_PREFIX + (SEEDS[kid] ?? "")));
}

function fromBase64url(text: string): Uint8Array {
  const b64 = text.replace(/-/g, "+").replace(/_/g, "/");
  return Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
}

const good: LeasePayload = {
  v: 1,
  kid: "primary",
  sub: "user_2abcDEF",
  state: "active",
  trial_ends_at: 1760000000,
  active_until: 1762600000,
  issued_at: 1760500000,
  client_time: 1760499990,
  offline_days: 30,
};

function withFields(fields: Record<string, unknown>): LeasePayload {
  const p: Record<string, unknown> = { ...good, ...fields };
  for (const [k, v] of Object.entries(fields)) {
    if (v === undefined) delete p[k];
  }
  return p as unknown as LeasePayload;
}

describe("the shared vectors (Node, workerd and the Rust app agree)", () => {
  it("has the six cases the Rust side checks", () => {
    expect(vectors.cases).toHaveLength(6);
  });

  for (const c of vectors.cases) {
    it(`signs "${c.name}" to the exact recorded bytes`, async () => {
      const payload = c.payload as LeasePayload;
      expect(payloadJson(payload)).toBe(c.payload_json);
      const key = await importLeaseKey(payload.kid, pkcs8(payload.kid));
      expect(await signLease(payload, key)).toBe(c.wire);
    });
  }
});

describe("signing", () => {
  it("signs the domain prefix plus the exact payload bytes", async () => {
    const key = await importLeaseKey("primary", pkcs8("primary"));
    const wire = await signLease(good, key);
    const [p, s] = wire.split(".");
    const payloadBytes = fromBase64url(p ?? "");
    expect(new TextDecoder().decode(payloadBytes)).toBe(payloadJson(good));

    const message = new Uint8Array([...new TextEncoder().encode(LEASE_DOMAIN), ...payloadBytes]);
    const publicKey = await crypto.subtle.importKey(
      "raw",
      fromBase64url(vectors.public_keys.primary),
      { name: "Ed25519" },
      false,
      ["verify"],
    );
    const sig = fromBase64url(s ?? "");
    expect(await crypto.subtle.verify({ name: "Ed25519" }, publicKey, sig, message)).toBe(true);
    // Without the domain prefix the same signature is worthless.
    expect(await crypto.subtle.verify({ name: "Ed25519" }, publicKey, sig, payloadBytes)).toBe(false);
  });

  it("uses base64url without padding in exactly two parts", async () => {
    const key = await importLeaseKey("primary", pkcs8("primary"));
    const wire = await signLease(good, key);
    expect(wire).toMatch(/^[A-Za-z0-9_-]+\.[A-Za-z0-9_-]{86}$/);
  });

  it("refuses a payload whose kid is not the signing key's", async () => {
    const key = await importLeaseKey("backup", pkcs8("backup"));
    await expect(signLease(good, key)).rejects.toThrow(LeaseError);
  });

  it("omits absent deadlines rather than writing null", () => {
    const json = payloadJson(withFields({ state: "ended", active_until: undefined }));
    expect(json).not.toContain("active_until");
    expect(json).not.toContain("null");
  });
});

describe("never signs what the app would refuse", () => {
  const bad: Array<[string, Record<string, unknown>]> = [
    ["version 2", { v: 2 }],
    ["an upper-case kid", { kid: "Primary" }],
    ["an empty kid", { kid: "" }],
    ["a 33-character kid", { kid: "k".repeat(33) }],
    ["an empty sub", { sub: "" }],
    ["a sub with a space", { sub: "user 2" }],
    ["a sub with a newline", { sub: "user\n2" }],
    ["a non-ASCII sub", { sub: "usér" }],
    ["a 257-character sub", { sub: "u".repeat(257) }],
    ["an unknown state", { state: "paid" }],
    ["a trial without its end", { state: "trial", trial_ends_at: undefined }],
    ["an active lease without its end", { state: "active", active_until: undefined }],
    ["a failed payment without its end", { state: "payment_failed", active_until: undefined }],
    ["issued_at 0", { issued_at: 0 }],
    ["a negative issued_at", { issued_at: -1 }],
    ["a fractional issued_at", { issued_at: 1760500000.5 }],
    ["an unsafe integer", { issued_at: 2 ** 53 }],
    ["client_time 0", { client_time: 0 }],
    ["a string time", { client_time: "1760499990" }],
    ["a fractional deadline", { active_until: 1762600000.5 }],
    ["offline_days 0", { offline_days: 0 }],
    ["offline_days 367", { offline_days: 367 }],
    ["fractional offline_days", { offline_days: 30.5 }],
  ];
  for (const [name, fields] of bad) {
    it(`refuses ${name}`, () => {
      expect(() => payloadJson(withFields(fields))).toThrow(LeaseError);
    });
  }

  it("accepts the edges the app accepts", () => {
    expect(() => payloadJson(withFields({ sub: "u".repeat(256) }))).not.toThrow();
    expect(() => payloadJson(withFields({ kid: "k-2026-09" }))).not.toThrow();
    expect(() => payloadJson(withFields({ offline_days: 1 }))).not.toThrow();
    expect(() => payloadJson(withFields({ offline_days: 366 }))).not.toThrow();
    // §8.26 §4: the client's clock is signed as received, however far off.
    expect(() => payloadJson(withFields({ client_time: 1 }))).not.toThrow();
    expect(() =>
      payloadJson(withFields({ state: "ended", trial_ends_at: undefined, active_until: undefined })),
    ).not.toThrow();
  });
});

describe("the signing key", () => {
  it("is imported non-extractable", async () => {
    const key = await importLeaseKey("primary", pkcs8("primary"));
    expect(key.kid).toBe("primary");
    expect(key.key.extractable).toBe(false);
  });

  it("refuses a malformed secret or key id without quoting the secret", async () => {
    const secret = pkcs8("primary");
    for (const [kid, text] of [
      ["primary", "not base64!"],
      ["primary", ""],
      ["primary", secret.slice(0, 40)],
      ["Primary", secret],
    ] as const) {
      const err = await importLeaseKey(kid, text).then(
        () => null,
        (e: unknown) => e,
      );
      expect(err).toBeInstanceOf(LeaseError);
      expect(String((err as Error).message)).not.toContain(secret.slice(0, 20));
    }
  });
});
