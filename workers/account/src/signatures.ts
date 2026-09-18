// Webhook signatures.
//
// Paddle (developer.paddle.com, "Verify webhook signatures"):
//   Paddle-Signature: ts=<unix seconds>;h1=<hex HMAC-SHA256>[;h1=...]
//   h1 = HMAC-SHA256(secret, "<ts>:<raw body>"); several h1 values appear
//   while a secret is being rotated; a 5-second tolerance on ts.
// §5 locks "any h1, 5 s tolerance". The comparison is WebCrypto's
// `verify`, which is constant-time.

const PADDLE_TOLERANCE_SECONDS = 5;

// Svix, which signs Clerk's webhooks (docs.svix.com, "Verifying webhooks
// manually"): HMAC-SHA256 over "<svix-id>.<svix-timestamp>.<raw body>",
// keyed with the base64 part of the `whsec_` secret; `svix-signature` is a
// space-separated list of "v1,<base64>" (several during rotation). The
// 5-minute tolerance is Svix's own libraries' default (§8.30).
const SVIX_TOLERANCE_SECONDS = 5 * 60;

export interface SvixHeaders {
  id: string | null;
  timestamp: string | null;
  signature: string | null;
}

export async function verifySvixSignature(h: SvixHeaders, rawBody: string, secret: string, now: number): Promise<boolean> {
  if (h.id === null || h.timestamp === null || h.signature === null) return false;
  if (!/^[\x21-\x7e]{1,256}$/.test(h.id) || !/^\d{1,12}$/.test(h.timestamp)) return false;
  if (Math.abs(now - Number(h.timestamp)) > SVIX_TOLERANCE_SECONDS) return false;
  if (!secret.startsWith("whsec_")) return false;
  let keyBytes: Uint8Array;
  try {
    keyBytes = base64Bytes(secret.slice("whsec_".length));
  } catch {
    return false;
  }
  const signatures: Uint8Array[] = [];
  for (const entry of h.signature.split(" ")) {
    const comma = entry.indexOf(",");
    if (comma < 0 || entry.slice(0, comma) !== "v1") continue;
    try {
      signatures.push(base64Bytes(entry.slice(comma + 1)));
    } catch {
      continue;
    }
  }
  if (signatures.length === 0) return false;

  const key = await crypto.subtle.importKey("raw", keyBytes, { name: "HMAC", hash: "SHA-256" }, false, ["verify"]);
  const message = new TextEncoder().encode(`${h.id}.${h.timestamp}.${rawBody}`);
  for (const sig of signatures) {
    if (await crypto.subtle.verify("HMAC", key, sig, message)) return true;
  }
  return false;
}

function base64Bytes(text: string): Uint8Array {
  return Uint8Array.from(atob(text), (c) => c.charCodeAt(0));
}

export async function verifyPaddleSignature(header: string | null, rawBody: string, secret: string, now: number): Promise<boolean> {
  if (header === null) return false;
  let ts: string | null = null;
  const signatures: Uint8Array[] = [];
  for (const part of header.split(";")) {
    const eq = part.indexOf("=");
    if (eq < 0) return false;
    const name = part.slice(0, eq);
    const value = part.slice(eq + 1);
    if (name === "ts") {
      if (ts !== null || !/^\d{1,12}$/.test(value)) return false;
      ts = value;
    } else if (name === "h1") {
      if (!/^[0-9a-f]{64}$/.test(value)) return false;
      signatures.push(hexBytes(value));
    }
  }
  if (ts === null || signatures.length === 0) return false;
  if (Math.abs(now - Number(ts)) > PADDLE_TOLERANCE_SECONDS) return false;

  const key = await crypto.subtle.importKey("raw", new TextEncoder().encode(secret), { name: "HMAC", hash: "SHA-256" }, false, ["verify"]);
  const message = new TextEncoder().encode(`${ts}:${rawBody}`);
  for (const sig of signatures) {
    if (await crypto.subtle.verify("HMAC", key, sig, message)) return true;
  }
  return false;
}

function hexBytes(hex: string): Uint8Array {
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i += 1) out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  return out;
}
