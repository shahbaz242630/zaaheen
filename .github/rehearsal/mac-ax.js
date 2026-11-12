// Update rehearsal (s115, Mac): find and press buttons in Zaaheen's window
// the way assistive software does (System Events, Accessibility), as a
// person would click them. GitHub's Mac machine allows this (mac-probe).
//
// usage: osascript -l JavaScript mac-ax.js <action> <label> [seconds] [next]
//   click    press the first button whose label starts with <label>
//   wait     wait until such a button is on screen
//   advance  press <label> until a button starting with <next> shows
//   gone     wait until no such button is on screen
//   dump     print every button and text on screen
// Exits non-zero (a thrown error) when the time runs out.

function zaaheen(se) {
  const ps = se.processes.whose({ bundleIdentifier: "com.zaaheen.app", backgroundOnly: false })();
  return ps.length ? ps[0] : null;
}

function labels(e) {
  const out = [];
  for (const k of ["name", "description", "title", "value"]) {
    try {
      const v = e[k]();
      if (v !== null && v !== undefined && String(v).trim()) out.push(String(v).trim());
    } catch (_) {}
  }
  return out;
}

function everything(p) {
  try {
    return p.windows[0].entireContents();
  } catch (_) {
    return [];
  }
}

function find(p, label) {
  for (const e of everything(p)) {
    let role;
    try { role = e.role(); } catch (_) { continue; }
    if (role !== "AXButton" && role !== "AXRadioButton") continue;
    if (labels(e).some((l) => l.startsWith(label))) return e;
  }
  return null;
}

function press(e) {
  try { e.actions.byName("AXPress").perform(); } catch (_) { e.click(); }
}

function run(argv) {
  const [action, label, secs, next] = argv;
  const se = Application("System Events");
  const until = Date.now() + 1000 * Number(secs || 60);
  if (action === "dump") {
    const p = zaaheen(se);
    if (!p) return "no Zaaheen window";
    return everything(p)
      .map((e) => { let r = ""; try { r = e.role(); } catch (_) {} return r + " " + labels(e).join(" | "); })
      .filter((s) => /AXButton|AXRadioButton|AXStaticText|AXHeading/.test(s))
      .slice(0, 150)
      .join("\n");
  }
  while (Date.now() < until) {
    const p = zaaheen(se);
    if (p) {
      const e = find(p, label);
      if (action === "gone" && !e) return "gone: " + label;
      if (e && action === "wait") return "seen: " + label;
      if (e && action === "click") { press(e); return "pressed: " + label; }
      if (action === "advance") {
        if (find(p, next)) return "reached: " + next;
        if (e) press(e);
        delay(4);
        if (find(p, next)) return "reached: " + next;
      }
    }
    delay(1);
  }
  throw new Error(action + " ran out of time: " + label + (next ? " -> " + next : ""));
}
