import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'astro/config';
import searchIndex from './src/integrations/search-index.mjs';
import { builtFiles, pageFolders, siteHeaders, siteRedirects } from './scripts/cloudflare-files.mjs';

// Cloudflare's _headers and _redirects, translated from the built .htaccess
// files (scripts/cloudflare-files.mjs, ADR-SEC-047). scripts/audit.mjs rebuilds them
// and fails on any difference.
const cloudflareFiles = {
  name: 'zaaheen-cloudflare-files',
  hooks: {
    'astro:build:done': ({ dir }) => {
      const out = fileURLToPath(dir);
      const pay = path.join(out, 'pay', '.htaccess');
      const root = fs.readFileSync(path.join(out, '.htaccess'), 'utf8');
      fs.writeFileSync(path.join(out, '_headers'), siteHeaders(root, fs.existsSync(pay) ? fs.readFileSync(pay, 'utf8') : null));
      fs.writeFileSync(path.join(out, '_redirects'), siteRedirects(pageFolders(builtFiles(out))));
    },
  },
};

// Static output only: every page is plain HTML on disk, readable by crawlers
// and AI page readers that never run JavaScript.
export default defineConfig({
  site: 'https://zaaheen.com',
  output: 'static',
  // The Documents search index, written after every build (s83).
  integrations: [searchIndex(), cloudflareFiles],
  trailingSlash: 'ignore',
  build: {
    format: 'directory',
  },
  // Astro 7 defaults to 'jsx', which deletes the space at a line break before an
  // inline element ("Choose\n<em>More info</em>" renders "ChooseMore info").
  // true collapses whitespace but keeps one space, as Astro 6 did.
  compressHTML: true,
  vite: {
    build: {
      // Never inline scripts or styles into the HTML. The Content-Security-Policy
      // in public/.htaccess allows only same-origin files, so an inlined script
      // would be silently blocked. scripts/audit.mjs fails the build if one appears.
      assetsInlineLimit: 0,
    },
  },
});
