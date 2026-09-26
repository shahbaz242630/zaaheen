// The origin's only Trusted Types policy (AUTH-PAGES-DESIGN amendment S1-1).
// Loaded first on every page, before any other script; the CSP's
// "trusted-types default" means no other policy can ever be created.
// The one script URL any code here may set is Cloudflare's bot-check loader,
// exactly (clerk-js inserts it for sign-up). Everything else is refused:
// other script URLs, HTML from strings, script text, blob workers.
(function () {
  'use strict';
  if (!window.trustedTypes || !window.trustedTypes.createPolicy) return;
  var BOT_CHECK = 'https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit';
  window.trustedTypes.createPolicy('default', {
    createScriptURL: function (url) {
      if (url === BOT_CHECK) return url;
      throw new TypeError('script URL refused');
    },
    createHTML: function () { throw new TypeError('HTML refused'); },
    createScript: function () { throw new TypeError('script refused'); },
  });
})();
