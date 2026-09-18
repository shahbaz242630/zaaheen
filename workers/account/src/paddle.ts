// The Paddle Billing API, as the Worker uses it (SIGNIN-DESIGN.md §5).
//
//   GET /subscriptions?customer_id=ctm_...&per_page=200   (+ meta.pagination.next)
//
// §1 (reviewer-verified): list-subscriptions filters by customer_id, status
// and price_id. Pages are followed only on Paddle's own API host, so the key
// can never be sent elsewhere, and at most MAX_PAGES are read: a partial
// list could hide a payer's subscription, so running out is an
// UpstreamError, never a derivation from what was read.

import { type Fetch, UpstreamError, isObject, readJson, send } from "./upstream";

export type PaddleEnvironment = "sandbox" | "live";

const API: Record<PaddleEnvironment, string> = {
  sandbox: "https://sandbox-api.paddle.com",
  live: "https://api.paddle.com",
};
const CUSTOMER_ID = /^ctm_[a-z0-9]{26}$/;
const PRICE_ID = /^pri_[a-z0-9]{26}$/;
const SUBSCRIPTION_ID = /^sub_[a-z0-9]{26}$/;
/** One customer's subscriptions: a handful at most. */
const MAX_PAGES = 5;
/**
 * Every live subscription of ours (the Clerk webhook). 20 pages of 200 is
 * 4,000 subscriptions and 20 of the free plan's 50 subrequests; §5 moves to
 * Workers Paid at about 30 paying customers, long before that.
 */
const MAX_PAGES_ALL = 20;

export interface PaddleConfig {
  apiKey: string;
  environment: PaddleEnvironment;
}

export class PaddleClient {
  constructor(
    private readonly config: PaddleConfig,
    private readonly fetch: Fetch,
  ) {}

  /** Every subscription of this customer, as Paddle returned them. */
  async listSubscriptions(customerId: string): Promise<unknown[]> {
    const what = "paddle list subscriptions";
    if (!CUSTOMER_ID.test(customerId)) throw new UpstreamError(`${what}: bad customer id`);
    return this.listAll(what, `${this.base()}/subscriptions?customer_id=${customerId}&per_page=200`, MAX_PAGES);
  }

  /**
   * Every subscription on one of `priceIds` whose status is one of
   * `statuses`, across customers (for `/clerk/webhook`, §5). Up to
   * MAX_PAGES_ALL pages of 200.
   */
  async listOurSubscriptions(priceIds: readonly string[], statuses: readonly string[]): Promise<unknown[]> {
    const what = "paddle list our subscriptions";
    return this.listAll(what, this.ourSubscriptionsUrl(what, priceIds, statuses), MAX_PAGES_ALL);
  }

  /**
   * The first page (up to 200) of the same list, for the daily sweep (§5:
   * "each run handles one page"). A sweep may see a partial list; a
   * derivation never does.
   */
  async listOurSubscriptionsFirstPage(priceIds: readonly string[], statuses: readonly string[]): Promise<unknown[]> {
    const what = "paddle list our subscriptions (first page)";
    return this.listAll(what, this.ourSubscriptionsUrl(what, priceIds, statuses), 1, { partialOk: true });
  }

  private ourSubscriptionsUrl(what: string, priceIds: readonly string[], statuses: readonly string[]): string {
    if (priceIds.some((id) => !PRICE_ID.test(id)) || statuses.some((s) => !/^[a-z_]+$/.test(s))) {
      throw new UpstreamError(`${what}: bad filter`);
    }
    return `${this.base()}/subscriptions?price_id=${priceIds.join(",")}&status=${statuses.join(",")}&per_page=200`;
  }

  /** Cancel a subscription at once (`effective_from: immediately`). */
  async cancelSubscription(subscriptionId: string): Promise<void> {
    const what = "paddle cancel subscription";
    if (!SUBSCRIPTION_ID.test(subscriptionId)) throw new UpstreamError(`${what}: bad subscription id`);
    const response = await send(this.fetch, what, `${this.base()}/subscriptions/${subscriptionId}/cancel`, {
      method: "POST",
      headers: this.headers(true),
      body: JSON.stringify({ effective_from: "immediately" }),
    });
    if (response.status !== 200) throw new UpstreamError(`${what}: status ${response.status}`);
  }

  private async listAll(what: string, first: string, maxPages: number, opts: { partialOk?: boolean } = {}): Promise<unknown[]> {
    const base = this.base();
    let url = first;
    const all: unknown[] = [];
    for (let page = 0; page < maxPages; page += 1) {
      const response = await send(this.fetch, what, url, {
        method: "GET",
        headers: { authorization: `Bearer ${this.config.apiKey}` },
      });
      if (response.status !== 200) throw new UpstreamError(`${what}: status ${response.status}`);
      const body = await readJson(what, response);
      if (!isObject(body) || !Array.isArray(body["data"])) throw new UpstreamError(`${what}: unexpected body`);
      all.push(...body["data"]);
      const pagination = isObject(body["meta"]) ? body["meta"]["pagination"] : undefined;
      const hasMore = isObject(pagination) && pagination["has_more"] === true;
      if (!hasMore || (opts.partialOk === true && page + 1 === maxPages)) return all;
      const next = isObject(pagination) ? pagination["next"] : undefined;
      if (typeof next !== "string") throw new UpstreamError(`${what}: more pages but no link`);
      let nextUrl: URL;
      try {
        nextUrl = new URL(next);
      } catch {
        throw new UpstreamError(`${what}: bad next link`);
      }
      if (nextUrl.origin !== base) throw new UpstreamError(`${what}: next link leaves Paddle`);
      url = nextUrl.href;
    }
    throw new UpstreamError(`${what}: too many pages`);
  }

  /**
   * The customer with exactly this email, created if there is none. A
   * creation race (409) re-reads once. An email containing a comma is
   * refused: Paddle's `email` filter is a comma-separated list.
   */
  async findOrCreateCustomer(email: string): Promise<string> {
    const what = "paddle customer";
    if (email.length === 0 || email.length > 320 || email.includes(",")) throw new UpstreamError(`${what}: unusable email`);
    const found = await this.findCustomer(email);
    if (found !== null) return found;
    const response = await send(this.fetch, what, `${this.base()}/customers`, {
      method: "POST",
      headers: this.headers(true),
      body: JSON.stringify({ email }),
    });
    if (response.status === 409) {
      const again = await this.findCustomer(email);
      if (again !== null) return again;
      throw new UpstreamError(`${what}: conflict but not found`);
    }
    if (response.status !== 201 && response.status !== 200) throw new UpstreamError(`${what}: status ${response.status}`);
    return customerId(what, await readJson(what, response));
  }

  /** An authenticated customer-portal link (https, on a Paddle host). */
  async createPortalSession(customerId: string, subscriptionIds: string[]): Promise<string> {
    const what = "paddle portal session";
    if (!CUSTOMER_ID.test(customerId)) throw new UpstreamError(`${what}: bad customer id`);
    const response = await send(this.fetch, what, `${this.base()}/customers/${customerId}/portal-sessions`, {
      method: "POST",
      headers: this.headers(true),
      body: JSON.stringify({ subscription_ids: subscriptionIds }),
    });
    if (response.status !== 201 && response.status !== 200) throw new UpstreamError(`${what}: status ${response.status}`);
    const body = await readJson(what, response);
    const data = isObject(body) ? body["data"] : undefined;
    const urls = isObject(data) ? data["urls"] : undefined;
    const general = isObject(urls) ? urls["general"] : undefined;
    const overview = isObject(general) ? general["overview"] : undefined;
    if (typeof overview !== "string" || !isPaddleHttps(overview)) throw new UpstreamError(`${what}: unexpected link`);
    return overview;
  }

  /** A checkout transaction for one unit of `priceId`; returns its id. */
  async createTransaction(customerId: string, priceId: string, clerkUserId: string): Promise<string> {
    const what = "paddle transaction";
    if (!CUSTOMER_ID.test(customerId)) throw new UpstreamError(`${what}: bad customer id`);
    const response = await send(this.fetch, what, `${this.base()}/transactions`, {
      method: "POST",
      headers: this.headers(true),
      body: JSON.stringify({
        items: [{ price_id: priceId, quantity: 1 }],
        customer_id: customerId,
        custom_data: { clerk_user_id: clerkUserId },
      }),
    });
    if (response.status !== 201 && response.status !== 200) throw new UpstreamError(`${what}: status ${response.status}`);
    const body = await readJson(what, response);
    const id = isObject(body) && isObject(body["data"]) ? body["data"]["id"] : undefined;
    if (typeof id !== "string" || !TRANSACTION_ID.test(id)) throw new UpstreamError(`${what}: unexpected id`);
    return id;
  }

  private async findCustomer(email: string): Promise<string | null> {
    const what = "paddle find customer";
    const url = `${this.base()}/customers?email=${encodeURIComponent(email)}&status=active`;
    const response = await send(this.fetch, what, url, { method: "GET", headers: this.headers(false) });
    if (response.status !== 200) throw new UpstreamError(`${what}: status ${response.status}`);
    const body = await readJson(what, response);
    if (!isObject(body) || !Array.isArray(body["data"])) throw new UpstreamError(`${what}: unexpected body`);
    const first: unknown = body["data"][0];
    return first === undefined ? null : customerId(what, { data: first });
  }

  private base(): string {
    return API[this.config.environment];
  }

  private headers(json: boolean): Record<string, string> {
    return { authorization: `Bearer ${this.config.apiKey}`, ...(json ? { "content-type": "application/json" } : {}) };
  }
}

const TRANSACTION_ID = /^txn_[a-z0-9]{26}$/;

function customerId(what: string, body: unknown): string {
  const id = isObject(body) && isObject(body["data"]) ? body["data"]["id"] : undefined;
  if (typeof id !== "string" || !CUSTOMER_ID.test(id)) throw new UpstreamError(`${what}: unexpected id`);
  return id;
}

/** https on paddle.com or one of its subdomains, nothing else. */
function isPaddleHttps(link: string): boolean {
  let url: URL;
  try {
    url = new URL(link);
  } catch {
    return false;
  }
  return url.protocol === "https:" && (url.hostname === "paddle.com" || url.hostname.endsWith(".paddle.com"));
}
