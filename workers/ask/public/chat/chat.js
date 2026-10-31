// Ask Zaaheen's chat page (ADR-117, ADR-SEC-052). Runs inside the bubble's
// frame on ask.zaaheen.com. Everything shown is set as text, never HTML;
// links are built only for https://zaaheen.com pages. The conversation and
// the session live in this page only, and are gone when it closes.
"use strict";

(() => {
  const MAX_QUESTIONS = 6;
  const SITE = "https://zaaheen.com/";
  // The pages that may embed this one, and so may be told to close it.
  const PARENTS = ["https://zaaheen.com"];
  // The box's own words in each language of zaaheen.com (the bubble passes
  // ?lang=). English is the founder-approved text in src/text.ts; the others
  // are its translations, in the site's own terms (test/chat-page.test.ts).
  // Answers are written by the model in the visitor's language (src/prompt.ts).
  const WORDS = {
    "en": {
      "title": "Ask Zaaheen",
      "close": "Close",
      "greeting": "Hi! Ask me anything about Zaaheen: how it works, connecting your AI apps, pricing or your trial.",
      "question": "Your question",
      "placeholder": "Type your question",
      "send": "Send",
      "note": "Answers come from our website and can be wrong. For anything important, check the linked page or email customerservice@zaaheen.com.",
      "unavailable": "The question box can't answer right now. Please try again in a few minutes, or email customerservice@zaaheen.com.",
      "end": "That's the most questions for one chat. To ask more, close this box and open it again, or email customerservice@zaaheen.com.",
      "turned_away": "I can only help with questions about Zaaheen. For anything else, please email customerservice@zaaheen.com.",
      "resting": "The question box is resting for now. Please email customerservice@zaaheen.com and we'll help.",
      "region": "The question box isn't available in your region. Please email customerservice@zaaheen.com and we'll help."
    },
    "es": {
      "title": "Pregunta a Zaaheen",
      "close": "Cerrar",
      "greeting": "¡Hola! Pregúntame lo que quieras sobre Zaaheen: cómo funciona, cómo conectar tus apps de IA, los precios o tu prueba gratis.",
      "question": "Tu pregunta",
      "placeholder": "Escribe tu pregunta",
      "send": "Enviar",
      "note": "Las respuestas salen de nuestro sitio web y pueden equivocarse. Para algo importante, revisa la página enlazada o escribe a customerservice@zaaheen.com.",
      "unavailable": "El cuadro de preguntas no puede responder ahora. Vuelve a intentarlo en unos minutos o escribe a customerservice@zaaheen.com.",
      "end": "Has llegado al máximo de preguntas en un chat. Para preguntar más, cierra este cuadro y vuelve a abrirlo, o escribe a customerservice@zaaheen.com.",
      "turned_away": "Solo puedo ayudarte con preguntas sobre Zaaheen. Para cualquier otra cosa, escribe a customerservice@zaaheen.com.",
      "resting": "El cuadro de preguntas está descansando por ahora. Escribe a customerservice@zaaheen.com y te ayudaremos.",
      "region": "El cuadro de preguntas no está disponible en tu región. Escribe a customerservice@zaaheen.com y te ayudaremos."
    },
    "pt": {
      "title": "Pergunte ao Zaaheen",
      "close": "Fechar",
      "greeting": "Olá! Pergunte o que quiser sobre o Zaaheen: como funciona, como conectar seus apps de IA, preços ou seu teste grátis.",
      "question": "Sua pergunta",
      "placeholder": "Digite sua pergunta",
      "send": "Enviar",
      "note": "As respostas vêm do nosso site e podem estar erradas. Para algo importante, confira a página indicada ou escreva para customerservice@zaaheen.com.",
      "unavailable": "A caixa de perguntas não consegue responder agora. Tente de novo em alguns minutos ou escreva para customerservice@zaaheen.com.",
      "end": "Você chegou ao máximo de perguntas em um chat. Para perguntar mais, feche esta caixa e abra de novo, ou escreva para customerservice@zaaheen.com.",
      "turned_away": "Só posso ajudar com perguntas sobre o Zaaheen. Para qualquer outra coisa, escreva para customerservice@zaaheen.com.",
      "resting": "A caixa de perguntas está descansando por enquanto. Escreva para customerservice@zaaheen.com e nós ajudamos.",
      "region": "A caixa de perguntas não está disponível na sua região. Escreva para customerservice@zaaheen.com e nós ajudamos."
    },
    "de": {
      "title": "Zaaheen fragen",
      "close": "Schließen",
      "greeting": "Hallo! Frag mich alles über Zaaheen: wie es funktioniert, wie du deine KI-Apps verbindest, Preise oder deine kostenlose Testphase.",
      "question": "Deine Frage",
      "placeholder": "Schreib deine Frage",
      "send": "Senden",
      "note": "Die Antworten stammen von unserer Website und können falsch sein. Prüf bei Wichtigem die verlinkte Seite oder schreib an customerservice@zaaheen.com.",
      "unavailable": "Das Fragefeld kann gerade nicht antworten. Versuch es in ein paar Minuten noch einmal oder schreib an customerservice@zaaheen.com.",
      "end": "Mehr Fragen gehen in diesem Chat nicht. Um weiter zu fragen, schließ dieses Feld und öffne es erneut, oder schreib an customerservice@zaaheen.com.",
      "turned_away": "Ich kann nur bei Fragen zu Zaaheen helfen. Für alles andere schreib bitte an customerservice@zaaheen.com.",
      "resting": "Das Fragefeld macht gerade Pause. Schreib an customerservice@zaaheen.com, dann helfen wir dir.",
      "region": "Das Fragefeld ist in deiner Region nicht verfügbar. Schreib an customerservice@zaaheen.com, dann helfen wir dir."
    },
    "fr": {
      "title": "Demander à Zaaheen",
      "close": "Fermer",
      "greeting": "Bonjour ! Posez-moi vos questions sur Zaaheen : son fonctionnement, la connexion de vos applications d’IA, les prix ou votre essai gratuit.",
      "question": "Votre question",
      "placeholder": "Écrivez votre question",
      "send": "Envoyer",
      "note": "Les réponses viennent de notre site et peuvent être fausses. Pour tout ce qui est important, vérifiez la page indiquée ou écrivez à customerservice@zaaheen.com.",
      "unavailable": "La boîte à questions ne peut pas répondre pour le moment. Réessayez dans quelques minutes ou écrivez à customerservice@zaaheen.com.",
      "end": "Vous avez atteint le nombre maximum de questions pour une discussion. Pour en poser d’autres, fermez cette boîte et rouvrez-la, ou écrivez à customerservice@zaaheen.com.",
      "turned_away": "Je ne peux répondre qu’aux questions sur Zaaheen. Pour tout le reste, écrivez à customerservice@zaaheen.com.",
      "resting": "La boîte à questions fait une pause pour l’instant. Écrivez à customerservice@zaaheen.com et nous vous aiderons.",
      "region": "La boîte à questions n’est pas disponible dans votre région. Écrivez à customerservice@zaaheen.com et nous vous aiderons."
    }
  };
  // The Worker's reply kinds that carry one of our own sentences (src/text.ts).
  const FIXED = ["turned_away", "resting", "unavailable", "region"];
  // zaaheen.com's pages that exist in every language (site/src/i18n/locales.ts
  // TRANSLATED): a link to one opens it in the visitor's language.
  const TRANSLATED = [
    "/", "/products/", "/pricing/",
    "/docs/", "/docs/getting-started/", "/docs/subscribe/", "/docs/connect-claude/", "/docs/connect-claude-code/", "/docs/connect-cursor/", "/docs/connect-chatgpt/", "/docs/connect-codex/", "/docs/connect-antigravity/", "/docs/connect-other-apps/", "/docs/troubleshooting/", "/docs/contact/",
    "/learn/", "/learn/ai-memory-apps-compared/", "/learn/ai-memory-tool-or-notes-file/", "/learn/best-memory-mcp-server-for-claude-code/", "/learn/chatgpt-memory-vs-claude-memory/", "/learn/does-claude-code-remember-between-sessions/", "/learn/mem0-alternative/", "/learn/memoryplugin-alternative/", "/learn/obsidian-vs-ai-memory-app/", "/learn/one-memory-across-claude-code-cursor-and-codex/", "/learn/share-memory-between-chatgpt-and-claude/", "/learn/supermemory-alternative/", "/learn/switch-from-chatgpt-to-claude/"
  ];

  const log = document.getElementById("log");
  const form = document.getElementById("ask");
  const input = document.getElementById("question");
  const send = document.getElementById("send");
  const check = document.getElementById("check");

  const turns = [];
  const CHECK_WAIT_MS = 15000;
  // Long enough for a visitor to answer a check Cloudflare chooses to show.
  const CHECK_ANSWER_MS = 60000;
  let session = null;
  let widget = null;

  // The bubble names its page (?from=); only a page on the list is ever told anything.
  const params = new URLSearchParams(window.location.search);
  const from = params.get("from");
  const parent = PARENTS.find((origin) => origin === from);

  // The page's language (?lang=); anything not on the list is English.
  const lang = Object.hasOwn(WORDS, params.get("lang")) ? params.get("lang") : "en";
  const t = WORDS[lang];
  if (lang !== "en") {
    document.documentElement.lang = lang;
    document.title = t.title;
    document.querySelector("main").setAttribute("aria-label", t.title);
    document.querySelector("h1").textContent = t.title;
    document.getElementById("close").setAttribute("aria-label", t.close);
    log.querySelector("li").textContent = t.greeting;
    document.querySelector('label[for="question"]').textContent = t.question;
    input.placeholder = t.placeholder;
    send.textContent = t.send;
    document.querySelector(".note").textContent = t.note;
  }

  // A link to a page that exists in the visitor's language opens that version.
  function localPage(page) {
    if (lang === "en") return page;
    const path = page.slice(SITE.length - 1).replace(/[?#].*$/, "");
    return TRANSLATED.includes(path) ? `${SITE}${lang}${page.slice(SITE.length - 1)}` : page;
  }
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
      const href = localPage(page);
      const link = document.createElement("a");
      link.href = href;
      link.target = "_blank";
      link.rel = "noopener noreferrer";
      link.textContent = href.slice(SITE.length - 1);
      const entry = document.createElement("li");
      entry.appendChild(link);
      list.appendChild(entry);
    }
    if (list.childElementCount > 0) item.appendChild(list);
  }

  // The check's script loads by itself, and a quick first question can get here
  // before it does (2026-10-31: the box said "can't answer" with no check and no
  // request). Wait for it; give up only if it never comes.
  async function checkReady() {
    for (let waited = 0; !window.turnstile && waited < CHECK_WAIT_MS; waited += 100) {
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    return Boolean(window.turnstile);
  }

  // A Turnstile token, asked for only when there is no session yet.
  async function token() {
    if (!(await checkReady())) return null;
    return new Promise((resolve) => {
      // A stalled check must not leave the visitor watching "…" for ever.
      const limit = setTimeout(() => resolve(null), CHECK_ANSWER_MS);
      const done = (value) => {
        clearTimeout(limit);
        resolve(value || null);
      };
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
      // With no pass, still ask: the Worker refuses it and its log records why.
      result = await post(passed ? { turns: conversation, turnstile: passed } : { turns: conversation });
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

    // Our own sentences come in the page's language; an answer is the model's.
    const text = data.kind === "answer" && typeof data.text === "string"
      ? data.text
      : FIXED.includes(data.kind) ? t[data.kind] : t.unavailable;
    const item = add(text, "box");
    if (data.kind === "answer" && Array.isArray(data.pages)) addPages(item, data.pages);
    if (data.kind !== "error") {
      turns.push({ role: "user", text: question }, { role: "assistant", text });
    }

    const asked = turns.length / 2;
    if (data.kind === "resting" || data.kind === "region") return;
    if (asked >= MAX_QUESTIONS) {
      add(t.end, "box");
      return;
    }
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
