// Update rehearsal: press "Restart to update" in the running app the way a
// person would, through WebView2's debugging port (the app is started with
// --remote-debugging-port=9222). The page polls every 10 minutes, so it is
// asked to look again first. Prints what it found and did.
// The port can open after the app's own work starts: try for a minute.
let pages;
for (let i = 0; ; i++) {
  try { pages = await (await fetch("http://127.0.0.1:9222/json")).json(); break; }
  catch (e) { if (i >= 30) throw e; await new Promise((r) => setTimeout(r, 2000)); }
}
const page = pages.find((p) => p.type === "page");
if (!page) throw new Error("no page on the debugging port: " + JSON.stringify(pages));

const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((ok, fail) => { ws.onopen = ok; ws.onerror = fail; });
let id = 0;
const pending = new Map();
ws.onmessage = (m) => {
  const msg = JSON.parse(m.data);
  if (pending.has(msg.id)) { pending.get(msg.id)(msg); pending.delete(msg.id); }
};
const evaluate = (expression) => new Promise((ok) => {
  const n = ++id;
  pending.set(n, ok);
  ws.send(JSON.stringify({ id: n, method: "Runtime.evaluate", params: { expression, awaitPromise: true, returnByValue: true } }));
});
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

console.log("page:", page.url);
for (let i = 0; i < 12; i++) {
  await evaluate("refreshUpdateStatus()");
  await sleep(2000);
  const seen = await evaluate(`(() => {
    const b = [...document.querySelectorAll('button[data-action="restart_to_update"]')]
      .find((x) => x.offsetParent !== null);
    return b ? { text: b.textContent, line: b.closest('.account-banner')?.textContent } : null;
  })()`);
  const found = seen.result && seen.result.result && seen.result.result.value;
  if (found) {
    console.log("visible:", JSON.stringify(found));
    const clicked = await evaluate(`(() => {
      const b = [...document.querySelectorAll('button[data-action="restart_to_update"]')]
        .find((x) => x.offsetParent !== null);
      b.click();
      return 'clicked';
    })()`);
    console.log("result:", JSON.stringify(clicked.result && clicked.result.result));
    // A refusal (busy / failed) shows in the bar; on success the app ends.
    await sleep(8000);
    const after = await Promise.race([
      evaluate("[...document.querySelectorAll('.account-banner')].map((b) => b.textContent).join(' | ')"),
      sleep(5000).then(() => null),
    ]);
    console.log("after 8 s:", after ? JSON.stringify(after.result && after.result.result) : "no answer (the app has likely ended to install)");
    ws.close();
    process.exit(0);
  }
  console.log("no visible button yet");
}
const body = await evaluate("document.body.innerText.slice(0, 2000)");
console.log("page text:", JSON.stringify(body.result && body.result.result));
// The install path is still rehearsed: the same command the button calls.
// The run reports this as "button not seen" so the screen gap is not hidden.
console.log("BUTTON NOT SEEN: calling restart_to_update directly");
// Not awaited: on success the app ends before the call returns.
await evaluate("window.__rehearsal = 'waiting'; window.__TAURI__.core.invoke('restart_to_update').then(() => { window.__rehearsal = 'ok'; }, (e) => { window.__rehearsal = 'error: ' + e; }); 'sent'");
await sleep(8000);
const after = await Promise.race([evaluate("window.__rehearsal"), sleep(5000).then(() => null)]);
console.log("after 8 s:", after ? JSON.stringify(after.result && after.result.result) : "no answer (the app has likely ended to install)");
ws.close();
process.exit(0);
