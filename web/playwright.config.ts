import { defineConfig } from "@playwright/test";

// The tests drive a real program with the core and this crate's apps
// (examples/dev.rs, with a stand-in for chat's rooms), on SQLite in a
// throwaway directory, its media node on loopback. Browsers use fake
// cameras and microphones.
const port = Number(process.env.ORTUS_E2E_PORT ?? 7988);
const binary = process.env.ORTUS_E2E_BINARY ?? "../target/debug/examples/dev";

export default defineConfig({
  testDir: "e2e",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: 0,
  reporter: [["list"]],
  globalSetup: "./e2e/global-setup.ts",
  use: {
    baseURL: `http://localhost:${port}`,
    viewport: { width: 1440, height: 900 },
    colorScheme: "dark",
    timezoneId: "UTC",
    storageState: "test-results/owner.json",
    permissions: ["camera", "microphone"],
    launchOptions: { args: ["--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream", "--autoplay-policy=no-user-gesture-required"] },
  },
  projects: [{ name: "chromium", use: { browserName: "chromium" } }],
  webServer: [
    {
      command: `rm -rf test-results/data && ${binary}`,
      env: {
        ORTUS_ROLES: "web,worker,media",
        ORTUS_WEB_PORT: String(port),
        ORTUS_DATA_DIR: "test-results/data",
        ORTUS_MEDIA_LISTEN: "127.0.0.1:0",
        ORTUS_SETUP_TOKEN: "e2e-setup-token",
        ORTUS_PUBLIC_URL: `http://localhost:${port}`,
      },
      url: `http://127.0.0.1:${port}/api/v1/health`,
      reuseExistingServer: false,
      timeout: 30_000,
    },
  ],
});
