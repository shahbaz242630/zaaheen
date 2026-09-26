// Fetches the one file the account pages need from Clerk's npm package
// (AUTH-PAGES-DESIGN D3, amendment S2-1) and writes it next to the pages.
//
//   node scripts/vendor-clerk.mjs <exact version>      (from site/)
//   node scripts/vendor-clerk.mjs --check              (CI: writes nothing)
//
// --check fetches the pinned version (account/scripts/clerk-pin.js) and fails
// unless npm's published file has the pinned SHA-256 and the file in
// account-public/clerk/ is byte-identical to it. So a change that swaps both
// the file and its pin for something npm never published fails CI
// (independent review, session 65, finding 7).
//
// Why a vendored file and not an npm dependency: the pages serve Clerk's
// prebuilt dist/clerk.browser.js and nothing else from the package, while the
// package's own dependencies (wallet SDKs) would enter the site's lockfile and
// never run. The tarball is checked against the registry's sha512 integrity
// before anything is read from it; the file's SHA-256 is printed for the pin
// in scripts/audit-account.mjs (CLERK_JS). Nothing is installed or executed.
import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import crypto from 'node:crypto';

import { CLERK_JS } from '../account/scripts/clerk-pin.js';

const CHECK = process.argv[2] === '--check';
const version = CHECK ? CLERK_JS.version : process.argv[2];
if (!/^\d+\.\d+\.\d+$/.test(version || '')) {
  console.error('usage: node scripts/vendor-clerk.mjs <exact version, e.g. 6.34.1>');
  process.exit(1);
}
const meta = await (await fetch(`https://registry.npmjs.org/@clerk/clerk-js/${version}`)).json();
const { tarball, integrity } = meta.dist || {};
if (!tarball || !/^sha512-/.test(integrity || '')) throw new Error('registry gave no tarball or sha512 integrity');
const tgz = Buffer.from(await (await fetch(tarball)).arrayBuffer());
const got = `sha512-${crypto.createHash('sha512').update(tgz).digest('base64')}`;
if (got !== integrity) throw new Error('tarball does not match the registry integrity');

// A plain tar walk: 512-byte headers, name at 0..100, size (octal) at 124..136.
const tar = zlib.gunzipSync(tgz);
let file = null;
for (let off = 0; off + 512 <= tar.length;) {
  const name = tar.subarray(off, off + 100).toString('utf8').replace(/\0.*$/s, '');
  if (!name) break;
  const size = parseInt(tar.subarray(off + 124, off + 136).toString('utf8').replace(/\0.*$/s, '').trim(), 8);
  if (name === 'package/dist/clerk.browser.js') file = tar.subarray(off + 512, off + 512 + size);
  off += 512 + Math.ceil(size / 512) * 512;
}
if (!file) throw new Error('package/dist/clerk.browser.js not in the tarball');
const published = crypto.createHash('sha256').update(file).digest('hex');

if (CHECK) {
  const local = path.resolve('account-public/clerk', CLERK_JS.file);
  const problems = [];
  if (published !== CLERK_JS.sha256) problems.push(`npm's ${version} file has sha256 ${published}, the pin says ${CLERK_JS.sha256}`);
  if (!fs.existsSync(local)) problems.push(`account-public/clerk/${CLERK_JS.file} is missing`);
  else if (!fs.readFileSync(local).equals(file)) problems.push(`account-public/clerk/${CLERK_JS.file} is not byte-identical to npm's published file`);
  if (problems.length) {
    console.error(`vendor-clerk --check: ${problems.join('; ')}`);
    process.exit(1);
  }
  console.log(`vendor-clerk --check: ${CLERK_JS.file} is npm's published @clerk/clerk-js ${version} (sha256 ${published})`);
  process.exit(0);
}

const outDir = path.resolve('account-public/clerk');
fs.mkdirSync(outDir, { recursive: true });
for (const old of fs.readdirSync(outDir)) fs.rmSync(path.join(outDir, old));
const name = `clerk.browser.${version}.js`;
fs.writeFileSync(path.join(outDir, name), file);
console.log(`wrote account-public/clerk/${name}`);
console.log(`bytes  ${file.length}`);
console.log(`sha256 ${published}`);
