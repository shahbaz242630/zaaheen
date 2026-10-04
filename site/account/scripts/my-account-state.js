// The website account page's decisions (AUTH-PAGES-DESIGN S85-1, ADR-SEC-043),
// free of the DOM and of Clerk so they can be tested in plain Node
// (scripts/my-account-state.test.mjs). my-account.js wires them to the page.
// The rows and words follow the app's Account tab.

export const ACCOUNT_PATH = '/account/';
export const PRODUCTION_API_HOST = 'api.zaaheen.com';
const PAY_PAGE = 'https://zaaheen.com/pay/';
const HOST = /^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$/;
const TXN = /^txn_[a-z0-9]{26}$/;
const PADDLE_HOST = 'paddle.com';

/** The SIGNIN-DESIGN 8.48 answers, word for word as in the app. */
export const CANCEL_LINES = {
  opened: "Finish cancelling in the page that opens. Zaaheen keeps working until the end of the time you've paid for.",
  nothing_to_cancel: "You don't have a paid subscription, so there's nothing to cancel.",
  already_ending: "Your subscription is already cancelled. Zaaheen keeps working until the end of the time you've paid for.",
};

/**
 * The account service's host. Production derives it from the sign-in host
 * (clerk.<domain> → api.<domain>), so the two can never disagree; a
 * development build names it (PUBLIC_ZAAHEEN_API_HOST) or has none.
 */
export function apiHostFor(config, devHost) {
  if (!config) return null;
  if (!config.dev) return config.fapiHost.startsWith('clerk.') ? `api.${config.fapiHost.slice('clerk.'.length)}` : null;
  return typeof devHost === 'string' && HOST.test(devHost) ? devHost : null;
}

/** `/sso-callback/?after=account`: Google's return to this page (as D4's delete marker). */
export function accountMarker(search) {
  if (typeof search !== 'string') return false;
  const params = [...new URLSearchParams(search)];
  return params.length === 1 && params[0][0] === 'after' && params[0][1] === 'account';
}

const dateOf = (epoch) =>
  new Date(epoch * 1000).toLocaleDateString('en-GB', { day: 'numeric', month: 'long', year: 'numeric', timeZone: 'UTC' });

/** What "Your plan" says and which buttons show, or null for an answer of no known shape. */
export function planView(plan) {
  if (!plan || typeof plan !== 'object') return null;
  const view = (line, paying) => ({ line, plans: !paying, manage: paying, cancel: false });
  switch (plan.state) {
    case 'not_started':
      return view('Your 30-day free trial starts when you first open the Zaaheen app.', false);
    case 'trial': {
      const d = plan.days_left;
      if (!Number.isInteger(d) || d < 0) return null;
      return view(`Free trial, ${d <= 0 ? 'less than a day' : d === 1 ? '1 day' : `${d} days`} left`, false);
    }
    case 'active':
    case 'payment_failed': {
      if (!Number.isFinite(plan.until) || typeof plan.ending !== 'boolean' || typeof plan.paying !== 'boolean') return null;
      // Time given without a subscription (review s85, finding 4): nothing to manage or cancel.
      if (!plan.paying) return { line: `Your plan runs until ${dateOf(plan.until)}`, plans: false, manage: false, cancel: false };
      const line = plan.state === 'payment_failed'
        ? "Your last payment didn't go through. Update your card under Manage subscription."
        : `Subscribed, ${plan.ending ? 'ends' : 'renews'} on ${dateOf(plan.until)}`;
      return { ...view(line, true), cancel: !plan.ending };
    }
    case 'ended':
      return view('Your plan has ended. Subscribe to keep using Zaaheen.', false);
    default:
      return null;
  }
}

/** A Paddle portal link (https on paddle.com or a subdomain, no credentials), or null. */
export function portalTarget(link) {
  let url;
  try {
    url = new URL(link);
  } catch {
    return null;
  }
  if (url.protocol !== 'https:' || url.username || url.password) return null;
  const host = url.hostname.toLowerCase();
  return host === PADDLE_HOST || host.endsWith(`.${PADDLE_HOST}`) ? url.href : null;
}

/** /v1/checkout's answer: our pay page for a checked transaction, or the portal; else null. */
export function checkoutTarget(answer) {
  if (!answer || typeof answer !== 'object') return null;
  if (answer.kind === 'checkout' && typeof answer.txn === 'string' && TXN.test(answer.txn)) {
    const url = new URL(PAY_PAGE);
    url.searchParams.set('_ptxn', answer.txn);
    return url.href;
  }
  if (answer.kind === 'portal' && typeof answer.url === 'string') return portalTarget(answer.url);
  return null;
}

/** Manage's answer (/v1/checkout with portal_only): only ever the portal, or nothing. */
export function manageTarget(answer) {
  if (!answer || typeof answer !== 'object' || answer.kind !== 'portal' || typeof answer.url !== 'string') return null;
  return portalTarget(answer.url);
}

/** /v1/cancel's answer: the line to show and what to open (a checked portal link, or nothing). */
export function cancelAnswer(answer) {
  if (!answer || typeof answer !== 'object') return null;
  if (answer.kind === 'none') return { line: CANCEL_LINES.nothing_to_cancel, open: null };
  if (answer.kind === 'ending') return { line: CANCEL_LINES.already_ending, open: null };
  if (answer.kind === 'portal' && typeof answer.url === 'string') {
    const open = portalTarget(answer.url);
    return open ? { line: CANCEL_LINES.opened, open } : null;
  }
  return null;
}
