// The Worker's configuration (§8.30). Every value is account-specific, so
// none lives in the public repo: they come from Cloudflare secrets in
// production and `.dev.vars` (gitignored) locally. Anything missing or
// inconsistent gives `null`, and the Worker answers 503 ("try later"), never
// a refusal that could look like the user's fault.

import type { PaddleEnvironment } from "./paddle";

export interface Config {
  /** `webhookSecret` is the Svix signing secret (`whsec_...`) for /clerk/webhook. */
  clerk: { secretKey: string; clientId: string; webhookSecret: string };
  paddle: {
    apiKey: string;
    environment: PaddleEnvironment;
    productId: string;
    /** The allowlisted prices `/v1/checkout` maps a plan to (§5). */
    prices: { monthly: string; annual: string };
    /** The notification destination's secret key (`/paddle/webhook`). */
    webhookSecret: string;
  };
  lease: { kid: string; pkcs8: string };
  killSwitch: boolean;
}

const KID = /^[a-z0-9-]{1,32}$/;
const PRODUCT_ID = /^pro_[a-z0-9]{26}$/;
const PRICE_ID = /^pri_[a-z0-9]{26}$/;
const PADDLE_KEY_PREFIX: Record<PaddleEnvironment, string> = {
  sandbox: "pdl_sdbx_apikey_",
  live: "pdl_live_apikey_",
};

export function readConfig(env: Record<string, unknown>): Config | null {
  const get = (name: string): string | null => {
    const v = env[name];
    return typeof v === "string" && v.length > 0 ? v : null;
  };
  const clerkKey = get("CLERK_SECRET_KEY");
  const clientId = get("CLERK_OAUTH_CLIENT_ID");
  const clerkWebhookSecret = get("CLERK_WEBHOOK_SECRET");
  const paddleKey = get("PADDLE_API_KEY");
  const paddleEnv = get("PADDLE_ENVIRONMENT");
  const productId = get("PADDLE_PRODUCT_ID");
  const monthly = get("PADDLE_PRICE_MONTHLY");
  const annual = get("PADDLE_PRICE_ANNUAL");
  const webhookSecret = get("PADDLE_WEBHOOK_SECRET");
  const kid = get("LEASE_PRIMARY_KID");
  const pkcs8 = get("LEASE_PRIMARY_KEY");
  if (!clerkKey || !clientId || !clerkWebhookSecret || !paddleKey || !paddleEnv || !productId || !monthly || !annual || !webhookSecret || !kid || !pkcs8) {
    return null;
  }

  if (!/^sk_(test|live)_/.test(clerkKey)) return null;
  if (!clerkWebhookSecret.startsWith("whsec_")) return null;
  if (paddleEnv !== "sandbox" && paddleEnv !== "live") return null;
  // A live key against the sandbox, or the other way round, is a deploy mistake.
  if (!paddleKey.startsWith(PADDLE_KEY_PREFIX[paddleEnv])) return null;
  if (!PRODUCT_ID.test(productId) || !KID.test(kid)) return null;
  if (!PRICE_ID.test(monthly) || !PRICE_ID.test(annual) || monthly === annual) return null;

  return {
    clerk: { secretKey: clerkKey, clientId, webhookSecret: clerkWebhookSecret },
    paddle: { apiKey: paddleKey, environment: paddleEnv, productId, prices: { monthly, annual }, webhookSecret },
    lease: { kid, pkcs8 },
    killSwitch: env["NEVER_END_PAYERS"] === "1",
  };
}
