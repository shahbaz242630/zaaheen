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
    missing: "Please fill in every field that is not marked optional, and tick the first box.",
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

  // "Another country": the visitor types the whole number, + code first.
  const phoneCode = document.getElementById("phoneCode");
  const phone = document.getElementById("phone");
  const phoneHelp = document.getElementById("phoneHelp");
  function phoneMode() {
    const other = phoneCode.value === "other";
    phone.placeholder = other ? "+44 20 1234 5678" : "50 123 4567";
    phone.autocomplete = other ? "tel" : "tel-national";
    phoneHelp.textContent = other
      ? "Type the full number, starting with + and the country code."
      : "Choose the country code, then type your number. A first 0 is fine.";
  }
  // The same shapes the Worker accepts (src/validate.ts), so a mistyped
  // number is caught here with a clear message, not refused after sending.
  // Digit counts too: at least 4 after the code (not counting a leading 0,
  // which the Worker drops), at most 15 in all; 6 to 15 for a full number.
  const digitCount = (s) => s.replace(/[^0-9]/g, "").length;
  function phoneCheck() {
    const v = phone.value.trim();
    const other = phoneCode.value === "other";
    let ok;
    if (other) {
      ok = /^\+[0-9][0-9 ()./-]{5,24}$/.test(v) && digitCount(v) >= 6 && digitCount(v) <= 15;
    } else {
      const local = digitCount(v) - (v.replace(/[^0-9]/g, "").startsWith("0") ? 1 : 0);
      ok = /^[0-9 ()./-]{4,20}$/.test(v) && local >= 4 && digitCount(phoneCode.value) + local <= 15;
    }
    phone.setCustomValidity(
      v === "" || ok
        ? ""
        : other
          ? "Type the full number, starting with + and the country code."
          : "Type the number without the country code, digits only. Check it is complete.",
    );
  }
  phoneCode.addEventListener("change", () => { phoneMode(); phoneCheck(); });
  phone.addEventListener("input", phoneCheck);
  phoneMode();

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
        phoneCode: value("phoneCode"),
        phone: value("phone"),
        company: value("company"),
        country: value("country"),
        size: value("size"),
        turnover: value("turnover"),
        sector: value("sector"),
        callTime: value("callTime"),
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
