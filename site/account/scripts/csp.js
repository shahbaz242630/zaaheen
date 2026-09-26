// The account origin's one Content-Security-Policy (AUTH-PAGES-DESIGN D5 and
// amendment S1-1), shared by the build (astro.account.config.mjs writes it
// into .htaccess) and the audit (scripts/audit-account.mjs pins it). Changing
// it is a reviewed ADR-SEC-034 amendment, never a quiet edit.
//
// Why each third-party source is here (measured in build step 1):
//   challenges.cloudflare.com  Clerk's bot check: its loader (script-src) and
//                              the frame it opens (frame-src). Protect is not
//                              allowed (D5): sign-in fails closed instead.
//   the Frontend API host      every Clerk call (connect-src).
export const PRODUCTION_FAPI_HOST = 'clerk.zaaheen.com';

export const accountCsp = (fapiHost) => [
  "default-src 'self'",
  "script-src 'self' https://challenges.cloudflare.com",
  `connect-src 'self' https://${fapiHost}`,
  'frame-src https://challenges.cloudflare.com',
  "img-src 'self' data:",
  "style-src 'self'",
  "font-src 'self'",
  "object-src 'none'",
  "frame-ancestors 'none'",
  "base-uri 'none'",
  "form-action 'self'",
  "require-trusted-types-for 'script'",
  'trusted-types default',
].join('; ');
