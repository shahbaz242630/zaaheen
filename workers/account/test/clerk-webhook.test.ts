// POST /clerk/webhook (SIGNIN-DESIGN.md §5, §8.30), quoted:
//   (Svix-verified): user.deleted -> page through subscriptions with status
//   active/past_due/paused and our price IDs, match
//   custom_data.clerk_user_id, cancel immediately. Failure -> 5xx (Svix
//   retries). Runbook backup.
// Svix signing (docs.svix.com, "Verifying webhooks manually"): HMAC-SHA256
// over "<svix-id>.<svix-timestamp>.<body>", keyed with the base64 part of
// the whsec_ secret; svix-signature is a space-separated list of "v1,<b64>".
import { describe, expect, it } from "vitest";

import type { Config } from "../src/config";
import { MARKETING_WORDING_VERSION, handleClerkWebhook } from "../src/routes/clerk-webhook";
import { type Seen, fakeFetch, json, rfcPkcs8 } from "./support";

const T = 1_800_000_000;
const DELETED = "user_2deletedxxxxxxxxxxxxxxxxxx";
const SOMEONE = "user_2someoneelsexxxxxxxxxxxxxx";
const MONTHLY = "pri_01bbbbbbbbbbbbbbbbbbbbbbbb";
const ANNUAL = "pri_01cccccccccccccccccccccccc";
// 32 bytes of test key material, base64: not a real Clerk secret.
const SECRET = `whsec_${btoa(String.fromCharCode(...Array.from({ length: 32 }, (_, i) => i * 7 + 1)))}`;

const config: Config = {
  clerk: { secretKey: "sk_test_SECRET", clientId: "client_ours", webhookSecret: SECRET },
  paddle: {
    apiKey: "pdl_sdbx_apikey_SECRET",
    environment: "sandbox",
    productId: "pro_01aaaaaaaaaaaaaaaaaaaaaaaa",
    prices: { monthly: MONTHLY, annual: ANNUAL },
    webhookSecret: "pdl_ntfset_test",
  },
  lease: { kid: "primary", pkcs8: rfcPkcs8("primary") },
  trials: { key: "k".repeat(32) },
  killSwitch: false,
};

function sub(id: string, userId: string | null, status = "active"): unknown {
  return { id, status, customer_id: "ctm_01h8441jn5pcwrfhwh78jqt8hk", custom_data: userId === null ? null : { clerk_user_id: userId } };
}

function event(type = "user.deleted", id: string = DELETED): string {
  return JSON.stringify({ data: { id, object: "user", deleted: true }, object: "event", type, timestamp: T * 1000 });
}

async function svixSignature(id: string, ts: number, body: string, secret = SECRET): Promise<string> {
  const keyBytes = Uint8Array.from(atob(secret.slice("whsec_".length)), (c) => c.charCodeAt(0));
  const key = await crypto.subtle.importKey("raw", keyBytes, { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const mac = new Uint8Array(await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(`${id}.${ts}.${body}`)));
  return `v1,${btoa(String.fromCharCode(...mac))}`;
}

interface World {
  pages?: unknown[][];
  listDown?: boolean;
  cancelFails?: boolean;
  /** Status Clerk answers the metadata write with (default 200). */
  metadataStatus?: number;
}

function world(w: World) {
  const f = fakeFetch((req: Seen) => {
    const { pathname, origin } = req.url;
    if (origin === "https://sandbox-api.paddle.com" && pathname === "/subscriptions" && req.method === "GET") {
      if (w.listDown) return json(502, {});
      const pages = w.pages ?? [[]];
      const index = Number(req.url.searchParams.get("after")?.replace("page", "") ?? "0");
      const hasMore = index + 1 < pages.length;
      const next = hasMore ? `https://sandbox-api.paddle.com/subscriptions?after=page${index + 1}` : null;
      return json(200, { data: pages[index] ?? [], meta: { pagination: { has_more: hasMore, next } } });
    }
    if (origin === "https://sandbox-api.paddle.com" && /^\/subscriptions\/sub_[a-z0-9]+\/cancel$/.test(pathname)) {
      return w.cancelFails ? json(500, {}) : json(200, { data: { status: "canceled" } });
    }
    const meta = /^\/v1\/users\/(user_[A-Za-z0-9]+)\/metadata$/.exec(pathname);
    if (origin === "https://api.clerk.com" && meta && req.method === "PATCH") {
      const status = w.metadataStatus ?? 200;
      return json(status, status === 200 ? { id: meta[1] } : {});
    }
    return json(418, { unexpected: req.url.href });
  });
  return f;
}

async function call(w: World, body: string, headers: Record<string, string> | null, now = T) {
  const f = world(w);
  const request = new Request("https://api.zaaheen.com/clerk/webhook", {
    method: "POST",
    headers: { "content-type": "application/json", ...(headers ?? {}) },
    body,
  });
  const response = await handleClerkWebhook(request, config, { fetch: f.fetch, now: () => now });
  return { response, seen: f.seen };
}

async function signed(body: string, ts = T, id = "msg_2test"): Promise<Record<string, string>> {
  return { "svix-id": id, "svix-timestamp": String(ts), "svix-signature": await svixSignature(id, ts, body) };
}

const cancels = (seen: Seen[]) => seen.filter((r) => r.url.pathname.endsWith("/cancel")).map((r) => r.url.pathname.split("/")[2]);

describe("a genuine user.deleted", () => {
  it("lists our live subscriptions and cancels exactly the deleted user's, immediately", async () => {
    const pages = [[sub("sub_01aaaaaaaaaaaaaaaaaaaaaaaa", SOMEONE), sub("sub_01bbbbbbbbbbbbbbbbbbbbbbbb", DELETED)], [sub("sub_01cccccccccccccccccccccccc", DELETED, "paused"), sub("sub_01dddddddddddddddddddddddd", null)]];
    const body = event();
    const { response, seen } = await call({ pages }, body, await signed(body));
    expect(response.status).toBe(200);
    const list = seen[0];
    expect(list?.url.searchParams.get("price_id")).toBe(`${MONTHLY},${ANNUAL}`);
    expect(list?.url.searchParams.get("status")).toBe("active,past_due,paused");
    expect(cancels(seen)).toEqual(["sub_01bbbbbbbbbbbbbbbbbbbbbbbb", "sub_01cccccccccccccccccccccccc"]);
    const cancel = seen.find((r) => r.url.pathname.endsWith("/cancel"));
    expect(cancel?.method).toBe("POST");
    expect(JSON.parse(cancel?.body ?? "")).toEqual({ effective_from: "immediately" });
  });

  it("a user with no subscription: 200, nothing cancelled", async () => {
    const body = event();
    const { response, seen } = await call({ pages: [[sub("sub_01aaaaaaaaaaaaaaaaaaaaaaaa", SOMEONE)]] }, body, await signed(body));
    expect(response.status).toBe(200);
    expect(cancels(seen)).toEqual([]);
  });

  it("accepts any v1 signature in the list (secret rotation) and ignores other versions", async () => {
    const body = event();
    const h = await signed(body);
    h["svix-signature"] = `v2,AAAA v1,${btoa("x".repeat(32))} ${h["svix-signature"]}`;
    expect((await call({}, body, h)).response.status).toBe(200);
  });

  it("accepts a timestamp up to 5 minutes away, either side", async () => {
    const body = event();
    expect((await call({}, body, await signed(body, T - 300))).response.status).toBe(200);
    expect((await call({}, body, await signed(body, T + 300))).response.status).toBe(200);
  });
});

describe("anything not provably from Clerk is refused (401) before any other call", () => {
  const cases: Array<[string, () => Promise<[string, Record<string, string> | null]>]> = [
    ["no Svix headers", async () => [event(), null]],
    ["a signature under another secret", async () => {
      const body = event();
      const h = await signed(body);
      const other = `whsec_${btoa("y".repeat(32))}`;
      h["svix-signature"] = await svixSignature("msg_2test", T, body, other);
      return [body, h];
    }],
    ["a body changed after signing", async () => [event("user.deleted", SOMEONE), await signed(event())]],
    ["a different message id", async () => {
      const body = event();
      const h = await signed(body);
      h["svix-id"] = "msg_2other";
      return [body, h];
    }],
    ["a timestamp 301 seconds old", async () => [event(), await signed(event(), T - 301)]],
    ["a non-numeric timestamp", async () => {
      const h = await signed(event());
      h["svix-timestamp"] = "soon";
      return [event(), h];
    }],
    ["only a v2 signature", async () => {
      const h = await signed(event());
      h["svix-signature"] = h["svix-signature"]?.replace(/^v1,/, "v2,") ?? "";
      return [event(), h];
    }],
  ];
  for (const [name, make] of cases) {
    it(name, async () => {
      const [body, headers] = await make();
      const { response, seen } = await call({}, body, headers);
      expect(response.status).toBe(401);
      expect(seen).toHaveLength(0);
    });
  }
});

describe("genuine events with nothing to do: 200", () => {
  it("another event type", async () => {
    const body = event("user.updated");
    const { response, seen } = await call({}, body, await signed(body));
    expect(response.status).toBe(200);
    expect(seen).toHaveLength(0);
  });

  it("a user.deleted without a usable id", async () => {
    const body = event("user.deleted", "not-a-user-id");
    const { response, seen } = await call({}, body, await signed(body));
    expect(response.status).toBe(200);
    expect(seen).toHaveLength(0);
  });
});

describe("failures ask Svix to retry (5xx)", () => {
  it("Paddle down while listing", async () => {
    const body = event();
    expect((await call({ listDown: true }, body, await signed(body))).response.status).toBe(503);
  });

  it("a cancel failing", async () => {
    const body = event();
    expect((await call({ cancelFails: true, pages: [[sub("sub_01bbbbbbbbbbbbbbbbbbbbbbbb", DELETED)]] }, body, await signed(body))).response.status).toBe(503);
  });
});

describe("the request itself", () => {
  it("only POST", async () => {
    const f = world({});
    const response = await handleClerkWebhook(new Request("https://api.zaaheen.com/clerk/webhook"), config, { fetch: f.fetch, now: () => T });
    expect(response.status).toBe(405);
  });

  it("a body over 64 KB is 413, without checking anything else", async () => {
    const body = JSON.stringify({ type: "user.deleted", data: { id: DELETED }, pad: "x".repeat(70_000) });
    const { response, seen } = await call({}, body, await signed(body));
    expect(response.status).toBe(413);
    expect(seen).toHaveLength(0);
  });
});

// AUTH-PAGES-DESIGN D6: the sign-up's unticked box travels as
// unsafe_metadata.marketing_consent, which the person can write, so it is only
// the carrier. On user.created the Worker copies a true into
// private_metadata.marketing with the server's time and the wording's version;
// the dashboard reads only that copy. Anything but exactly true copies nothing.
describe("user.created: the marketing consent copy (AUTH-PAGES-DESIGN D6)", () => {
  const NEW = "user_2newsignupxxxxxxxxxxxxxxxxx";
  const created = (unsafe: unknown, id: string = NEW) =>
    JSON.stringify({ data: { id, object: "user", unsafe_metadata: unsafe }, object: "event", type: "user.created", timestamp: T * 1000 });

  it("a ticked box is copied into private metadata with the server's time and the wording version", async () => {
    const body = created({ marketing_consent: true });
    const { response, seen } = await call({}, body, await signed(body));
    expect(response.status).toBe(200);
    expect(seen).toHaveLength(1);
    expect(seen[0]?.method).toBe("PATCH");
    expect(seen[0]?.url.href).toBe(`https://api.clerk.com/v1/users/${NEW}/metadata`);
    expect(JSON.parse(seen[0]?.body ?? "")).toEqual({
      private_metadata: { marketing: { consent: true, at: T, wording: MARKETING_WORDING_VERSION } },
    });
  });

  it("an unticked box, a missing one, or anything but exactly true writes nothing", async () => {
    for (const unsafe of [{ marketing_consent: false }, {}, null, { marketing_consent: "true" }, { marketing_consent: 1 }, "x"]) {
      const body = created(unsafe);
      const { response, seen } = await call({}, body, await signed(body));
      expect(response.status).toBe(200);
      expect(seen).toHaveLength(0);
    }
  });

  it("a user.created without a usable id writes nothing", async () => {
    const body = created({ marketing_consent: true }, "not-a-user-id");
    const { response, seen } = await call({}, body, await signed(body));
    expect(response.status).toBe(200);
    expect(seen).toHaveLength(0);
  });

  it("a user already gone (404) is done; any other failure asks Svix to retry", async () => {
    const body = created({ marketing_consent: true });
    expect((await call({ metadataStatus: 404 }, body, await signed(body))).response.status).toBe(200);
    for (const status of [401, 429, 500]) {
      expect((await call({ metadataStatus: status }, body, await signed(body))).response.status).toBe(503);
    }
  });

  it("an unsigned user.created is refused before any write", async () => {
    const body = created({ marketing_consent: true });
    const { response, seen } = await call({}, body, null);
    expect(response.status).toBe(401);
    expect(seen).toHaveLength(0);
  });

  it("the wording version is pinned", () => {
    expect(MARKETING_WORDING_VERSION).toBe("tips-2026-09");
  });
});
