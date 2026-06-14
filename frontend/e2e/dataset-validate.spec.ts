import {
  test,
  expect,
  request as pwRequest,
  type APIRequestContext,
  type Page,
} from '@playwright/test';

// E2E coverage for the SHACL dataset-validation pipeline as the user sees it:
//   - bulk import with auto-split routes shapes into a '{target}/shapes'
//     subgraph (graph_role='shapes') which is auto-registered into the SHACL
//     Studio Library and surfaces in the dataset's "Effective shapes" panel;
//   - POST /api/datasets/:id/validate resolves those shapes and renders a real
//     report (envelope { report, run_id, ran_at });
//   - a dataset with no shapes anywhere gets an actionable inline error;
//   - the /validation overview page recognises auto-registered shapes (no
//     "No shapes" pill) and can run validation.
//
// Auth/navigation pattern mirrors shacl-studio.spec.ts: the app keeps its
// access token in-memory, so we sign in through the form once per test and
// then navigate the SPA client-side (pushState + popstate) — a hard page.goto
// would drop the token.

const BACKEND = process.env.OTS_BACKEND_URL ?? 'http://localhost:7878';
const ADMIN = { username: 'e2e-admin', password: 'e2e-password-123' };

// Unique per run so re-runs against the same backend never collide.
const RUN = `${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;

// One merged file: PersonShape + instances. Expected violations: bob (missing
// name, negative age) and carol (string-typed age) — alice is clean. With
// auto_split the shapes section lands in '{target}/shapes' (role=shapes) and
// the instances in '{target}/instances'.
const MERGED_TTL = `@prefix sh:  <http://www.w3.org/ns/shacl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix ex:  <http://example.org/e2e#> .

ex:PersonShape a sh:NodeShape ;
  sh:targetClass ex:Person ;
  sh:property [ sh:path ex:age ; sh:datatype xsd:integer ; sh:minInclusive 0 ] ;
  sh:property [ sh:path ex:name ; sh:minCount 1 ] .

ex:alice a ex:Person ; ex:name "Alice" ; ex:age 33 .
ex:bob a ex:Person ; ex:age -5 .
ex:carol a ex:Person ; ex:name "Carol" ; ex:age "thirty" .
`;

// Plain instance data, no shapes anywhere — validation must fail actionably.
const PLAIN_TTL = `@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix ex:   <http://example.org/e2e#> .

ex:thing1 rdfs:label "Just a thing" .
ex:thing2 rdfs:label "Another thing" .
`;

let api: APIRequestContext;
let token = '';
let shapedId = '';
const shapedName = `e2e-validate-shaped-${RUN}`;
let plainId = '';
const plainName = `e2e-validate-plain-${RUN}`;

async function apiLogin(): Promise<void> {
  const res = await api.post('/api/auth/login', { data: ADMIN });
  expect(res.ok(), `login failed: ${res.status()}`).toBeTruthy();
  token = (await res.json()).access_token;
}

const authHeaders = () => ({ authorization: `Bearer ${token}` });

async function createDataset(name: string): Promise<string> {
  const me = await (await api.get('/api/auth/me', { headers: authHeaders() })).json();
  const res = await api.post('/api/datasets', {
    headers: authHeaders(),
    data: {
      name,
      owner_type: 'user',
      owner_id: String(me.id),
      visibility: 'private',
    },
  });
  expect(res.ok(), `dataset create failed: ${res.status()} ${await res.text()}`).toBeTruthy();
  return (await res.json()).id as string;
}

// Bulk-import one Turtle file into the dataset's own namespaced graph
// '{dataset_iri}/graphs/data'. The dataset IRI comes from the API (the server's
// BASE_URL may differ from the URL the tests reach it on).
async function bulkImport(
  datasetId: string,
  filename: string,
  turtle: string,
  opts: { autoSplit?: boolean } = {},
): Promise<void> {
  const detail = await (
    await api.get(`/api/datasets/${datasetId}`, { headers: authHeaders() })
  ).json();
  const target = `${detail.dataset_iri}/graphs/data`;
  const res = await api.post('/api/import/bulk', {
    headers: authHeaders(),
    multipart: {
      file: { name: filename, mimeType: 'text/turtle', buffer: Buffer.from(turtle) },
      meta: JSON.stringify({
        dataset_id: datasetId,
        default_target_graph: target,
        targets: { [filename]: target },
        auto_split_files: opts.autoSplit ? [filename] : [],
      }),
    },
  });
  expect(res.ok(), `bulk import failed: ${res.status()} ${await res.text()}`).toBeTruthy();
}

async function signIn(page: Page): Promise<void> {
  await page.goto('/login');
  await page.getByLabel('Username').fill(ADMIN.username);
  await page.getByLabel('Password').fill(ADMIN.password);
  await page.getByRole('button', { name: 'Login' }).click();
  await expect(page).toHaveURL(/\/$/);
}

// Client-side route change (Router listens on popstate) — keeps the in-memory
// access token alive, unlike page.goto.
async function clientNavigate(page: Page, path: string): Promise<void> {
  await page.evaluate((p) => {
    window.history.pushState({}, '', p);
    window.dispatchEvent(new PopStateEvent('popstate'));
  }, path);
}

// The SHACL Validation card on the dataset detail page.
const validationCard = (page: Page) =>
  page
    .locator('div.card')
    .filter({ has: page.getByRole('heading', { name: 'SHACL Validation' }) });

test.describe('Dataset SHACL validation', () => {
  test.beforeAll(async () => {
    api = await pwRequest.newContext({ baseURL: BACKEND });
    await apiLogin();
    shapedId = await createDataset(shapedName);
    await bulkImport(shapedId, 'people-merged.ttl', MERGED_TTL, { autoSplit: true });
    plainId = await createDataset(plainName);
    await bulkImport(plainId, 'plain-things.ttl', PLAIN_TTL);
  });

  test.afterAll(async () => {
    await api?.dispose();
  });

  test.beforeEach(async ({ page }) => {
    await signIn(page);
  });

  test('effective shapes panel lists the auto-registered imported shapes', async ({ page }) => {
    await clientNavigate(page, `/datasets/${shapedId}`);
    const card = validationCard(page);
    await expect(card).toBeVisible();
    // The auto-split shapes subgraph was auto-registered into the Studio
    // Library as "<dataset name> shapes" and bound to the dataset, so the
    // effective-shapes list must contain at least that entry.
    const effList = card.locator('.effective-shapes');
    await expect(effList).toBeVisible();
    await expect(effList.locator('.eff-item').first()).toBeVisible();
    await expect(
      effList.getByRole('link', { name: `${shapedName} shapes` }),
    ).toBeVisible();
  });

  test('run validation renders a real report with violations', async ({ page }) => {
    await clientNavigate(page, `/datasets/${shapedId}`);
    const card = validationCard(page);
    await expect(card).toBeVisible();

    // Marker to prove the flow never triggers a full page reload.
    await page.evaluate(() => {
      (window as unknown as Record<string, unknown>).__e2eNoReload = true;
    });

    await card.getByRole('button', { name: 'Run Validation' }).click();
    const dialog = page.getByRole('dialog', { name: 'SHACL Validation' });
    await expect(dialog).toBeVisible();
    // Leave the shapes-IRI override blank → the dataset's effective shapes.
    await dialog.getByRole('button', { name: 'Run Validation' }).click();

    // A real (non-conforming) report renders inside the dialog.
    const report = dialog.locator('.report');
    await expect(report).toBeVisible({ timeout: 20_000 });
    await expect(report).toContainText('Does not conform');
    const rows = report.locator('tbody tr');
    expect(await rows.count()).toBeGreaterThanOrEqual(3);
    // Focus nodes for both bad instances appear in the results table.
    await expect(report.locator('code', { hasText: 'e2e#bob' }).first()).toBeVisible();
    await expect(report.locator('code', { hasText: 'e2e#carol' }).first()).toBeVisible();

    await dialog.getByRole('button', { name: 'Close' }).click();
    await expect(dialog).not.toBeVisible();

    // The card now shows the persisted run: non-conformance + ran-at timestamp.
    const cardReport = card.locator('.report');
    await expect(cardReport).toBeVisible();
    await expect(cardReport).toContainText('Does not conform');
    await expect(cardReport.locator('.run-meta')).toBeVisible();
    await expect(cardReport.locator('.run-meta')).toHaveAttribute('title', /\d{4}/);

    // Still the same document — no reload happened.
    expect(
      await page.evaluate(() => (window as unknown as Record<string, unknown>).__e2eNoReload),
    ).toBe(true);
  });

  test('validation on a shapeless dataset surfaces the error inline', async ({ page }) => {
    await clientNavigate(page, `/datasets/${plainId}`);
    const card = validationCard(page);
    await expect(card).toBeVisible();

    await card.getByRole('button', { name: 'Run Validation' }).click();
    const dialog = page.getByRole('dialog', { name: 'SHACL Validation' });
    await expect(dialog).toBeVisible();
    await dialog.getByRole('button', { name: 'Run Validation' }).click();

    // The backend's actionable 400 message is shown inline next to the control
    // (never silence): it tells the user how to provide shapes.
    const error = dialog.locator('.validation-error');
    await expect(error).toBeVisible({ timeout: 15_000 });
    await expect(error).toContainText(/shapes/i);
    // No report rendered for the failed run.
    await expect(dialog.locator('.report')).toHaveCount(0);
  });

  test('Validation page recognises the dataset and runs it', async ({ page }) => {
    await clientNavigate(page, '/validation');

    const row = page.locator('.dataset-row', { hasText: shapedName });
    await expect(row).toBeVisible();
    // Shapes were auto-registered from the import → no "No shapes" pill, and
    // the per-row run control is enabled (title flips to the no-shapes hint
    // otherwise).
    await expect(row.getByText('No shapes', { exact: true })).toHaveCount(0);
    const runBtn = row.locator('button[title="Run validation"]');
    await expect(runBtn).toBeEnabled();

    await runBtn.click();
    // The results pane populates with the non-conforming outcome.
    const banner = page.locator('.results-banner');
    await expect(banner).toBeVisible({ timeout: 20_000 });
    await expect(banner).toContainText(/issue\(s\) found/);
    await expect(banner).toContainText(shapedName);
    await expect(page.locator('.issue-results')).toBeVisible();
  });
});
