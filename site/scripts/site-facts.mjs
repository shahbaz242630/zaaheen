// Facts the audit checks the build against, read from src/data/site.ts (the
// single source of truth) so a price is never written down twice. A plain
// pattern over our own source: the audit stays dependency-free, and a reshaped
// declaration fails loudly here rather than letting a check pass on nothing.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const SITE_TS = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', 'src', 'data', 'site.ts');

/** PRICES from site.ts: { monthly, yearly, currency }. Throws if it cannot be read. */
export function readPrices() {
  const src = fs.readFileSync(SITE_TS, 'utf8');
  const m = src.match(/export const PRICES = \{ monthly: (\d+(?:\.\d+)?), yearly: (\d+(?:\.\d+)?), currency: '([A-Z]{3})' \}/);
  if (!m) throw new Error(`site-facts: cannot read PRICES from ${SITE_TS}; update the pattern with the declaration`);
  return { monthly: Number(m[1]), yearly: Number(m[2]), currency: m[3] };
}

/** The app's JSON-LD offers as the audit expects them: one per plan, in plan order. */
export function expectedOffers() {
  const p = readPrices();
  return [
    { price: p.monthly, priceCurrency: p.currency, billingDuration: 'P1M' },
    { price: p.yearly, priceCurrency: p.currency, billingDuration: 'P1Y' },
  ];
}

const PRICING_TS = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', 'src', 'data', 'pricing.ts');

/** CHECKED_ON from pricing.ts ('27 September 2026'), as a Date. Throws if it cannot be read. */
export function readComparisonDate() {
  const src = fs.readFileSync(PRICING_TS, 'utf8');
  const m = src.match(/export const CHECKED_ON = '(\d{1,2} [A-Z][a-z]+ \d{4})';/);
  const date = m ? new Date(`${m[1]} 00:00 UTC`) : null;
  if (!date || Number.isNaN(date.getTime())) throw new Error(`site-facts: cannot read CHECKED_ON from ${PRICING_TS}; update the pattern with the declaration`);
  return date;
}
