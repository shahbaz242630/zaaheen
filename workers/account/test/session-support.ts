// A throwaway RS256 key and Clerk-shaped session tokens for route tests
// (ADR-SEC-043). Nothing here is a real key.
import type { WebConfig } from "../src/session";

export const WEB_ISSUER = "https://clerk.example.test";
export const WEB_ORIGIN = "https://account.example.test";

const b64url = (bytes: Uint8Array): string =>
  btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
const enc = (v: unknown): string => b64url(new TextEncoder().encode(JSON.stringify(v)));

export interface SessionKit {
  web: WebConfig;
  /** A session token for `sub`, valid around `now`. */
  token(sub: string, now: number): Promise<string>;
}

export async function sessionKit(): Promise<SessionKit> {
  const pair = (await crypto.subtle.generateKey(
    { name: "RSASSA-PKCS1-v1_5", modulusLength: 2048, publicExponent: new Uint8Array([1, 0, 1]), hash: "SHA-256" },
    true,
    ["sign", "verify"],
  )) as CryptoKeyPair;
  const jwk = (await crypto.subtle.exportKey("jwk", pair.publicKey)) as JsonWebKey;
  return {
    web: { jwk: { ...jwk, kid: "ins_test", alg: "RS256", use: "sig" } as JsonWebKey, issuer: WEB_ISSUER, origin: WEB_ORIGIN },
    async token(sub, now) {
      const head = enc({ alg: "RS256", typ: "JWT", kid: "ins_test" });
      const body = enc({ iss: WEB_ISSUER, azp: WEB_ORIGIN, sub, iat: now - 5, nbf: now - 5, exp: now + 55 });
      const sig = await crypto.subtle.sign("RSASSA-PKCS1-v1_5", pair.privateKey, new TextEncoder().encode(`${head}.${body}`));
      return `${head}.${body}.${b64url(new Uint8Array(sig))}`;
    },
  };
}
