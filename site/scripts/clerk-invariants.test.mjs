// clerk-invariants.mjs: a correct instance passes, and each invariant, broken
// on its own, is reported. The fixtures follow the real shape read from
// production on 2026-09-28 (trimmed to the fields the check reads).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { instanceProblems, EXPECTED_PATHS } from './clerk-invariants.mjs';

const off = { enabled: false, required: false, used_for_first_factor: false, used_for_second_factor: false };
const good = () => ({
  env: {
    display_config: { instance_environment_type: 'production', ...EXPECTED_PATHS },
    auth_config: { second_factors: [] },
    user_settings: {
      attributes: {
        email_address: { enabled: true, required: true, used_for_first_factor: true, used_for_second_factor: false },
        phone_number: { ...off },
        authenticator_app: { ...off },
        backup_code: { ...off },
        passkey: { ...off },
      },
      sign_up: { mfa: { required: false } },
      actions: { delete_self: true },
    },
    organization_settings: { enabled: false, force_organization_selection: false },
  },
  oauth: {
    issuer: 'https://clerk.zaaheen.com',
    code_challenge_methods_supported: ['S256'],
  },
});

test('the intended production instance has no problems', () => {
  const { env, oauth } = good();
  assert.deepEqual(instanceProblems(env, oauth), []);
});

const broken = [
  ['a development instance', (c) => { c.env.display_config.instance_environment_type = 'development'; }, /not the production instance/],
  ['a second factor', (c) => { c.env.auth_config.second_factors = ['totp']; }, /second factors are on/],
  ['authenticator app on', (c) => { c.env.user_settings.attributes.authenticator_app.enabled = true; }, /authenticator_app is not off/],
  ['backup codes on', (c) => { c.env.user_settings.attributes.backup_code.enabled = true; }, /backup_code is not off/],
  ['phone as a second factor', (c) => { c.env.user_settings.attributes.phone_number.used_for_second_factor = true; }, /phone_number is used as a second factor/],
  ['MFA required at sign-up', (c) => { c.env.user_settings.sign_up.mfa.required = true; }, /MFA is required/],
  ['forced organisation selection', (c) => { c.env.organization_settings = { enabled: true, force_organization_selection: true }; }, /organisation selection is forced/],
  ['sign-in on the hosted portal', (c) => { c.env.display_config.sign_in_url = 'https://accounts.zaaheen.com/sign-in'; }, /sign_in_url/],
  ['sign-up on the hosted portal', (c) => { c.env.display_config.sign_up_url = 'https://accounts.zaaheen.com/sign-up'; }, /sign_up_url/],
  ['consent moved', (c) => { c.env.display_config.oauth_consent_url = 'https://account.zaaheen.com/oauth-consent'; }, /oauth_consent_url/],
  ['self-delete off', (c) => { c.env.user_settings.actions.delete_self = false; }, /self-delete is off/],
  ['DCR on', (c) => { c.oauth.registration_endpoint = 'https://clerk.zaaheen.com/oauth/register'; }, /Dynamic Client Registration/],
  ['CIMD on', (c) => { c.oauth.client_id_metadata_document_supported = true; }, /client ID metadata/],
  ['no PKCE S256', (c) => { c.oauth.code_challenge_methods_supported = ['plain']; }, /PKCE/],
  ['metadata unreadable', (c) => { c.oauth = null; }, /could not be read/],
];
for (const [label, breakIt, expected] of broken) {
  test(`reports ${label}`, () => {
    const c = good();
    breakIt(c);
    const problems = instanceProblems(c.env, c.oauth);
    assert.equal(problems.length, 1, JSON.stringify(problems));
    assert.match(problems[0], expected);
  });
}

test('fails closed on an empty environment', () => {
  assert.ok(instanceProblems({}, {}).length >= 5);
});
