import { test, expect, request as pwRequest, type APIRequestContext, type Page } from '@playwright/test';

// The admin maintenance pages against a real backend: take a backup, verify
// it, stage a restore through the app's confirmation dialog (typing the
// backup's id) and cancel it again; rebuild the full-text index; and add an
// LDES sync job that mirrors one of this server's own streams (the e2e
// backend allow-lists itself in OTS_LDES_ALLOWLIST), run it, and forget it.

const BACKEND = process.env.OTS_BACKEND_URL ?? 'http://localhost:7878';
const ADMIN = { username: 'e2e-admin', password: 'e2e-password-123' };
const RUN = `${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;

let api: APIRequestContext;
let token = '';
const authHeaders = () => ({ authorization: `Bearer ${token}` });

async function signIn(page: Page): Promise<void> {
  await page.goto('/login');
  await page.getByLabel('Username').fill(ADMIN.username);
  await page.getByLabel('Password').fill(ADMIN.password);
  await page.getByRole('button', { name: 'Login' }).click();
  await expect(page).toHaveURL(/\/$/);
}

test.beforeAll(async () => {
  api = await pwRequest.newContext({ baseURL: BACKEND });
  const login = await api.post('/api/auth/login', { data: ADMIN });
  expect(login.ok(), `login failed: ${login.status()}`).toBeTruthy();
  token = (await login.json()).access_token;
});

test.afterAll(async () => {
  // Never leave a restore staged: the next start of a reused data dir would apply it.
  await api?.delete('/api/admin/backup/restore', { headers: authHeaders() }).catch(() => undefined);
  await api?.dispose();
});

test.beforeEach(async ({ page }) => {
  await signIn(page);
});

test('backups: create, verify, stage a restore in the app dialog, cancel it', async ({ page }) => {
  await page.getByRole('link', { name: 'Backups & index' }).click();
  await expect(page).toHaveURL(/\/admin\/maintenance$/);
  const card = page.getByTestId('backups-card');
  await expect(card.getByTestId('s3-status')).toBeVisible({ timeout: 20_000 });

  await card.getByRole('button', { name: 'Back up now' }).click();
  const table = card.getByTestId('backups-table');
  await expect(table.locator('tbody tr').first()).toBeVisible({ timeout: 60_000 });
  const list = await (await api.get('/api/admin/backup', { headers: authHeaders() })).json();
  const id: string = list[0].id;
  const row = table.locator('tr').filter({ hasText: id });

  await row.getByRole('button', { name: 'Verify' }).click();
  await expect(row.getByText('Intact')).toBeVisible({ timeout: 30_000 });

  await row.getByRole('button', { name: 'Restore…' }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toContainText('replaces ALL data');
  const stage = dialog.getByRole('button', { name: 'Stage the restore' });
  await expect(stage).toBeDisabled();
  await dialog.locator('#confirm-phrase-input').fill(id);
  await stage.click();
  await expect(card.getByTestId('pending-restore')).toContainText(id, { timeout: 30_000 });
  const staged = await (await api.get('/api/admin/backup/status', { headers: authHeaders() })).json();
  expect(staged.pending_restore.id).toBe(id);

  await card.getByRole('button', { name: 'Cancel the restore' }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Cancel restore' }).click();
  await expect(card.getByTestId('pending-restore')).toHaveCount(0);
  const after = await (await api.get('/api/admin/backup/status', { headers: authHeaders() })).json();
  expect(after.pending_restore).toBeNull();
});

test('text index: status and a confirmed rebuild', async ({ page }) => {
  await page.getByRole('link', { name: 'Backups & index' }).click();
  const card = page.getByTestId('index-card');
  const status = card.getByTestId('index-status');
  await expect(status).toBeVisible({ timeout: 20_000 });
  await card.getByRole('button', { name: 'Rebuild index' }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Rebuild index' }).click();
  await expect(status).toContainText(/literals in/, { timeout: 120_000 });
  const s = await (await api.get('/api/text-search/status', { headers: authHeaders() })).json();
  expect(s.last_reindex.indexed).toBeGreaterThanOrEqual(0);
});

test('LDES sync: add a stream, see its run, forget it', async ({ page }) => {
  const me = await (await api.get('/api/auth/me', { headers: authHeaders() })).json();
  const mk = async (name: string) => {
    const r = await api.post('/api/datasets', {
      headers: authHeaders(),
      data: { name, owner_type: 'user', owner_id: String(me.id), visibility: 'public' },
    });
    expect(r.ok()).toBeTruthy();
    return (await r.json()).id as string;
  };
  const src = await mk(`e2e-ldes-src-${RUN}`);
  const dst = await mk(`e2e-ldes-mirror-${RUN}`);
  const graph = `${BACKEND}/dataset/${src}/instances`;
  await api.put(`/store?graph=${encodeURIComponent(graph)}`, {
    headers: { ...authHeaders(), 'content-type': 'text/turtle' },
    data: '<http://example.org/e2e#road1> a <http://example.org/e2e#Road> ; <http://www.w3.org/2000/01/rdf-schema#label> "Road one" .',
  });
  await api.post(`/api/datasets/${src}/graphs`, { headers: authHeaders(), data: { graph_iri: graph } });
  const pub = await api.put(`/api/datasets/${src}/ldes`, { headers: authHeaders(), data: { enabled: true } });
  expect(pub.ok()).toBeTruthy();
  const stream: string = (await pub.json()).stream;

  await page.getByRole('link', { name: 'Event stream sync' }).click();
  await expect(page).toHaveURL(/\/admin\/ldes$/);
  await page.getByRole('button', { name: 'Add a stream' }).click();
  const form = page.getByTestId('ldes-add');
  await form.getByLabel('Stream URL').fill(stream);
  await form.getByTestId('ldes-dataset').selectOption(dst);
  const target = `${BACKEND}/dataset/${dst}/mirror`;
  await form.getByRole('textbox', { name: /^Into graph/ }).fill(target);
  await form.getByRole('button', { name: 'Add and sync' }).click();

  const jobs = page.getByTestId('ldes-jobs');
  const row = jobs.locator('tr').filter({ hasText: stream });
  await expect(row.getByText('OK', { exact: true })).toBeVisible({ timeout: 60_000 });
  const listed = await (await api.get('/api/admin/ldes/syncs', { headers: authHeaders() })).json();
  const job = listed.find((j: { source_url: string }) => j.source_url === stream);
  expect(job.graph_iri).toBe(target);
  expect(job.last_report.entities_updated).toBeGreaterThan(0);

  await row.getByRole('button', { name: 'Forget' }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Forget' }).click();
  await expect(jobs.locator('tr').filter({ hasText: stream })).toHaveCount(0);
});
