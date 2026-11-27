// Tests run inside workerd, the runtime the Worker ships on, with a local
// D1 database built from migrations/ for each test file.
import { cloudflareTest, readD1Migrations } from "@cloudflare/vitest-plugin";
import { defineConfig } from "vitest/config";

export default defineConfig(async () => {
  const migrations = await readD1Migrations("./migrations");
  return {
    plugins: [
      cloudflareTest({
        wrangler: { configPath: "./wrangler.jsonc" },
        miniflare: {
          bindings: { TEST_MIGRATIONS: migrations },
          // A stand-in for zaaheen-consulting, so the CONSULTING service
          // binding in wrangler.jsonc resolves in tests (s130).
          workers: [
            {
              name: "zaaheen-consulting",
              modules: true,
              compatibilityDate: "2026-09-01",
              script:
                'import { WorkerEntrypoint } from "cloudflare:workers";' +
                "export class Nightly extends WorkerEntrypoint { async run() {} }" +
                'export default { fetch() { return new Response("stub"); } };',
            },
          ],
        },
      }),
    ],
  };
});
