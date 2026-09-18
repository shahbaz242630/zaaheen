// Small HTTP helpers shared by the routes.

/** A JSON answer that no cache or proxy may keep. */
export function jsonResponse(status: number, body: unknown, headers: Record<string, string> = {}): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json; charset=utf-8", "cache-control": "no-store", ...headers },
  });
}

/** A fixed-text error: the code is ours, never an upstream message. */
export function errorResponse(status: number, code: string, headers: Record<string, string> = {}): Response {
  return jsonResponse(status, { error: code }, headers);
}

/** The request body as text, or `null` when it is larger than `maxBytes`. */
export async function readCapped(request: Request, maxBytes: number): Promise<string | null> {
  if (request.body === null) return "";
  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.byteLength;
    if (total > maxBytes) {
      await reader.cancel();
      return null;
    }
    chunks.push(value);
  }
  const all = new Uint8Array(total);
  let offset = 0;
  for (const c of chunks) {
    all.set(c, offset);
    offset += c.byteLength;
  }
  return new TextDecoder().decode(all);
}

/** The bearer token from `Authorization`, or `null`. */
export function bearerToken(request: Request): string | null {
  const m = /^Bearer ([\x21-\x7e]{1,4096})$/.exec(request.headers.get("authorization") ?? "");
  return m?.[1] ?? null;
}
