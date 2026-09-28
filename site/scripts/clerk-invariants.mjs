// The production Clerk instance's invariants (AUTH-PAGES-DESIGN D4, D9), checked
// from what the instance publishes to anyone: its Frontend API environment
// (GET https://clerk.zaaheen.com/v1/environment) and its OAuth server metadata
// (GET https://clerk.zaaheen.com/.well-known/oauth-authorization-server).
//
// No Clerk secret is needed or held: the deploy job must not carry the key that
// administers every account. One invariant is not public: "exactly one OAuth
// application, loopback-only" needs the Backend API, so it stays a dashboard and
// CLI check (OPS-HANDOFF section I) rather than a CI one.
//
// Why each rule exists:
// - MFA and session tasks off: the account pages cannot complete a second
//   factor or a task, so a person who enrols one would be locked out (step-2
//   review, finding 5).
// - DCR and CIMD off: no one can register an OAuth client of their own and
//   phish a consent screen under our name (D4).
// - Sign-in and sign-up paths on account.zaaheen.com, consent on Clerk's hosted
//   accounts.zaaheen.com (D9 production).
// - Self-delete on: the delete page and the app's Delete my account need it
//   (ACCOUNT-DELETION-DESIGN).

export const ACCOUNT_ORIGIN = 'https://account.zaaheen.com';
export const EXPECTED_PATHS = {
  sign_in_url: `${ACCOUNT_ORIGIN}/sign-in/`,
  sign_up_url: `${ACCOUNT_ORIGIN}/sign-up/`,
  oauth_consent_url: 'https://accounts.zaaheen.com/oauth-consent',
};

/** Every way the instance differs from D4/D9. `env` and `oauth` are the two parsed JSON documents. */
export function instanceProblems(env, oauth) {
  const problems = [];
  const display = env?.display_config ?? {};
  const auth = env?.auth_config ?? {};
  const attrs = env?.user_settings?.attributes ?? {};
  const orgs = env?.organization_settings ?? {};

  if (display.instance_environment_type !== 'production') {
    problems.push(`not the production instance (instance_environment_type ${display.instance_environment_type})`);
  }

  // MFA: no second factor of any kind, and none required at sign-up.
  if (!Array.isArray(auth.second_factors) || auth.second_factors.length !== 0) {
    problems.push(`second factors are on (${JSON.stringify(auth.second_factors)}); the account pages cannot complete one`);
  }
  for (const name of ['authenticator_app', 'backup_code']) {
    if (attrs[name]?.enabled !== false) problems.push(`${name} is not off`);
  }
  for (const [name, a] of Object.entries(attrs)) {
    if (a?.used_for_second_factor === true) problems.push(`${name} is used as a second factor`);
  }
  if (env?.user_settings?.sign_up?.mfa?.required !== false) problems.push('MFA is required at sign-up');

  // Session tasks: the only one Clerk has on an instance without passwords is
  // choosing an organisation.
  if (orgs.force_organization_selection !== false && orgs.enabled !== false) {
    problems.push('organisation selection is forced (a session task the pages cannot complete)');
  }

  // Our own sign-in pages, Clerk's hosted consent.
  for (const [key, want] of Object.entries(EXPECTED_PATHS)) {
    if (display[key] !== want) problems.push(`${key} is ${JSON.stringify(display[key])}, expected ${want}`);
  }

  if (env?.user_settings?.actions?.delete_self !== true) problems.push('self-delete is off (the delete page needs it)');

  // DCR and CIMD: both advertise themselves in the OAuth server metadata when on.
  if (!oauth || typeof oauth !== 'object' || !oauth.issuer) {
    problems.push('the OAuth server metadata could not be read');
  } else {
    if ('registration_endpoint' in oauth) problems.push('Dynamic Client Registration is on (registration_endpoint is published)');
    for (const key of Object.keys(oauth)) {
      if (/client_id_metadata/i.test(key) && oauth[key]) problems.push(`client ID metadata documents are on (${key})`);
    }
    if (!(oauth.code_challenge_methods_supported ?? []).includes('S256')) problems.push('PKCE S256 is not supported');
  }
  return problems;
}
