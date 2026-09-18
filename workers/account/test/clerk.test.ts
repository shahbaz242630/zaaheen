// The Clerk Backend API client (SIGNIN-DESIGN.md §5 "Auth", §8.30).
// Contracts from Clerk's BAPI spec 2026-05-12: verifyOAuthAccessToken,
// GetUser, UpdateUserMetadata (deep merge; null deletes a key).
import { describe, expect, it } from "vitest";

import { ClerkClient } from "../src/clerk";
import { UpstreamError } from "../src/upstream";
import { fakeFetch, json } from "./support";

const SECRET = "sk_test_PLACEHOLDER_not_a_key";
const CLIENT_ID = "mZOxuTESTclient1";
const USER = "user_2abcdefghijklmnopqrstuvwxyz0";

function valid(fields: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    object: "clerk_idp_oauth_access_token",
    id: "oat_0ef5a7a33d87ed87ee7954c845d80450",
    client_id: CLIENT_ID,
    subject: USER,
    scopes: ["openid", "profile", "email", "offline_access"],
    revoked: false,
    revocation_reason: null,
    expired: false,
    expiration: 1_900_000_000,
    created_at: 1_800_000_000,
    updated_at: 1_800_000_000,
    ...fields,
  };
}

function client(answer: Parameters<typeof fakeFetch>[0]) {
  const f = fakeFetch(answer);
  return { clerk: new ClerkClient({ secretKey: SECRET, clientId: CLIENT_ID }, f.fetch), seen: f.seen };
}

describe("verifying an access token", () => {
  it("sends the documented request and returns the subject", async () => {
    const { clerk, seen } = client(() => json(200, valid()));
    expect(await clerk.verifyAccessToken("at_TOKEN")).toEqual({ kind: "valid", sub: USER });
    expect(seen).toHaveLength(1);
    expect(seen[0]?.method).toBe("POST");
    expect(seen[0]?.url.href).toBe("https://api.clerk.com/v1/oauth_applications/access_tokens/verify");
    expect(seen[0]?.headers.get("authorization")).toBe(`Bearer ${SECRET}`);
    expect(seen[0]?.headers.get("content-type")).toBe("application/json");
    expect(JSON.parse(seen[0]?.body ?? "")).toEqual({ access_token: "at_TOKEN" });
  });

  const refusals: Array<[string, Response]> = [
    ["another app's token", json(200, valid({ client_id: "someone_else" }))],
    ["a revoked token", json(200, valid({ revoked: true, revocation_reason: "Revoked by user" }))],
    ["an expired token", json(200, valid({ expired: true }))],
    ["an inactive JWT", json(200, { active: false })],
    ["an unknown token (404)", json(404, { errors: [{ message: "not found", long_message: "x", code: "resource_not_found" }] })],
    ["a malformed token (400)", json(400, { errors: [{ message: "bad", long_message: "x", code: "bad_request" }] })],
  ];
  for (const [name, response] of refusals) {
    it(`refuses ${name}`, async () => {
      const { clerk } = client(() => response.clone());
      expect(await clerk.verifyAccessToken("at_TOKEN")).toEqual({ kind: "refused" });
    });
  }

  const upstream: Array<[string, () => Response | "network-error"]> = [
    ["our own key rejected (401)", () => json(401, { errors: [] })],
    ["forbidden (403)", () => json(403, { errors: [] })],
    ["rate limited (429)", () => json(429, { errors: [] })],
    ["a server error (500)", () => json(500, "oops")],
    ["no network", () => "network-error"],
    ["a 200 that is not JSON", () => new Response("<html>", { status: 200 })],
    ["a 200 missing the subject", () => json(200, valid({ subject: undefined }))],
    ["a 200 with a subject that could not be a user id", () => json(200, valid({ subject: "user 2 x" }))],
  ];
  for (const [name, answer] of upstream) {
    it(`treats ${name} as an upstream failure, never a refusal`, async () => {
      const { clerk } = client(answer);
      await expect(clerk.verifyAccessToken("at_TOKEN")).rejects.toThrow(UpstreamError);
    });
  }
});

describe("reading a user", () => {
  const user = {
    id: USER,
    object: "user",
    primary_email_address_id: "idn_2",
    email_addresses: [
      { id: "idn_1", email_address: "old@example.com" },
      { id: "idn_2", email_address: "primary@example.com" },
    ],
    private_metadata: { zaaheen_memory: { trial_started_at: 1_800_000_000 } },
    banned: false,
    locked: false,
  };

  it("sends the documented request and returns the fields the Worker uses", async () => {
    const { clerk, seen } = client(() => json(200, user));
    expect(await clerk.getUser(USER)).toEqual({
      id: USER,
      primaryEmail: "primary@example.com",
      privateMetadata: user.private_metadata,
    });
    expect(seen[0]?.method).toBe("GET");
    expect(seen[0]?.url.href).toBe(`https://api.clerk.com/v1/users/${USER}`);
    expect(seen[0]?.headers.get("authorization")).toBe(`Bearer ${SECRET}`);
  });

  it("a deleted user is null", async () => {
    const { clerk } = client(() => json(404, { errors: [] }));
    expect(await clerk.getUser(USER)).toBeNull();
  });

  it("a user with no primary email still reads", async () => {
    const { clerk } = client(() => json(200, { ...user, primary_email_address_id: null }));
    expect((await clerk.getUser(USER))?.primaryEmail).toBeNull();
  });

  it("anything else is an upstream failure", async () => {
    for (const answer of [() => json(500, {}), () => json(401, {}), () => "network-error" as const, () => json(200, { id: "user_other" })]) {
      const { clerk } = client(answer);
      await expect(clerk.getUser(USER)).rejects.toThrow(UpstreamError);
    }
  });

  it("never puts an unchecked id into the URL", async () => {
    const { clerk, seen } = client(() => json(200, user));
    await expect(clerk.getUser("../users?x=1")).rejects.toThrow(UpstreamError);
    expect(seen).toHaveLength(0);
  });
});

describe("writing the record", () => {
  it("merges the patch under private_metadata.zaaheen_memory", async () => {
    const { clerk, seen } = client(() => json(200, { id: USER }));
    await clerk.mergeRecord(USER, { live_fetch_at: 1_800_000_000, checkout_at: null });
    expect(seen[0]?.method).toBe("PATCH");
    expect(seen[0]?.url.href).toBe(`https://api.clerk.com/v1/users/${USER}/metadata`);
    expect(JSON.parse(seen[0]?.body ?? "")).toEqual({
      private_metadata: { zaaheen_memory: { live_fetch_at: 1_800_000_000, checkout_at: null } },
    });
  });

  it("any failure is an upstream failure", async () => {
    for (const answer of [() => json(422, {}), () => json(500, {}), () => "network-error" as const]) {
      const { clerk } = client(answer);
      await expect(clerk.mergeRecord(USER, { live_fetch_at: 1 })).rejects.toThrow(UpstreamError);
    }
  });
});

describe("errors never carry secrets", () => {
  it("the secret key and the token appear in no error message", async () => {
    const { clerk } = client(() => json(500, { echo: SECRET }));
    const err = await clerk.verifyAccessToken("at_TOKEN_VALUE").then(
      () => null,
      (e: unknown) => e as Error,
    );
    expect(err).toBeInstanceOf(UpstreamError);
    expect(String(err?.message)).not.toContain(SECRET);
    expect(String(err?.message)).not.toContain("at_TOKEN_VALUE");
  });
});
