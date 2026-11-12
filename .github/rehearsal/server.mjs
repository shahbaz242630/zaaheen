// Update rehearsal (s115, AUTO-UPDATE-DESIGN.md section 5): stands in for
// https://dl.zaaheen.com on the runner. The hosts file points dl.zaaheen.com
// and dl2.zaaheen.com here; a rehearsal-only root, trusted by the machine,
// signs the certificate. Every request is appended to requests.log.
// usage: node server.mjs <served dir> <cert> <key> <requests.log>
import { createServer } from "node:https";
import { readFileSync, existsSync, statSync, createReadStream, appendFileSync } from "node:fs";
import { join, normalize } from "node:path";

const [root, cert, key, log] = process.argv.slice(2);

createServer({ cert: readFileSync(cert), key: readFileSync(key) }, (req, res) => {
  const host = req.headers.host || "";
  const path = decodeURIComponent(new URL(req.url, "https://x").pathname);
  appendFileSync(log, `${new Date().toISOString()} ${req.method} ${host}${path}\n`);
  const file = normalize(join(root, path));
  if (!file.startsWith(normalize(root)) || !existsSync(file) || !statSync(file).isFile()) {
    res.writeHead(404).end();
    return;
  }
  res.writeHead(200, { "content-length": statSync(file).size });
  createReadStream(file).pipe(res);
}).listen(443, "127.0.0.1", () => console.log("rehearsal server on 443"));
