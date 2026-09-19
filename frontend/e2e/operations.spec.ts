import { test, expect, request as pwRequest, type APIRequestContext, type Page } from '@playwright/test';

// E2E for the admin Operations page (/admin/operations): what the page says
// must be what the API says. The backend the suite runs against may be a
// stand-alone store (role none, capture off — CI) or a replication leader
// (the two-container example), so every expectation is read from the API
// first and then looked for on the page.
//
// Auth/navigation pattern mirrors dataset-validate.spec.ts: the app keeps its
// access token in memory, so we sign in through the form and navigate the SPA
// client-side.

const BACKEND = process.env.OTS_BACKEND_URL ?? 'http://localhost:7878';
const ADMIN = { username: 'e2e-admin', password: 'e2e-password-123' };

let api: APIRequestContext;
let token = '';

async function apiLogin(): Promise<void> {
  const res = await api.post('/api/auth/login', { data: ADMIN });
  expect(res.ok(), `login failed: ${res.status()}`).toBeTruthy();
  token = (await res.json()).access_token;
}

const authHeaders = () => ({ authorization: `Bearer ${token}` });

async function signIn(page: Page): Promise<void> {
  await page.goto('/login');
  await page.getByLabel('Username').fill(ADMIN.username);
  await page.getByLabel('Password').fill(ADMIN.password);
  await page.getByRole('button', { name: 'Login' }).click();
  await expect(page).toHaveURL(/\/$/);
}

async function clientNavigate(page: Page, path: string): Promise<void> {
  await page.evaluate((p) => {
    window.history.pushState({}, '', p);
    window.dispatchEvent(new PopStateEvent('popstate'));
  }, path);
}

test.beforeAll(async () => {
  api = await pwRequest.newContext({ baseURL: BACKEND });
  await apiLogin();
});

test.afterAll(async () => {
  await api.dispose();
});

test.beforeEach(async ({ page }) => {
  await signIn(page);
});

test('the sidebar offers Node status to an admin and the page matches the API', async ({ page }) => {
  const replication = await (await api.get('/api/replication/status')).json();
  const changes = await (await api.get('/api/admin/changes/status', { headers: authHeaders() })).json();
  const telemetry = await (await api.get('/api/admin/telemetry', { headers: authHeaders() })).json();

  await page.getByRole('link', { name: 'Node status' }).click();
  await expect(page).toHaveURL(/\/admin\/operations$/);
  await expect(page.getByRole('heading', { name: 'Node status', level: 2 })).toBeVisible();

  // Replication: the badge is the API's role in a word.
  const state =
    replication.role === 'none' ? 'Not replicating'
    : replication.role === 'leader' ? 'Leader'
    : replication.lag_rows === 0 && replication.healthy ? 'In sync'
    : /Catching up|Stale|Error/;
  // Cards are found by their heading: the replication card's own text
  // mentions the change log, so text filters would cross-match.
  const cardWith = (title: string) =>
    page.locator('.card').filter({ has: page.getByRole('heading', { name: title, level: 3 }) });
  const card = cardWith('Replication');
  await expect(card.locator('.state-badge')).toHaveText(state);
  await expect(card).toContainText(replication.node_id);

  // Queries: every exit, in the order the store tries them, with a meaning.
  const exits = cardWith('Queries').locator('table').first().locator('tbody tr td:first-child code');
  await expect(exits).toHaveText(['cache_hit', 'fast_count', 'shards', 'columnar', 'full_copy', 'engine']);
  await expect(page.getByText('the cached result is returned')).toBeVisible();
  // The exact total the API reported is on the page (the page loaded after
  // the API call, so it may have grown — it can only be higher).
  const totalCell = page.locator('.stat-card', { hasText: 'Queries since start' }).locator('.stat-value');
  const shown = Number((await totalCell.textContent())?.replace(/\D/g, ''));
  expect(shown).toBeGreaterThanOrEqual(Number(telemetry.queries.total));

  // Change log: capturing or off, and when off, the switch.
  const log = cardWith('Change log');
  if (changes.enabled) {
    await expect(log.locator('.state-badge')).toHaveText('Capturing');
    await expect(log).toContainText('Cursors');
  } else {
    await expect(log.locator('.state-badge')).toHaveText('Off');
    await expect(log).toContainText('OTS_CHANGE_CAPTURE=on');
    await expect(log).toContainText('×2.5–4');
  }
  await expect(log).toContainText(`${changes.retention_days} days`);
});

test('the page refreshes on its own and pauses on request', async ({ page }) => {
  let calls = 0;
  await page.route('**/api/admin/telemetry', async (route) => {
    calls += 1;
    await route.continue();
  });
  await clientNavigate(page, '/admin/operations');
  await expect(page.getByRole('heading', { name: 'Node status', level: 2 })).toBeVisible();
  await expect.poll(() => calls, { timeout: 8_000 }).toBeGreaterThanOrEqual(2);

  await page.getByRole('button', { name: 'Pause' }).click();
  const paused = calls;
  await page.waitForTimeout(6_000);
  expect(calls).toBe(paused);

  await page.getByRole('button', { name: 'Resume' }).click();
  await expect.poll(() => calls, { timeout: 3_000 }).toBeGreaterThan(paused);
});

test('a signed-out visitor is not offered the page and its data needs an admin', async ({ page, browser }) => {
  // The admin-only bodies answer 401 without a token.
  const anon = await pwRequest.newContext({ baseURL: BACKEND });
  expect((await anon.get('/api/admin/telemetry')).status()).toBe(401);
  expect((await anon.get('/api/admin/changes/status')).status()).toBe(401);
  // …while the replication status is public, beside /livez.
  expect((await anon.get('/api/replication/status')).ok()).toBeTruthy();
  await anon.dispose();

  // The signed-in page has the link; a signed-out visitor in a fresh browser
  // context (no session cookie) does not, and is sent home from the route.
  await expect(page.getByRole('link', { name: 'Node status' })).toHaveCount(1);
  const visitor = await browser.newContext();
  const fresh = await visitor.newPage();
  await fresh.goto('/');
  await expect(fresh.getByRole('heading', { level: 1 }).first()).toBeVisible();
  await expect(fresh.getByRole('link', { name: 'Node status' })).toHaveCount(0);
  await fresh.goto('/admin/operations');
  await expect(fresh).not.toHaveURL(/\/admin\/operations$/);
  await visitor.close();
});
