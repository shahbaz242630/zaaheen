// Bindings that exist only in tests (vitest.config.ts).
declare namespace Cloudflare {
  interface Env {
    TEST_MIGRATIONS: import("cloudflare:test").D1Migration[];
  }
}

// Vite's raw imports: a file's text, for tests that read the chat page.
declare module "*?raw" {
  const text: string;
  export default text;
}
