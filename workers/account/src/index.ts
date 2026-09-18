// The account Worker (SIGNIN-DESIGN.md §5). S2 step 1 builds the offline
// core (lease signing, billing derivation, the lease decision); the routes
// arrive with step 2. Until then every request is a 404.
export default {
  async fetch(): Promise<Response> {
    return new Response(null, { status: 404 });
  },
} satisfies ExportedHandler<Env>;
