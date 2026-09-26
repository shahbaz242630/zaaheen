import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'astro/config';
import { readAccountConfig } from './account/scripts/redirect.js';
import { accountCsp } from './account/scripts/csp.js';

// account.zaaheen.com: the sign-in, sign-up and Google-return pages
// (AUTH-PAGES-DESIGN, ADR-109). Same repo and styles as zaaheen.com, its own
// origin and build output (D1): npm run build:account → dist-account/.
//
// Two public build settings (D3): PUBLIC_CLERK_PUBLISHABLE_KEY (the Frontend
// API host is decoded from it, so key and host can never disagree) and
// PUBLIC_ZAAHEEN_CLIENT_ID (the app's OAuth client). Missing or undecodable
// settings fail the build: a page that matches nothing must never ship.
const env = { publishableKey: process.env.PUBLIC_CLERK_PUBLISHABLE_KEY, clientId: process.env.PUBLIC_ZAAHEEN_CLIENT_ID };
const config = readAccountConfig(env);
if (!config) {
  throw new Error('account build: PUBLIC_CLERK_PUBLISHABLE_KEY and PUBLIC_ZAAHEEN_CLIENT_ID must be set and valid (AUTH-PAGES-DESIGN D3)');
}
// A development key turns on development-only allowances (the Portal consent
// page, Clerk's handshake parameter; redirect.js). So a pk_test_ build must be
// asked for by name, never produced by a stray setting (independent review,
// session 65). The deploy additionally runs the --release audit.
if (config.dev && process.env.ACCOUNT_DEV !== '1') {
  throw new Error('account build: a pk_test_ key needs ACCOUNT_DEV=1 (a development build; never deployed)');
}
const here = path.dirname(fileURLToPath(import.meta.url));

// The origin's one policy (account/scripts/csp.js), written into .htaccess with
// this build's Frontend API host. scripts/audit-account.mjs pins it word for
// word, and on --release pins the production host.
const writeHtaccess = {
  name: 'zaaheen-account-htaccess',
  hooks: {
    'astro:build:done': ({ dir }) => {
      const out = fileURLToPath(dir);
      const template = fs.readFileSync(path.join(here, 'account', 'htaccess.template'), 'utf8');
      if (template.split('{{CSP}}').length !== 2) throw new Error('htaccess.template must hold exactly one {{CSP}}');
      fs.writeFileSync(path.join(out, '.htaccess'), template.replace('{{CSP}}', accountCsp(config.fapiHost)));
    },
  },
};

export default defineConfig({
  site: 'https://account.zaaheen.com',
  output: 'static',
  srcDir: './account',
  publicDir: './account-public',
  outDir: './dist-account',
  trailingSlash: 'always',
  build: { format: 'directory', inlineStylesheets: 'never' },
  compressHTML: true,
  devToolbar: { enabled: false },
  integrations: [writeHtaccess],
  vite: {
    // Never inline scripts or styles: the CSP allows only same-origin files.
    build: { assetsInlineLimit: 0 },
  },
});
