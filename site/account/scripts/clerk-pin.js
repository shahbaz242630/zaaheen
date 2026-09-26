// The one Clerk file the account pages serve (AUTH-PAGES-DESIGN D3, S1-4,
// S2-1): Clerk's prebuilt dist/clerk.browser.js from the npm package, fetched
// and integrity-checked by scripts/vendor-clerk.mjs into account-public/clerk/.
// scripts/audit-account.mjs refuses a build whose file differs from this hash.
// To move to a new version: run vendor-clerk.mjs with it, paste the printed
// file name and sha256 here, and repeat the browser test before merging.
export const CLERK_JS = Object.freeze({
  version: '6.34.1',
  file: 'clerk.browser.6.34.1.js',
  sha256: 'fea358f2d8b1792c7689c35c706b928976e8ec4dc3349d00c2c33d32fa599b6c',
});
