import { defineConfig, devices } from '@playwright/test';

// Sub-path deploy smoke test (e2e/subpath.spec.ts): builds the UI with
// OTS_BASE_PATH=/ots/ into dist-subpath/, serves it with `vite preview` (which
// honours the base and the SPA fallback, like a reverse proxy in front of the
// backend would), and loads deep links. Hermetic: the spec answers the API
// calls itself, so no backend is needed.
//
//   npm run e2e:subpath
//
// Kept apart from playwright.config.ts, whose web servers boot the Rust
// backend and the dev server for the demo suite.

const PORT = Number(process.env.OTS_SUBPATH_PORT ?? 4319);
const BASE = '/ots/';

export default defineConfig({
  testDir: './e2e',
  testMatch: 'subpath.spec.ts',
  timeout: 60_000,
  expect: { timeout: 15_000 },
  fullyParallel: false,
  workers: 1,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? [['github'], ['html', { open: 'never', outputFolder: 'playwright-report-subpath' }]] : 'list',
  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    trace: 'on-first-retry',
    screenshot: 'only-on-failure',
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
  webServer: {
    command:
      `npx vite build --outDir dist-subpath --emptyOutDir && ` +
      `npx vite preview --outDir dist-subpath --host 127.0.0.1 --port ${PORT} --strictPort`,
    url: `http://127.0.0.1:${PORT}${BASE}`,
    reuseExistingServer: false,
    timeout: 600_000,
    stdout: 'pipe',
    stderr: 'pipe',
    env: {
      OTS_BASE_PATH: BASE,
      // Nothing listens here: a request the base-aware preview proxy should
      // have kept on the SPA (or that the spec failed to answer) errors out.
      OTS_BACKEND_URL: 'http://127.0.0.1:9',
      LD_DISCOVERY: '',
    },
  },
});
