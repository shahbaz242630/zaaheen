// The enquiry form (ADR-SEC-057): checks the fields as the Worker will,
// asks Turnstile for a token, and posts to this host only. Nothing is kept
// in the browser.
(() => {
  "use strict";
  const form = document.getElementById("enquiry");
  const send = document.getElementById("send");
  const status = document.getElementById("status");
  const check = document.getElementById("check");
  const CHECK_WAIT_MS = 10000;

  const TEXT = {
    missing: "Please fill in the fields marked as required, and tick the first box.",
    checking: "Sending…",
    received: "Thank you. We have your enquiry and will call you within one working day.",
    challenge: "We could not confirm you are a person. Please try again.",
    failed: "Something went wrong. Please try again, or email consulting@zaaheen.com.",
    limit: "We can't take more enquiries from here today. Please email consulting@zaaheen.com and we'll reply.",
  };

  let widget = null;

  async function turnstileReady() {
    for (let waited = 0; !window.turnstile && waited < CHECK_WAIT_MS; waited += 100) {
      await new Promise((r) => setTimeout(r, 100));
    }
    return Boolean(window.turnstile);
  }

  async function token() {
    if (!(await turnstileReady())) return null;
    return new Promise((resolve) => {
      const done = (t) => resolve(t);
      if (widget === null) {
        widget = window.turnstile.render(check, {
          sitekey: check.dataset.sitekey,
          callback: done,
          "error-callback": () => resolve(null),
        });
      } else {
        window.turnstile.reset(widget);
        window.turnstile.execute(widget, { callback: done });
      }
    });
  }

  function value(id) {
    return document.getElementById(id).value.trim();
  }

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    if (!form.checkValidity()) {
      status.textContent = TEXT.missing;
      form.reportValidity();
      return;
    }
    send.disabled = true;
    status.textContent = TEXT.checking;
    try {
      const passed = await token();
      if (!passed) {
        status.textContent = TEXT.challenge;
        return;
      }
      const body = {
        name: value("name"),
        email: value("email"),
        phone: value("phone"),
        company: value("company"),
        country: value("country"),
        size: value("size"),
        departments: [...form.querySelectorAll('input[name="departments"]:checked')].map((b) => b.value),
        message: value("message"),
        contactConsent: document.getElementById("contactConsent").checked,
        marketingConsent: document.getElementById("marketingConsent").checked,
        turnstile: passed,
      };
      const response = await fetch("/v1/enquiry", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      });
      const result = await response.json().catch(() => ({}));
      if (response.ok && result.kind === "received") {
        form.reset();
        form.hidden = true;
        form.parentElement.insertBefore(Object.assign(document.createElement("p"), { className: "done", textContent: TEXT.received }), form);
        return;
      }
      status.textContent =
        result.kind === "challenge" ? TEXT.challenge : result.kind === "limit" ? TEXT.limit : TEXT.failed;
    } catch {
      status.textContent = TEXT.failed;
    } finally {
      send.disabled = false;
    }
  });
})();
