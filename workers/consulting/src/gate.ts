// Who may send an enquiry (ADR-SEC-057 §3): a Turnstile check solved on our
// form page's host. Same rule as Ask Zaaheen's gate (workers/ask/src/gate.ts).

export const SITEVERIFY = "https://challenges.cloudflare.com/turnstile/v0/siteverify";

/** The only host a Turnstile token may have been solved on. */
export const HOST = "consulting.zaaheen.com";

export type Fetcher = (url: string, init?: RequestInit) => Promise<Response>;

/** True only when Cloudflare accepts the token as solved on our host. Any doubt is a refusal. */
export async function verifyTurnstile(token: string, address: string, secret: string, fetcher: Fetcher): Promise<boolean> {
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
