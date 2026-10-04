// Builds the account pages with fixed, made-up development settings, for the
// audit's negative controls and CI (no real account value is needed or used).
//   node scripts/build-account-test.mjs        (from site/)
import { spawnSync } from 'node:child_process';

const env = {
  ...process.env,
  PUBLIC_CLERK_PUBLISHABLE_KEY: `pk_test_${Buffer.from('example-name-12.clerk.accounts.dev$').toString('base64')}`,
  PUBLIC_ZAAHEEN_CLIENT_ID: 'testclient00000',
  ACCOUNT_DEV: '1',
  // ADR-SEC-043: a development build names its account service host.
  PUBLIC_ZAAHEEN_API_HOST: 'api-sandbox.example.test',
};
const r = spawnSync(process.execPath, ['node_modules/astro/bin/astro.mjs', 'build', '--config', 'astro.account.config.mjs'], { env, stdio: 'inherit' });
process.exit(r.status ?? 1);
