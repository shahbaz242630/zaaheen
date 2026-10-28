// Ask Zaaheen's Worker (ADR-117, ADR-SEC-052). The routes come in a later
// step; until then every request is a 404.
export default {
  async fetch(): Promise<Response> {
    return new Response("Not found", { status: 404 });
  },
} satisfies ExportedHandler;
