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
// exercised by the UI demo), plus `asset-archive` for the ICDD container
// flows. Override with OTS_BACKEND_FEATURES if desired; keep this list and
// .github/workflows/e2e.yml in step.
const BACKEND_FEATURES =
  process.env.OTS_BACKEND_FEATURES ??
  'rdf-12,owl2-rl,owl2-el,owl2-ql,owl2-dl,text-search,ldp,shex,swrl,asset-archive';

const backendCmd =
  process.env.OTS_BACKEND_CMD ??
  `cargo run --manifest-path ../Cargo.toml --features "${BACKEND_FEATURES}" -- ` +
    `--bind 127.0.0.1 --port 7878 --data-dir ./e2e-data --log-level warn`;

export default defineConfig({
  testDir: './e2e',
  testMatch: '**/*.spec.ts',
  // The sub-path smoke test brings its own build and server (playwright.subpath.config.ts).
  testIgnore: '**/subpath.spec.ts',
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
        OTS_JWT_SECRET: process.env.OTS_JWT_SECRET ?? 'e2e_jwt_secret_must_be_32_chars_xx',
        // The suite drives many requests from one IP (repeated logins in
        // beforeEach, browsing + running saved queries), which would otherwise
        // trip the per-IP auth/SPARQL rate limiters and cause spurious 429s.
        // Relax them for the test backend only (secure-by-default in production).
        OTS_RATE_LIMIT_ENABLED: process.env.OTS_RATE_LIMIT_ENABLED ?? 'false',
        // Hermetic e2e: OTS_OFFLINE skips every startup download — the IFC
        // demo models (~49 MB plus a multi-minute debug-build import whose
        // store write locks starve the suite) and the LOV corpus. No spec
        // depends on that data.
        OTS_OFFLINE: process.env.OTS_OFFLINE ?? 'true',
        // The LDES sync spec mirrors one of this backend's own streams, so the
        // backend must be allowed to fetch from itself (deny by default).
        OTS_LDES_ALLOWLIST: process.env.OTS_LDES_ALLOWLIST ?? `${BACKEND}/`,
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
