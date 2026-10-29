// The go-live eval only (eval/ask.eval.ts): plain Node, the real model, a
// real key from the environment. Never part of `npm test` or CI.
import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["eval/**/*.eval.ts"],
    environment: "node",
    fileParallelism: false,
    maxConcurrency: 1,
  },
});
