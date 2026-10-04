// The website's sign-in proof (AUTH-PAGES-DESIGN S85-1, ADR-SEC-043): a
// Clerk session token (RS256 JWT) checked without a network call against the
// instance's pinned public key. Every case here is a way to look right
// without being right.
import { beforeAll, describe, expect, it } from "vitest";

import { type WebConfig, looksLikeJwt, verifySessionToken } from "../src/session";

const NOW = 1_800_000_000;
const ISSUER = "https://clerk.example.test";
const ORIGIN = "https://account.example.test";
const USER = "user_2abcdefghijklmnopqrstuvwxyz0";

let web: WebConfig;
let ours: CryptoKeyPair;
let stranger: CryptoKeyPair;

const RSA = { name: "RSASSA-PKCS1-v1_5", modulusLength: 2048, publicExponent: new Uint8Array([1, 0, 1]), hash: "SHA-256" };

beforeAll(async () => {
  ours = (await crypto.subtle.generateKey(RSA, true, ["sign", "verify"])) as CryptoKeyPair;
  stranger = (await crypto.subtle.generateKey(RSA, true, ["sign", "verify"])) as CryptoKeyPair;
  const jwk = (await crypto.subtle.exportKey("jwk", ours.publicKey)) as JsonWebKey;
  web = { jwk: { ...jwk, kid: "ins_test", alg: "RS256", use: "sig" } as JsonWebKey, issuer: ISSUER, origin: ORIGIN };
});

const b64url = (bytes: Uint8Array): string =>
  btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
const enc = (v: unknown): string => b64url(new TextEncoder().encode(JSON.stringify(v)));

function claims(over: Record<string, unknown> = {}): Record<string, unknown> {
  return { iss: ISSUER, azp: ORIGIN, sub: USER, iat: NOW - 10, nbf: NOW - 10, exp: NOW + 50, sid: "sess_x", ...over };
}

async function token(c: Record<string, unknown> = claims(), opts: { key?: CryptoKey; header?: Record<string, unknown> } = {}): Promise<string> {
  const head = enc(opts.header ?? { alg: "RS256", typ: "JWT", kid: "ins_test" });
  const body = enc(c);
  const sig = await crypto.subtle.sign("RSASSA-PKCS1-v1_5", opts.key ?? ours.privateKey, new TextEncoder().encode(`${head}.${body}`));
  return `${head}.${body}.${b64url(new Uint8Array(sig))}`;
}

describe("a good session token", () => {
  it("is accepted and names the user Clerk signed it for", async () => {
    expect(await verifySessionToken(await token(), web, NOW)).toEqual({ kind: "ok", sub: USER });
  });

  it("is accepted within 5 s of leeway either side", async () => {
    expect((await verifySessionToken(await token(claims({ exp: NOW - 4 })), web, NOW)).kind).toBe("ok");
    expect((await verifySessionToken(await token(claims({ nbf: NOW + 4 })), web, NOW)).kind).toBe("ok");
  });
});

describe("refused", () => {
  const refused = async (t: string): Promise<void> => {
    expect(await verifySessionToken(t, web, NOW)).toEqual({ kind: "refused" });
  };

  it("minted for another website (azp)", async () => {
    await refused(await token(claims({ azp: "https://evil.test" })));
    await refused(await token(claims({ azp: `${ORIGIN}.evil.test` })));
    await refused(await token(claims({ azp: undefined })));
  });

  it("from another issuer", async () => {
    await refused(await token(claims({ iss: "https://clerk.evil.test" })));
    await refused(await token(claims({ iss: undefined })));
  });

  it("expired, or not valid yet", async () => {
    await refused(await token(claims({ exp: NOW - 6 })));
    await refused(await token(claims({ nbf: NOW + 6 })));
    await refused(await token(claims({ exp: undefined })));
  });

  it("with the wrong types for its times, or an issuer with a trailing slash (review s85)", async () => {
    await refused(await token(claims({ nbf: "now" })));
    await refused(await token(claims({ exp: String(NOW + 50) })));
    await refused(await token(claims({ iss: `${ISSUER}/` })));
  });

  it("signed for longer than a session token lives, or with no iat (review s85, finding 2)", async () => {
    await refused(await token(claims({ iat: NOW - 10, exp: NOW + 400 })));
    await refused(await token(claims({ iat: undefined })));
  });

  it("for a session still waiting on a task (sts pending)", async () => {
    await refused(await token(claims({ sts: "pending" })));
    expect((await verifySessionToken(await token(claims({ sts: "active" })), web, NOW)).kind).toBe("ok");
  });

  it("signed by a key that is not ours", async () => {
    await refused(await token(claims(), { key: stranger.privateKey }));
  });

  it("claiming another algorithm", async () => {
    await refused(await token(claims(), { header: { alg: "HS256", typ: "JWT" } }));
    const head = enc({ alg: "none", typ: "JWT" });
    await refused(`${head}.${enc(claims())}.`);
  });

  it("with a body changed after signing", async () => {
    const [h, , s] = (await token()).split(".");
    await refused(`${h}.${enc(claims({ sub: "user_2zzzzzzzzzzzzzzzzzzzzzzzzzz" }))}.${s}`);
  });

  it("with a subject that is not a user id", async () => {
    await refused(await token(claims({ sub: "../admin" })));
    await refused(await token(claims({ sub: 42 })));
  });

  it("that is not a JWT at all", async () => {
    for (const t of ["", "abc", "a.b", "a.b.c.d", "!!.!!.!!", "oat_opaque_token_value"]) await refused(t);
  });
});

describe("telling the two kinds of token apart by shape", () => {
  it("a JWT has three base64url parts; the app's opaque token does not", async () => {
    expect(looksLikeJwt(await token())).toBe(true);
    expect(looksLikeJwt("at_TOKEN")).toBe(false);
    expect(looksLikeJwt("oat_2abc")).toBe(false);
    expect(looksLikeJwt("a.b")).toBe(false);
  });
});
