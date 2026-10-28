// Ask Zaaheen's chat page (ADR-117, ADR-SEC-052). Runs inside the bubble's
// frame on ask.zaaheen.com. Everything shown is set as text, never HTML;
// links are built only for https://zaaheen.com pages. The conversation and
// the session live in this page only, and are gone when it closes.
"use strict";

(() => {
  const MAX_QUESTIONS = 6;
  const SITE = "https://zaaheen.com/";
  // The pages that may embed this one, and so may be told to close it.
  const PARENTS = ["https://zaaheen.com", "https://coaching.zaaheen.com"];
  const ERROR = "The question box can't answer right now. Please try again in a few minutes, or email customerservice@zaaheen.com.";

  const log = document.getElementById("log");
  const form = document.getElementById("ask");
  const input = document.getElementById("question");
  const send = document.getElementById("send");
  const check = document.getElementById("check");

  const turns = [];
  let session = null;
  let widget = null;

  // The bubble names its page (?from=); only a page on the list is ever told anything.
  const from = new URLSearchParams(window.location.search).get("from");
  const parent = PARENTS.find((origin) => origin === from);
  document.getElementById("close").addEventListener("click", () => {
    if (parent) window.parent.postMessage({ type: "zaaheen-ask:close" }, parent);
  });

  function add(text, who) {
    const item = document.createElement("li");
    item.className = `msg from-${who}`;
    item.textContent = text;
    log.appendChild(item);
    log.scrollTop = log.scrollHeight;
    return item;
  }

  function addPages(item, pages) {
    const list = document.createElement("ul");
    list.className = "pages";
    for (const page of pages) {
      if (typeof page !== "string" || !page.startsWith(SITE)) continue;
      const link = document.createElement("a");
      link.href = page;
      link.target = "_blank";
      link.rel = "noopener noreferrer";
      link.textContent = page.slice(SITE.length - 1);
      const entry = document.createElement("li");
      entry.appendChild(link);
      list.appendChild(entry);
    }
    if (list.childElementCount > 0) item.appendChild(list);
  }

  // A Turnstile token, asked for only when there is no session yet.
  function token() {
    return new Promise((resolve) => {
      if (!window.turnstile) return resolve(null);
      const done = (value) => resolve(value || null);
      if (widget === null) {
        widget = window.turnstile.render(check, {
          sitekey: check.dataset.sitekey,
          appearance: "interaction-only",
          execution: "execute",
          callback: done,
          "error-callback": () => done(null),
          "expired-callback": () => done(null),
        });
      } else {
        window.turnstile.reset(widget);
      }
      window.turnstile.execute(widget, { callback: done });
    });
  }

  async function post(body) {
    const response = await fetch("/v1/ask", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    return { status: response.status, data: await response.json().catch(() => ({})) };
  }

  async function ask(question) {
    const conversation = [...turns, { role: "user", text: question }];
    let result;
    if (session) result = await post({ turns: conversation, session });
    if (!result || result.status === 401) {
      session = null;
      const passed = await token();
      if (!passed) return { kind: "error" };
      result = await post({ turns: conversation, turnstile: passed });
    }
    if (typeof result.data.session === "string") session = result.data.session;
    return result.status === 200 ? result.data : { kind: "error" };
  }

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const question = input.value.trim();
    if (!question) return;
    input.value = "";
    input.disabled = send.disabled = true;
    add(question, "you");
    const waiting = add("…", "box");
    waiting.classList.add("waiting");

    let data;
    try {
      data = await ask(question);
    } catch {
      data = { kind: "error" };
    }
    waiting.remove();

    const text = typeof data.text === "string" ? data.text : ERROR;
    const item = add(text, "box");
    if (data.kind === "answer" && Array.isArray(data.pages)) addPages(item, data.pages);
    if (data.kind !== "error") {
      turns.push({ role: "user", text: question }, { role: "assistant", text });
    }

    const asked = turns.length / 2;
    if (data.kind === "resting" || asked >= MAX_QUESTIONS) return;
    input.disabled = send.disabled = false;
    input.focus();
  });

  input.addEventListener("keydown", (event) => {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      form.requestSubmit();
    }
  });
})();
