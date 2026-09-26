// Serves a built account origin locally with the headers its .htaccess sets
// (AUTH-PAGES-DESIGN D5: astro preview ignores .htaccess, so the real policy
// is only tested through a server that sends it). Local only.
//
//   node scripts/serve-account.mjs [distDir] [port]     (default dist-account 4400)
//
// Mirrors the server's behaviour the pages rely on: "Header always set" lines,
// a directory without its slash redirects to it (query kept), / goes to
// /sign-in/, the 404 page. Nothing else from .htaccess is interpreted.
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';

const DIST = path.resolve(process.argv[2] || 'dist-account');
const PORT = Number(process.argv[3] || 4400);
const conf = fs.readFileSync(path.join(DIST, '.htaccess'), 'utf8');
const HEADERS = Object.fromEntries(
  [...conf.matchAll(/^\s*Header\s+always\s+set\s+(\S+)\s+"([^"]*)"\s*$/gim)].map((m) => [m[1], m[2]]),
);
const TYPES = {
  '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8', '.css': 'text/css; charset=utf-8',
  '.svg': 'image/svg+xml', '.woff2': 'font/woff2', '.woff': 'font/woff', '.txt': 'text/plain; charset=utf-8',
};

http.createServer((req, res) => {
  const url = new URL(req.url, `http://127.0.0.1:${PORT}`);
  const send = (status, headers, body) => {
    res.writeHead(status, { ...HEADERS, ...headers });
    res.end(body);
  };
  if (url.pathname === '/') return send(302, { Location: '/sign-in/' }, '');
  const rel = path.normalize(decodeURIComponent(url.pathname)).replace(/^([/\\])+/, '');
  let file = path.join(DIST, rel);
  const notFound = () => send(404, { 'Content-Type': TYPES['.html'] }, fs.readFileSync(path.join(DIST, '404.html')));
  if (!(file === DIST || file.startsWith(DIST + path.sep)) || path.basename(file).startsWith('.')) return notFound();
  // Read, and let the read decide: no exists-then-read gap (CodeQL js/file-system-race).
  let body;
  try {
    body = fs.readFileSync(file);
  } catch (e) {
    if (e.code !== 'EISDIR') return notFound();
    if (!url.pathname.endsWith('/')) return send(301, { Location: `${url.pathname}/${url.search}` }, '');
    file = path.join(file, 'index.html');
    try {
      body = fs.readFileSync(file);
    } catch {
      return notFound();
    }
  }
  send(200, { 'Content-Type': TYPES[path.extname(file)] || 'application/octet-stream' }, body);
}).listen(PORT, '127.0.0.1', () => console.log(`account pages on http://127.0.0.1:${PORT} (${DIST})`));
