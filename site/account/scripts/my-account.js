// The /account/ page once signed in (AUTH-PAGES-DESIGN S85-1, ADR-SEC-043).
// Rules this file keeps, pinned by scripts/account-source.test.mjs:
//   - every call to the account service carries Clerk's short-lived session
//     token as a Bearer header, never a cookie;
//   - the only navigation is location.assign(target), where target came from
//     checkoutTarget, manageTarget or cancelAnswer (a checked pay page or
//     Paddle link);
//   - text reaches the page only through textContent.
import { cancelAnswer, checkoutTarget, manageTarget, planView } from './my-account-state.js';

const TROUBLE = "We couldn't reach your account just now. Please try again in a minute.";
const $ = (id) => document.getElementById(id);

let api = null;
let clerk = null;
let working = false;

async function call(path, body) {
  const token = await clerk.session.getToken();
  if (!token) throw new Error('no session');
  const response = await fetch(`https://${api}${path}`, {
    method: 'POST',
    headers: { authorization: `Bearer ${token}`, 'content-type': 'application/json' },
    body: JSON.stringify(body),
    credentials: 'omit',
    cache: 'no-store',
  });
  if (!response.ok) throw new Error(`status ${response.status}`);
  return response.json();
}

function note(text) {
  const el = $('account-note');
  el.textContent = text;
  el.hidden = !text;
}

async function drawPlan() {
  $('plan-line').textContent = 'Checking…';
  for (const id of ['plans-row', 'manage-row', 'cancel-row']) $(id).hidden = true;
  let view = null;
  try {
    view = planView(await call('/v1/web/plan', {}));
  } catch {
    view = null;
  }
  if (!view) {
    $('plan-line').textContent = TROUBLE;
    return;
  }
  $('plan-line').textContent = view.line;
  $('plans-row').hidden = !view.plans;
  $('manage-row').hidden = !view.manage;
  $('cancel-row').hidden = !view.cancel;
}

// One action at a time; the buttons say so.
async function act(run) {
  if (working) return;
  working = true;
  for (const b of document.querySelectorAll('[data-view="account"] button')) b.disabled = true;
  note('');
  try {
    await run();
  } catch {
    note(TROUBLE);
  } finally {
    working = false;
    for (const b of document.querySelectorAll('[data-view="account"] button')) b.disabled = false;
  }
}

async function subscribe(plan) {
  const target = checkoutTarget(await call('/v1/checkout', { plan }));
  if (!target) return note(TROUBLE);
  location.assign(target);
}

// Only ever the subscription page: with no current subscription the service
// answers "none" and creates nothing (review s85, finding 4).
async function manage() {
  const target = manageTarget(await call('/v1/checkout', { plan: 'monthly', portal_only: true }));
  if (!target) {
    note("You don't have a paid subscription to manage.");
    return drawPlan();
  }
  location.assign(target);
}

async function cancel() {
  const answer = cancelAnswer(await call('/v1/cancel', {}));
  if (!answer) return note(TROUBLE);
  note(answer.line);
  if (answer.open) {
    const target = answer.open;
    location.assign(target);
  }
}

/** Show the signed-in account. `show` is account.js's view switch. */
export function openAccount({ clerk: c, show, apiHost }) {
  clerk = c;
  api = apiHost;
  const email = clerk.user && clerk.user.primaryEmailAddress && clerk.user.primaryEmailAddress.emailAddress;
  $('account-email').textContent = email || 'your Zaaheen account';
  show('account');
  if (!api) {
    $('plan-line').textContent = TROUBLE;
    return;
  }
  drawPlan();
}

/** Wired once, on the page's load. */
export function wireAccount({ onSignedOut }) {
  for (const b of document.querySelectorAll('[data-plan]')) {
    b.addEventListener('click', (ev) => {
      ev.preventDefault();
      act(() => subscribe(b.dataset.plan));
    });
  }
  $('manage').addEventListener('click', (ev) => {
    ev.preventDefault();
    act(manage);
  });
  $('cancel').addEventListener('click', (ev) => {
    ev.preventDefault();
    act(cancel);
  });
  $('sign-out').addEventListener('click', (ev) => {
    ev.preventDefault();
    // This browser only: the computers stay signed in (SIGNIN-DESIGN 8.50).
    // The callback form: Clerk then skips its own navigation to "/" (which
    // this origin sends to /sign-in/), and the page stays here (review s85,
    // finding 1).
    act(() => clerk.signOut(() => onSignedOut()));
  });
}
