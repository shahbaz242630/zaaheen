// The website's sign-in proof (AUTH-PAGES-DESIGN S85-1, ADR-SEC-043).
//
// The account page sends Clerk's short-lived session token, an RS256 JWT,
// as a Bearer token. It is checked here without a network call, against the
// instance's public key pinned in the Worker's configuration (CLERK_JWT_KEY,
// from the issuer's /.well-known/jwks.json; never fetched at run time):
// RS256 only, `iss` exactly the issuer, `azp` exactly the account origin,
// `exp`/`nbf` with 5 s of leeway, `iat` present and at most 5 minutes before
// `exp`, `sts` (when present) "active", and `sub` a Clerk user id. Anything else is
// "refused", never an error that could read as the person's fault.

export interface WebConfig {
  /** The instance's RS256 public key, as a JWK. */
  jwk: JsonWebKey;
  /** Clerk's issuer, exactly (`https://clerk.zaaheen.com`). */
  issuer: string;
  /** The one website origin allowed to use a session token. */
  origin: string;
}

export type SessionCheck = { kind: "ok"; sub: string } | { kind: "refused" };

const LEEWAY = 5;
// A Clerk session token lives about a minute; anything signed for longer (a
// JWT template, say) is not one (independent review s85, finding 2).
const MAX_LIFETIME = 300;
const USER_ID = /^user_[A-Za-z0-9]{20,40}$/;
const PART = /^[A-Za-z0-9_-]+$/;
const MAX_TOKEN = 4096;

/** Three base64url parts: a JWT. The app's opaque token is never one. */
export function looksLikeJwt(token: string): boolean {
  const parts = token.split(".");
  return parts.length === 3 && parts.every((p, i) => (i === 2 ? p === "" || PART.test(p) : PART.test(p)));
}

export async function verifySessionToken(token: string, web: WebConfig, now: number): Promise<SessionCheck> {
  const refused: SessionCheck = { kind: "refused" };
  if (token.length > MAX_TOKEN || !looksLikeJwt(token)) return refused;
  const [head, body, sig] = token.split(".") as [string, string, string];
  if (sig === "") return refused;

  const header = decodeJson(head);
  if (!isObject(header) || header["alg"] !== "RS256") return refused;
  const claims = decodeJson(body);
  if (!isObject(claims)) return refused;

  let verified = false;
  try {
    const key = await crypto.subtle.importKey("jwk", publicJwk(web.jwk), { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" }, false, ["verify"]);
    const signature = decodeBytes(sig);
    if (signature === null) return refused;
    verified = await crypto.subtle.verify("RSASSA-PKCS1-v1_5", key, signature, new TextEncoder().encode(`${head}.${body}`));
  } catch {
    return refused;
  }
  if (!verified) return refused;

  if (claims["iss"] !== web.issuer || claims["azp"] !== web.origin) return refused;
  const exp = claims["exp"];
  const nbf = claims["nbf"];
  if (typeof exp !== "number" || now > exp + LEEWAY) return refused;
  if (nbf !== undefined && (typeof nbf !== "number" || now < nbf - LEEWAY)) return refused;
  const iat = claims["iat"];
  if (typeof iat !== "number" || exp - iat > MAX_LIFETIME) return refused;
  // A session still waiting on a task (sts "pending") is not signed in yet.
  if (claims["sts"] !== undefined && claims["sts"] !== "active") return refused;
  const sub = claims["sub"];
  if (typeof sub !== "string" || !USER_ID.test(sub)) return refused;
  return { kind: "ok", sub };
}

/** Only the public parts of the configured key, so a mis-pasted private key cannot sign. */
function publicJwk(jwk: JsonWebKey): JsonWebKey {
  return { kty: "RSA", n: jwk.n ?? "", e: jwk.e ?? "", alg: "RS256", ext: true };
}

function decodeBytes(part: string): Uint8Array | null {
  try {
    const b64 = part.replace(/-/g, "+").replace(/_/g, "/") + "=".repeat((4 - (part.length % 4)) % 4);
    const bin = atob(b64);
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  } catch {
    return null;
  }
}

function decodeJson(part: string): unknown {
  const bytes = decodeBytes(part);
  if (bytes === null) return null;
  try {
    return JSON.parse(new TextDecoder("utf-8", { fatal: true, ignoreBOM: false }).decode(bytes));
  } catch {
    return null;
  }
}

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}
