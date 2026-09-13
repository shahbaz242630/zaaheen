import { defineConfig } from 'astro/config';

// Static output only: every page is plain HTML on disk, readable by crawlers
// and AI page readers that never run JavaScript.
export default defineConfig({
  site: 'https://zaaheen.com',
  output: 'static',
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
