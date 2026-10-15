import { defineConfig } from "@playwright/test";
import baseConfig from "./playwright.config";

// Configuration CRUD is mocked; this suite only needs the dashboard server.
const servers = Array.isArray(baseConfig.webServer) ? baseConfig.webServer : [];

export default defineConfig({
  ...baseConfig,
  testMatch: "micro-services.spec.ts",
  webServer: servers.filter((server) => server.command === "npm run dev")
    .map((server) => ({ ...server, reuseExistingServer: false })),
});
