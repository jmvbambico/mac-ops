/**
 * Vitest config.
 *
 * A separate file rather than `vite.config.ts`: that file is the Tauri dev
 * server's config (fixed port, HMR wiring) and is not ours to change. This
 * one is deliberately minimal — the tests are pure TypeScript with no DOM, so
 * the default `node` environment is exactly right and no jsdom is needed.
 */
import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
