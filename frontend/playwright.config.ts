import { defineConfig, devices } from '@playwright/test';

// Browser e2e for the bundled "Open Triplestore" demo. Boots the Rust backend
// (seeded on first-admin registration in global-setup) and the Vite dev server,
// then drives the demo through a real browser.
//
// Override any of these with env vars when running against an already-running
// stack (e.g. OTS_FRONTEND_URL=http://localhost:5173 npm run e2e).

const BACKEND = process.env.OTS_BACKEND_URL ?? 'http://localhost:7878';
const FRONTEND = process.env.OTS_FRONTEND_URL ?? 'http://localhost:5173';

// All demo standards except SAML (which needs system xmlsec libs and isn't
// exercised by the UI demo). Override with OTS_BACKEND_FEATURES if desired.
const BACKEND_FEATURES =
  process.env.OTS_BACKEND_FEATURES ??
  'rdf-12,owl2-rl,owl2-el,owl2-ql,owl2-dl,text-search,ldp,shex,swrl';

const backendCmd =
  process.env.OTS_BACKEND_CMD ??
  `cargo run --manifest-path ../Cargo.toml --features "${BACKEND_FEATURES}" -- ` +
    `--bind 127.0.0.1 --port 7878 --data-dir ./e2e-data --log-level warn`;

export default defineConfig({
  testDir: './e2e',
  testMatch: '**/*.spec.ts',
  timeout: 30_000,
  expect: { timeout: 10_000 },
  fullyParallel: false,
  workers: 1,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? [['github'], ['html', { open: 'never' }]] : 'list',
  globalSetup: './e2e/global-setup.ts',
  use: {
    baseURL: FRONTEND,
    trace: 'on-first-retry',
    screenshot: 'only-on-failure',
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
  webServer: [
    {
      command: backendCmd,
      url: `${BACKEND}/health`,
      reuseExistingServer: !process.env.CI,
      timeout: 300_000,
      stdout: 'pipe',
      stderr: 'pipe',
      env: {
        JWT_SECRET: process.env.JWT_SECRET ?? 'e2e_jwt_secret_must_be_32_chars_xx',
        // The suite drives many requests from one IP (repeated logins in
        // beforeEach, browsing + running saved queries), which would otherwise
        // trip the per-IP auth/SPARQL rate limiters and cause spurious 429s.
        // Relax them for the test backend only (secure-by-default in production).
        RATE_LIMIT_DISABLED: process.env.RATE_LIMIT_DISABLED ?? '1',
        // Hermetic e2e: an empty SEED_IFC_URL skips the Schependomlaan IFC demo
        // seed — a ~49 MB download plus a multi-minute debug-build import whose
        // store write locks starve the suite (no spec depends on that data).
        SEED_IFC_URL: process.env.SEED_IFC_URL ?? '',
      },
    },
    {
      command: 'npm run dev',
      url: FRONTEND,
      reuseExistingServer: !process.env.CI,
      timeout: 120_000,
    },
  ],
});
