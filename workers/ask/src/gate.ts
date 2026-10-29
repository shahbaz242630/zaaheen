// Who may ask (ADR-SEC-052 §3.3): a Turnstile check before the first
// question, then a short session we sign, so follow-ups need no new
// challenge. Nothing here knows who the visitor is.

export const SITEVERIFY = "https://challenges.cloudflare.com/turnstile/v0/siteverify";

/** The only host a Turnstile token may have been solved on. */
export const HOST = "ask.zaaheen.com";

/** How long a session lasts after a passed check. */
export const SESSION_MS = 30 * 60 * 1000;

const MAX_TOKEN_CHARS = 2048;

export type Fetcher = (url: string, init?: RequestInit) => Promise<Response>;

/** True only when Cloudflare accepts the token as solved on our host. Any doubt is a refusal. */
export async function verifyTurnstile(token: string, address: string, secret: string, fetcher: Fetcher): Promise<boolean> {
  if (token === "" || token.length > MAX_TOKEN_CHARS) return false;
  try {
    const response = await fetcher(SITEVERIFY, {
      method: "POST",
      body: new URLSearchParams({ secret, response: token, remoteip: address }),
    });
    if (!response.ok) return false;
    const result: unknown = await response.json();
    if (typeof result !== "object" || result === null) return false;
    const { success, hostname } = result as Record<string, unknown>;
    return success === true && hostname === HOST;
  } catch {
    return false;
  }
}

/**
 * A session: v1.<expiry ms>.<random>.<HMAC of the first three parts and the
 * visitor>. Bound to the visitor (budget.ts visitorKey: their network and
 * the day), so one passed check cannot be spent from many addresses.
 */
export async function issueSession(now: number, secret: string, visitor: string): Promise<string> {
  const nonce = hex(crypto.getRandomValues(new Uint8Array(16)));
  const body = `v1.${now + SESSION_MS}.${nonce}`;
  const mac = new Uint8Array(
    await crypto.subtle.sign("HMAC", await hmacKey(secret), new TextEncoder().encode(`${body}\n${visitor}`)),
  );
  return `${body}.${hex(mac)}`;
}

export async function verifySession(token: string, now: number, secret: string, visitor: string): Promise<boolean> {
  const parts = token.split(".");
  if (parts.length !== 4) return false;
  const [version, expiry, nonce, mac] = parts as [string, string, string, string];
  if (version !== "v1" || !/^\d{1,15}$/.test(expiry) || !/^[0-9a-f]{32}$/.test(nonce) || !/^[0-9a-f]{64}$/.test(mac)) {
    return false;
  }
  // crypto.subtle.verify compares in constant time.
  const valid = await crypto.subtle.verify(
    "HMAC",
    await hmacKey(secret),
    unhex(mac),
    new TextEncoder().encode(`${version}.${expiry}.${nonce}\n${visitor}`),
  );
  return valid && Number(expiry) >= now;
}

function hmacKey(secret: string): Promise<CryptoKey> {
  return crypto.subtle.importKey("raw", new TextEncoder().encode(secret), { name: "HMAC", hash: "SHA-256" }, false, [
    "sign",
    "verify",
  ]);
}

function hex(bytes: Uint8Array): string {
  return [...bytes].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

function unhex(text: string): Uint8Array {
  return new Uint8Array(text.match(/../g)?.map((pair) => parseInt(pair, 16)) ?? []);
}
