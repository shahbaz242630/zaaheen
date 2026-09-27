// The Clerk Backend API, as the Worker uses it (SIGNIN-DESIGN.md §5):
//
//   POST  /v1/oauth_applications/access_tokens/verify   {access_token}
//   GET   /v1/users/{user_id}
//   GET   /v1/oauth_applications?limit=100  (the sweep's instance check)
//   PATCH /v1/users/{user_id}/metadata   {private_metadata}  (deep merge;
//                                         null deletes a key)
//
// Shapes are from Clerk's BAPI spec 2026-05-12 (verifyOAuthAccessToken,
// GetUser, UpdateUserMetadata) and the S0 spike (§15 row "extra").
//
// A token is "refused" only on a definitive answer about the token: another
// app's client id, revoked, expired, an inactive JWT, 400 or 404. Our own
// key being refused (401/403), 429, 5xx, no network or an unreadable 200 is
// an UpstreamError, so a Worker-side fault can never look like a user's
// token being bad.

import { RECORD_KEY } from "./record";
import type { RecordPatch } from "./record";
import { type Fetch, UpstreamError, isObject, readJson, send } from "./upstream";

const API = "https://api.clerk.com/v1";
/** Clerk user ids as they appear in a path: `user_` plus word characters. */
const USER_ID = /^user_[A-Za-z0-9]{1,64}$/;

export interface ClerkConfig {
  secretKey: string;
  clientId: string;
}

export type TokenCheck = { kind: "valid"; sub: string } | { kind: "refused" };

export interface ClerkUser {
  id: string;
  primaryEmail: string | null;
  privateMetadata: unknown;
}

export class ClerkClient {
  constructor(
    private readonly config: ClerkConfig,
    private readonly fetch: Fetch,
  ) {}

  async verifyAccessToken(token: string): Promise<TokenCheck> {
    const what = "clerk verify";
    const response = await send(this.fetch, what, `${API}/oauth_applications/access_tokens/verify`, {
      method: "POST",
      headers: this.headers({ "content-type": "application/json" }),
      body: JSON.stringify({ access_token: token }),
    });
    if (response.status === 400 || response.status === 404) return { kind: "refused" };
    if (response.status !== 200) throw new UpstreamError(`${what}: status ${response.status}`);
    const body = await readJson(what, response);
    if (!isObject(body)) throw new UpstreamError(`${what}: unexpected body`);
    if (body["active"] === false) return { kind: "refused" };
    const { client_id, subject, revoked, expired } = body;
    if (typeof client_id !== "string" || typeof revoked !== "boolean" || typeof expired !== "boolean") {
      throw new UpstreamError(`${what}: unexpected body`);
    }
    if (client_id !== this.config.clientId || revoked || expired) return { kind: "refused" };
    if (typeof subject !== "string" || !USER_ID.test(subject)) throw new UpstreamError(`${what}: unexpected subject`);
    return { kind: "valid", sub: subject };
  }

  /** The user, or `null` if Clerk has no such user. */
  async getUser(userId: string): Promise<ClerkUser | null> {
    const what = "clerk get user";
    if (!USER_ID.test(userId)) throw new UpstreamError(`${what}: bad user id`);
    const response = await send(this.fetch, what, `${API}/users/${userId}`, { method: "GET", headers: this.headers({}) });
    if (response.status === 404) return null;
    if (response.status !== 200) throw new UpstreamError(`${what}: status ${response.status}`);
    const body = await readJson(what, response);
    if (!isObject(body) || body["id"] !== userId) throw new UpstreamError(`${what}: unexpected body`);
    return { id: userId, primaryEmail: primaryEmail(body), privateMetadata: body["private_metadata"] };
  }

  /**
   * Whether the secret key's instance lists our OAuth application (session 68,
   * review M1): a positive proof that the key is the right instance's, asked
   * before the sweep cancels anything. Throws on any failure.
   */
  async instanceHasOurApp(): Promise<boolean> {
    const what = "clerk list oauth applications";
    const response = await send(this.fetch, what, `${API}/oauth_applications?limit=100`, { method: "GET", headers: this.headers({}) });
    if (response.status !== 200) throw new UpstreamError(`${what}: status ${response.status}`);
    const body = await readJson(what, response);
    if (!isObject(body) || !Array.isArray(body["data"])) throw new UpstreamError(`${what}: unexpected body`);
    return body["data"].some((app) => isObject(app) && app["client_id"] === this.config.clientId);
  }

  /** Deep-merge `patch` into `private_metadata.zaaheen_memory`. */
  async mergeRecord(userId: string, patch: RecordPatch): Promise<void> {
    const what = "clerk merge metadata";
    if (!USER_ID.test(userId)) throw new UpstreamError(`${what}: bad user id`);
    const response = await send(this.fetch, what, `${API}/users/${userId}/metadata`, {
      method: "PATCH",
      headers: this.headers({ "content-type": "application/json" }),
      body: JSON.stringify({ private_metadata: { [RECORD_KEY]: patch } }),
    });
    if (response.status !== 200) throw new UpstreamError(`${what}: status ${response.status}`);
  }

  /**
   * Deep-merge top-level `private_metadata` keys other than our record
   * (AUTH-PAGES-DESIGN D6's `marketing`). `false` when Clerk has no such
   * user (404); throws on any other failure.
   */
  async mergePrivate(userId: string, fields: Record<string, unknown>): Promise<boolean> {
    const what = "clerk merge private metadata";
    if (!USER_ID.test(userId)) throw new UpstreamError(`${what}: bad user id`);
    if (RECORD_KEY in fields) throw new UpstreamError(`${what}: the record has its own writer`);
    const response = await send(this.fetch, what, `${API}/users/${userId}/metadata`, {
      method: "PATCH",
      headers: this.headers({ "content-type": "application/json" }),
      body: JSON.stringify({ private_metadata: fields }),
    });
    if (response.status === 404) return false;
    if (response.status !== 200) throw new UpstreamError(`${what}: status ${response.status}`);
    return true;
  }

  private headers(extra: Record<string, string>): Record<string, string> {
    return { authorization: `Bearer ${this.config.secretKey}`, ...extra };
  }
}

function primaryEmail(user: Record<string, unknown>): string | null {
  const id = user["primary_email_address_id"];
  const list = user["email_addresses"];
  if (typeof id !== "string" || !Array.isArray(list)) return null;
  for (const entry of list) {
    if (isObject(entry) && entry["id"] === id && typeof entry["email_address"] === "string") return entry["email_address"];
  }
  return null;
}
