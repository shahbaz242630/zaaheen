// The account pages' wiring (AUTH-PAGES-DESIGN D1-D4): the DOM and Clerk on
// one side, the tested decisions on the other (redirect.js, account-state.js).
// Rules this file keeps, pinned by scripts/account-source.test.mjs:
//   - text reaches the page only through textContent (no HTML from strings);
//   - the only navigation is location.assign(<URL from redirect.js>.href);
//   - every form handler calls preventDefault;
//   - Clerk's own error message is never shown (messageFor(code) instead).
import { readAccountConfig, readRedirect, checkTarget, checkNavigation } from './redirect.js';
import { FIXED_LINE, onLoad, afterSignIn, afterSignUp, resend, messageFor } from './account-state.js';
import {
  DELETE_PATH, NO_ACCOUNT_PATH, DELETE_FAILED_LINE, deleteMarker, noGoogleAccount, onDeleteLoad,
  confirmed, afterDelete, reverifyPlan, afterReverify,
} from './delete-state.js';

const body = document.body;
const PAGE = body.dataset.page;
const CONFIG = readAccountConfig({ publishableKey: body.dataset.pk, clientId: body.dataset.clientId });
const REDIRECT = readRedirect(location.search, CONFIG);
const $ = (id) => document.getElementById(id);

// The delete page (ACCOUNT-DELETION-DESIGN D3, D4): it signs in like the sign-in
// page, then confirms and deletes. Google's return from it carries a fixed
// marker, never a URL, and finishes only on these two constant addresses.
const DELETING = PAGE === 'delete-account';
const AFTER_DELETE = PAGE === 'sso-callback' && deleteMarker(location.search);
const DELETE_URL = new URL(DELETE_PATH, location.origin).href;
const NO_ACCOUNT_URL = new URL(NO_ACCOUNT_PATH, location.origin).href;

const LOAD_TIMEOUT_MS = 20_000;

let clerk = null;
let lastSentAt = null;
let resendTimer = null;

// ---------- views ----------------------------------------------------------------

function show(view) {
  for (const el of document.querySelectorAll('[data-view]')) el.hidden = el.dataset.view !== view;
  const first = document.querySelector(`[data-view="${view}"] input:not([type=checkbox])`);
  if (first && !first.closest('[hidden]')) first.focus();
}

function formError(text) {
  const el = document.querySelector('[data-view]:not([hidden]) .acct-error');
  if (!el) return;
  el.textContent = text;
  el.hidden = !text;
}

function fatal(line = FIXED_LINE) {
  $('error-line').textContent = line;
  show('error');
}

const codeOf = (e) => (e && Array.isArray(e.errors) && e.errors[0] && e.errors[0].code) || null;

function failWith(e) {
  const code = codeOf(e);
  if (code === 'session_exists') return DELETING ? showConfirm() : showSignedIn();
  const line = messageFor(code);
  if (line === FIXED_LINE) return fatal();
  formError(line);
}

function busy(form, on) {
  for (const b of form.querySelectorAll('button')) b.disabled = on;
}

// ---------- the way back -----------------------------------------------------------

const keepQuery = () => (REDIRECT.state === 'ok' ? `?${new URLSearchParams({ redirect_url: REDIRECT.url.href })}` : '');

// Only ever navigate to a URL redirect.js produced, checked again after Clerk
// adds its own parameters (D4).
function goBack() {
  if (REDIRECT.state !== 'ok') return showSignedIn();
  const built = checkTarget(clerk.buildUrlWithAuth(REDIRECT.url.href), CONFIG);
  if (!built) return fatal();
  location.assign(built.href);
}

function showSignedIn() {
  const email = clerk && clerk.user && clerk.user.primaryEmailAddress && clerk.user.primaryEmailAddress.emailAddress;
  const who = $('signed-in-as');
  who.textContent = email ? `Signed in as ${email}` : '';
  who.hidden = !email;
  $('continue-box').hidden = REDIRECT.state !== 'ok';
  $('no-app-line').hidden = REDIRECT.state === 'ok';
  show('signed-in');
}

async function finish(sessionId) {
  await clerk.setActive({ session: sessionId });
  // A pending session task (D1) is something these pages cannot complete.
  if (clerk.session && clerk.session.currentTask) return fatal();
  if (DELETING) return showConfirm();
  goBack();
}

function apply(next) {
  switch (next.view) {
    case 'finish': return false;
    case 'enter-code': return showCode(), true;
    case 'more-details': return showDetails(next.fields), true;
    case 'signed-in': return showSignedIn(), true;
    case 'error': return fatal(next.line), true;
    default: return show(next.view), true;
  }
}

// ---------- the code step ---------------------------------------------------------

function showCode() {
  const attempt = PAGE === 'sign-up' ? clerk.client.signUp : clerk.client.signIn;
  $('code-sent-to').textContent = (PAGE === 'sign-up' ? attempt.emailAddress : attempt.identifier) || 'your email';
  $('code').value = '';
  $('code-note').hidden = true;
  show('enter-code');
  tickResend();
}

function tickResend() {
  clearInterval(resendTimer);
  const btn = $('resend');
  const paint = () => {
    const r = resend(lastSentAt, Date.now());
    btn.disabled = !r.allowed;
    btn.textContent = r.allowed ? 'Send a new code' : `Send a new code in ${r.wait}s`;
    if (r.allowed) clearInterval(resendTimer);
  };
  paint();
  resendTimer = setInterval(paint, 1000);
}

async function sendCode() {
  if (PAGE === 'sign-up') {
    await clerk.client.signUp.prepareEmailAddressVerification({ strategy: 'email_code' });
  } else {
    const si = clerk.client.signIn;
    const factor = (si.supportedFirstFactors || []).find((f) => f.strategy === 'email_code');
    if (!factor) throw new Error('no email code factor');
    await si.prepareFirstFactor({ strategy: 'email_code', emailAddressId: factor.emailAddressId });
  }
  lastSentAt = Date.now();
}

// ---------- sign-up details (S1-2) -------------------------------------------------

let detailFields = [];

function showDetails(fields) {
  detailFields = fields;
  for (const el of document.querySelectorAll('[data-field]')) el.hidden = !fields.includes(el.dataset.field);
  show('more-details');
}

// ---------- wiring ---------------------------------------------------------------

function wireForms() {
  for (const a of document.querySelectorAll('a[data-keep-redirect]')) a.search = keepQuery();

  $('start-form').addEventListener('submit', async (ev) => {
    ev.preventDefault();
    formError('');
    const form = ev.currentTarget;
    const email = $('email').value.trim();
    busy(form, true);
    try {
      if (PAGE === 'sign-up') {
        const first = $('first-name').value.trim();
        const last = $('last-name').value.trim();
        if (!first || !last || !email) return formError(messageFor('form_param_nil'));
        if (!$('terms').checked) return formError('Please agree to the Terms and the Privacy Policy to continue.');
        const su = await clerk.client.signUp.create({
          firstName: first, lastName: last, emailAddress: email, legalAccepted: true,
          unsafeMetadata: { marketing_consent: $('tips').checked },
        });
        const next = afterSignUp(su);
        if (next.view === 'enter-code') await sendCode();
        if (!apply(next)) await finish(su.createdSessionId);
      } else {
        if (!email) return formError(messageFor('form_param_nil'));
        const si = await clerk.client.signIn.create({ identifier: email });
        const next = afterSignIn(si.status);
        if (next.view === 'enter-code') await sendCode();
        if (!apply(next)) await finish(si.createdSessionId);
      }
    } catch (e) {
      failWith(e);
    } finally {
      busy(form, false);
    }
  });

  $('code-form').addEventListener('submit', async (ev) => {
    ev.preventDefault();
    formError('');
    const form = ev.currentTarget;
    const code = $('code').value.replace(/\s+/g, '');
    if (!/^\d{6}$/.test(code)) return formError(messageFor('form_code_incorrect'));
    busy(form, true);
    try {
      if (PAGE === 'sign-up') {
        const su = await clerk.client.signUp.attemptEmailAddressVerification({ code });
        const next = afterSignUp(su);
        if (!apply(next)) await finish(su.createdSessionId);
      } else {
        const si = await clerk.client.signIn.attemptFirstFactor({ strategy: 'email_code', code });
        const next = afterSignIn(si.status);
        if (!apply(next)) await finish(si.createdSessionId);
      }
    } catch (e) {
      failWith(e);
    } finally {
      busy(form, false);
    }
  });

  $('resend').addEventListener('click', async (ev) => {
    ev.preventDefault();
    if (!resend(lastSentAt, Date.now()).allowed) return;
    formError('');
    try {
      await sendCode();
      tickResend();
      const note = $('code-note');
      note.textContent = 'We sent a new code.';
      note.hidden = false;
    } catch (e) {
      failWith(e);
    }
  });

  $('different-email').addEventListener('click', (ev) => {
    ev.preventDefault();
    clearInterval(resendTimer);
    lastSentAt = null;
    show('start');
  });

  $('google').addEventListener('click', async (ev) => {
    ev.preventDefault();
    formError('');
    const attempt = PAGE === 'sign-up' ? clerk.client.signUp : clerk.client.signIn;
    const extra = PAGE === 'sign-up'
      ? { legalAccepted: $('terms').checked || undefined, unsafeMetadata: { marketing_consent: $('tips').checked } }
      : {};
    try {
      await attempt.authenticateWithRedirect({
        strategy: 'oauth_google',
        redirectUrl: new URL(`/sso-callback/${DELETING ? '?after=delete' : keepQuery()}`, location.origin).href,
        redirectUrlComplete: DELETING ? DELETE_URL : REDIRECT.state === 'ok' ? REDIRECT.url.href : new URL('/sign-in/', location.origin).href,
        ...extra,
      });
    } catch (e) {
      failWith(e);
    }
  });

  const details = $('details-form');
  if (details) {
    details.addEventListener('submit', async (ev) => {
      ev.preventDefault();
      formError('');
      const form = ev.currentTarget;
      const update = {};
      if (detailFields.includes('first_name')) update.firstName = $('more-first-name').value.trim();
      if (detailFields.includes('last_name')) update.lastName = $('more-last-name').value.trim();
      if ((detailFields.includes('first_name') && !update.firstName) || (detailFields.includes('last_name') && !update.lastName)) {
        return formError(messageFor('form_param_nil'));
      }
      if (detailFields.includes('legal_accepted')) {
        if (!$('more-terms').checked) return formError('Please agree to the Terms and the Privacy Policy to continue.');
        update.legalAccepted = true;
        update.unsafeMetadata = { marketing_consent: $('more-tips').checked };
      }
      busy(form, true);
      try {
        const su = await clerk.client.signUp.update(update);
        const next = afterSignUp(su);
        if (!apply(next)) await finish(su.createdSessionId);
      } catch (e) {
        failWith(e);
      } finally {
        busy(form, false);
      }
    });
  }

  $('continue').addEventListener('click', (ev) => {
    ev.preventDefault();
    goBack();
  });
}

// ---------- the delete page (ACCOUNT-DELETION-DESIGN D3) ----------------------------

let reverified = false;

const accountEmail = () => (clerk && clerk.user && clerk.user.primaryEmailAddress && clerk.user.primaryEmailAddress.emailAddress) || '';

function showConfirm() {
  $('delete-email').textContent = accountEmail() || 'your Zaaheen account';
  $('delete-typed').value = '';
  $('delete-btn').disabled = true;
  show('confirm');
}

function applyDelete(next) {
  switch (next.view) {
    case 'confirm': return showConfirm();
    case 'deleted': return show('deleted');
    case 'reverify': return startReverify();
    case 'error': return fatal(next.line);
    case 'enter-code': return showCode();
    case 'start': {
      const note = $('start-note');
      note.textContent = next.note || '';
      note.hidden = !next.note;
      return show('start');
    }
    default: return show(next.view);
  }
}

// Clerk refused because the sign-in is too old: one code, sent now, asked for
// in place (spike S3-S5). Never a sign-out, never a loop.
async function startReverify() {
  try {
    const verification = await clerk.session.startVerification({ level: 'first_factor' });
    const plan = reverifyPlan(verification);
    if (plan.view === 'error') return fatal(plan.line);
    await clerk.session.prepareFirstFactorVerification({ strategy: 'email_code', emailAddressId: plan.emailAddressId });
    $('reverify-to').textContent = accountEmail() || 'your email';
    $('reverify-code').value = '';
    show('reverify');
  } catch {
    fatal(DELETE_FAILED_LINE);
  }
}

async function deleteNow() {
  let next;
  try {
    await clerk.user.delete();
    next = afterDelete(null, reverified);
  } catch (e) {
    next = afterDelete(codeOf(e) || 'unknown', reverified);
  }
  applyDelete(next);
}

function wireDelete() {
  $('delete-typed').addEventListener('input', () => {
    $('delete-btn').disabled = !confirmed($('delete-typed').value);
  });

  $('delete-form').addEventListener('submit', async (ev) => {
    ev.preventDefault();
    formError('');
    if (!confirmed($('delete-typed').value)) return;
    const form = ev.currentTarget;
    busy(form, true);
    await deleteNow();
    busy(form, false);
    $('delete-btn').disabled = !confirmed($('delete-typed').value);
  });

  $('reverify-form').addEventListener('submit', async (ev) => {
    ev.preventDefault();
    formError('');
    const form = ev.currentTarget;
    const code = $('reverify-code').value.replace(/\s+/g, '');
    if (!/^\d{6}$/.test(code)) return formError(messageFor('form_code_incorrect'));
    busy(form, true);
    try {
      const verification = await clerk.session.attemptFirstFactorVerification({ strategy: 'email_code', code });
      const next = afterReverify(verification && verification.status);
      if (next.view !== 'retry') return applyDelete(next);
      reverified = true;
      await deleteNow();
    } catch (e) {
      const line = messageFor(codeOf(e));
      if (line === FIXED_LINE) fatal(DELETE_FAILED_LINE);
      else formError(line);
    } finally {
      busy(form, false);
    }
  });
}

// ---------- Google's return page ---------------------------------------------------

async function callback() {
  const signIn = AFTER_DELETE ? NO_ACCOUNT_URL : new URL(`/sign-in/${keepQuery()}`, location.origin).href;
  const signUp = AFTER_DELETE ? NO_ACCOUNT_URL : new URL(`/sign-up/${keepQuery()}`, location.origin).href;
  const cont = AFTER_DELETE ? NO_ACCOUNT_URL : new URL(`/sign-up/?${new URLSearchParams({ continue: '1', ...(REDIRECT.state === 'ok' ? { redirect_url: REDIRECT.url.href } : {}) })}`, location.origin).href;
  const handed = [signIn, signUp, cont];
  // Where a finished Google sign-in or sign-up goes. Clerk navigates there by
  // ITSELF, not through `navigate` below, and without checking
  // allowedRedirectOrigins (independent review, session 65). So all four
  // redirect props must be `done`, and `done` must be validator output or our
  // own sign-in page: scripts/account-source.test.mjs pins both.
  const done = AFTER_DELETE ? DELETE_URL : REDIRECT.state === 'ok' ? REDIRECT.url.href : signIn;
  // Clerk's other steps (the "one more step" page, a transfer between sign-in
  // and sign-up) go through this function.
  const navigate = (to) => {
    const url = checkNavigation(String(to), handed, CONFIG, location.origin);
    if (!url) {
      fatal();
      return Promise.resolve();
    }
    location.assign(url.href);
    return Promise.resolve();
  };
  try {
    await clerk.handleRedirectCallback({
      signInUrl: signIn, signUpUrl: signUp, continueSignUpUrl: cont,
      signInFallbackRedirectUrl: done, signUpFallbackRedirectUrl: done,
      signInForceRedirectUrl: done, signUpForceRedirectUrl: done,
      // From the delete page, a Google address with no account goes to
      // NO_ACCOUNT_URL (Clerk's sign-in URL) instead of becoming a sign-up.
      ...(AFTER_DELETE ? { transferable: false } : {}),
    }, navigate);
  } catch (e) {
    failWith(e);
  }
}

// ---------- start ------------------------------------------------------------------

async function waitForClerk() {
  for (let i = 0; i < 100 && !window.Clerk; i++) await new Promise((r) => setTimeout(r, 50));
  if (!window.Clerk) throw new Error('Clerk did not load');
  return window.Clerk;
}

async function start() {
  // A bad link is refused before anything else, before Clerk is even asked (D4).
  if (!CONFIG || REDIRECT.state === 'invalid') return show('bad-link');
  try {
    clerk = await waitForClerk();
    const origins = [`https://${CONFIG.fapiHost}`, ...(CONFIG.portalHost ? [`https://${CONFIG.portalHost}`] : [])];
    // A Frontend API that never answers must not leave "One moment…" forever.
    await Promise.race([
      clerk.load({ telemetry: false, allowedRedirectOrigins: origins }),
      new Promise((_, reject) => setTimeout(() => reject(new Error('Clerk load timed out')), LOAD_TIMEOUT_MS)),
    ]);
  } catch {
    return fatal();
  }
  if (DELETING) {
    wireForms();
    wireDelete();
    return applyDelete(onDeleteLoad({
      redirect: REDIRECT.state,
      signedIn: Boolean(clerk.user),
      signIn: clerk.client && clerk.client.signIn ? { status: clerk.client.signIn.status } : null,
      noAccount: noGoogleAccount(location.search),
    }));
  }
  const view = onLoad({
    page: PAGE,
    redirect: REDIRECT.state,
    signedIn: Boolean(clerk.user),
    signIn: clerk.client && clerk.client.signIn ? { status: clerk.client.signIn.status } : null,
    signUp: clerk.client && clerk.client.signUp ? {
      status: clerk.client.signUp.status,
      missingFields: clerk.client.signUp.missingFields,
      unverifiedFields: clerk.client.signUp.unverifiedFields,
    } : null,
  });
  if (view.view === 'callback') {
    show('callback');
    return callback();
  }
  wireForms();
  apply(view);
}

start();
