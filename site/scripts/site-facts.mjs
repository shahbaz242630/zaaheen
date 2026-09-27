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
