// Tests run inside workerd, the runtime the Worker ships on, so WebCrypto's
// Ed25519 is the production implementation, not Node's.
import { cloudflareTest } from "@cloudflare/vitest-plugin";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [
    cloudflareTest({
      wrangler: { configPath: "./wrangler.jsonc" },
    }),
  ],
});
