// Where the question box does not answer (ADR-119, founder s120).
//
// The Worker runs in Europe (wrangler.jsonc), so a visitor's own location no
// longer decides whether the model can be reached. Anthropic's supported list,
// https://www.anthropic.com/supported-countries, says "Any country or region
// not listed is unsupported", for users physically there. Re-check it when it
// changes.
//
// A short list of what is missing, not a long list of what is allowed: the page
// names countries and not territories (Puerto Rico, Gibraltar), and an
// allow-list would turn those visitors away by mistake. Parts of Ukraine are
// excluded too, but a country code cannot tell them apart.
const UNSUPPORTED: ReadonlySet<string> = new Set([
  "AF", // Afghanistan
  "BY", // Belarus
  "CN", // China
  "CU", // Cuba
  "HK", // Hong Kong
  "IR", // Iran
  "KP", // North Korea
  "MM", // Myanmar
  "MO", // Macau
  "RU", // Russia
  "SY", // Syria
  "VE", // Venezuela
  "XK", // Kosovo
  "YE", // Yemen
]);

/** Whether a visitor's country (Cloudflare's two-letter code) is on the list. Unknown is never refused. */
export function isUnsupportedRegion(country: unknown): boolean {
  return typeof country === "string" && UNSUPPORTED.has(country);
}
