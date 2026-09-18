// The signed entitlement lease (SIGNIN-DESIGN.md §4, contracts in §8.27).
//
//   wire = b64url(payload) "." b64url(sig)
//   sig  = Ed25519(sk, "zaaheen-lease-v1\0" || payload_bytes)
//
// vault-account verifies the exact payload bytes before parsing them, so the
// JSON written here is the contract: fixed field order, absent deadlines
// omitted (never null). Every rule the app's verifier enforces is enforced
// here first, because a lease the app refuses locks its user out.

export const LEASE_DOMAIN = "zaaheen-lease-v1\0";
export const MAX_LEASE_BYTES = 4096;
export const MAX_OFFLINE_DAYS = 366;

const KID = /^[a-z0-9-]{1,32}$/;
const SUB = /^[\x21-\x7e]{1,256}$/;
const STATES = new Set(["trial", "active", "payment_failed", "ended"]);

export type LeaseState = "trial" | "active" | "payment_failed" | "ended";

export interface LeasePayload {
  v: 1;
  kid: string;
  sub: string;
  state: LeaseState;
  trial_ends_at?: number;
  active_until?: number;
  issued_at: number;
  client_time: number;
  offline_days: number;
}

/** A private signing key, with the `kid` its leases carry. */
export interface LeaseSigningKey {
  kid: string;
  key: CryptoKey;
}

/** A lease that must not be signed, or a key that cannot be used. Messages
 * are fixed text: nothing from the payload or the secret is quoted. */
export class LeaseError extends Error {
  override name = "LeaseError";
}

/**
 * Import a signing key from its secret: PKCS#8 DER, standard base64 (what
 * `openssl genpkey -algorithm ed25519` and Node's key export produce).
 * The key is non-extractable.
 */
export async function importLeaseKey(kid: string, pkcs8Base64: string): Promise<LeaseSigningKey> {
  if (!KID.test(kid)) throw new LeaseError("lease key id must be 1-32 characters of a-z 0-9 -");
  let der: Uint8Array;
  try {
    der = Uint8Array.from(atob(pkcs8Base64), (c) => c.charCodeAt(0));
  } catch {
    throw new LeaseError("lease signing key is not base64");
  }
  if (der.length === 0) throw new LeaseError("lease signing key is empty");
  try {
    const key = await crypto.subtle.importKey("pkcs8", der, { name: "Ed25519" }, false, ["sign"]);
    return { kid, key };
  } catch {
    throw new LeaseError("lease signing key is not an Ed25519 PKCS#8 key");
  }
}

/** The payload's exact JSON text, after checking it against the app's rules. */
export function payloadJson(p: LeasePayload): string {
  check(p);
  // Written field by field so the order never depends on how `p` was built.
  const ordered: Record<string, unknown> = {
    v: p.v,
    kid: p.kid,
    sub: p.sub,
    state: p.state,
  };
  if (p.trial_ends_at !== undefined) ordered["trial_ends_at"] = p.trial_ends_at;
  if (p.active_until !== undefined) ordered["active_until"] = p.active_until;
  ordered["issued_at"] = p.issued_at;
  ordered["client_time"] = p.client_time;
  ordered["offline_days"] = p.offline_days;
  return JSON.stringify(ordered);
}

/** Sign `p` with `signer` and return the wire form. */
export async function signLease(p: LeasePayload, signer: LeaseSigningKey): Promise<string> {
  if (p.kid !== signer.kid) throw new LeaseError("payload kid does not name the signing key");
  const payload = new TextEncoder().encode(payloadJson(p));
  const domain = new TextEncoder().encode(LEASE_DOMAIN);
  const message = new Uint8Array(domain.length + payload.length);
  message.set(domain, 0);
  message.set(payload, domain.length);
  const sig = new Uint8Array(await crypto.subtle.sign({ name: "Ed25519" }, signer.key, message));
  const wire = `${base64url(payload)}.${base64url(sig)}`;
  if (wire.length > MAX_LEASE_BYTES) throw new LeaseError("lease too large");
  return wire;
}

function check(p: LeasePayload): void {
  const raw = p as unknown as Record<string, unknown>;
  if (raw["v"] !== 1) throw new LeaseError("lease version must be 1");
  if (typeof raw["kid"] !== "string" || !KID.test(raw["kid"])) throw new LeaseError("bad kid");
  if (typeof raw["sub"] !== "string" || !SUB.test(raw["sub"])) throw new LeaseError("bad sub");
  if (typeof raw["state"] !== "string" || !STATES.has(raw["state"])) throw new LeaseError("bad state");
  for (const name of ["issued_at", "client_time"]) {
    const t = raw[name];
    if (!Number.isSafeInteger(t) || (t as number) <= 0) throw new LeaseError(`${name} must be a positive integer`);
  }
  for (const name of ["trial_ends_at", "active_until"]) {
    const t = raw[name];
    if (t !== undefined && !Number.isSafeInteger(t)) throw new LeaseError(`${name} must be an integer`);
  }
  const days = raw["offline_days"];
  if (!Number.isSafeInteger(days) || (days as number) < 1 || (days as number) > MAX_OFFLINE_DAYS) {
    throw new LeaseError("offline_days out of range");
  }
  const hasDeadline =
    p.state === "trial"
      ? p.trial_ends_at !== undefined
      : p.state === "ended" || p.active_until !== undefined;
  if (!hasDeadline) throw new LeaseError("lease state is missing its deadline");
}

function base64url(bytes: Uint8Array): string {
  let binary = "";
  for (const b of bytes) binary += String.fromCharCode(b);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}
