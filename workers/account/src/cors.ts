// CORS for the website account page (AUTH-PAGES-DESIGN S85-1, ADR-SEC-043).
// Only the exact account origin, only on the routes it calls, POST with two
// request headers and no credentials (a Bearer header, never a cookie). Any
// other origin gets no CORS headers at all. The app sends no Origin and is
// unaffected.

export const WEB_ROUTES: ReadonlySet<string> = new Set(["/v1/web/plan", "/v1/checkout", "/v1/cancel"]);

/** The answer to an OPTIONS preflight on a web route. */
export function corsPreflight(request: Request, allowed: string | undefined): Response {
  const origin = request.headers.get("origin");
  if (allowed === undefined || origin !== allowed) {
    return new Response(null, { status: 403, headers: { vary: "Origin" } });
  }
  return new Response(null, {
    status: 204,
    headers: {
      "access-control-allow-origin": allowed,
      "access-control-allow-methods": "POST",
      "access-control-allow-headers": "authorization, content-type",
      "access-control-max-age": "600",
      vary: "Origin",
    },
  });
}

/** `response`, with the allow-origin header when the request came from our origin. */
export function withCors(response: Response, request: Request, allowed: string | undefined): Response {
  const origin = request.headers.get("origin");
  if (allowed === undefined || origin !== allowed) return response;
  const out = new Response(response.body, response);
  out.headers.set("access-control-allow-origin", allowed);
  out.headers.set("vary", "Origin");
  return out;
}
