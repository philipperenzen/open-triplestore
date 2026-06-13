import {
  test,
  expect,
  request as pwRequest,
  type APIRequestContext,
  type Page,
} from '@playwright/test';

// Drives the real DataImport wizard with TWO files — instances FIRST, shapes
// SECOND — to cover the regression where the post-import "SHACL shapes
// detected" prompt only fired when shapes were the *first* imported file/graph.
// The prompt must aggregate the detect-shapes probe across every imported
// graph. Accepting the prompt links the shapes to a dataset, and the imported
// shapes graph must show up registered in the SHACL Studio Library
// (shape_graphs, source='imported').
//
// Auth/navigation pattern mirrors import.spec.ts / shacl-studio.spec.ts: the
// access token is in-memory, so we sign in via the form and move around the
// SPA client-side only.

const BACKEND = process.env.OTS_BACKEND_URL ?? 'http://localhost:7878';
const ADMIN = { username: 'e2e-admin', password: 'e2e-password-123' };

const RUN = `${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;

const INSTANCES_TTL = `@prefix ex: <http://example.org/e2e#> .

ex:alice a ex:Person ; ex:name "Alice" ; ex:age 33 .
ex:bob a ex:Person ; ex:age -5 .
ex:carol a ex:Person ; ex:name "Carol" ; ex:age "thirty" .
`;

const SHAPES_TTL = `@prefix sh:  <http://www.w3.org/ns/shacl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix ex:  <http://example.org/e2e#> .

ex:PersonShape a sh:NodeShape ;
  sh:targetClass ex:Person ;
  sh:property [ sh:path ex:age ; sh:datatype xsd:integer ; sh:minInclusive 0 ] ;
  sh:property [ sh:path ex:name ; sh:minCount 1 ] .
`;

let api: APIRequestContext;
let token = '';
let datasetId = '';
const datasetName = `e2e-import-shapes-${RUN}`;

const authHeaders = () => ({ authorization: `Bearer ${token}` });

async function signIn(page: Page): Promise<void> {
  await page.goto('/login');
  await page.getByLabel('Username').fill(ADMIN.username);
  await page.getByLabel('Password').fill(ADMIN.password);
  await page.getByRole('button', { name: 'Login' }).click();
  await expect(page).toHaveURL(/\/$/);
}

test.describe('Import wizard SHACL shapes detection', () => {
  test.beforeAll(async () => {
    api = await pwRequest.newContext({ baseURL: BACKEND });
    const login = await api.post('/api/auth/login', { data: ADMIN });
    expect(login.ok(), `login failed: ${login.status()}`).toBeTruthy();
    token = (await login.json()).access_token;
    const me = await (await api.get('/api/auth/me', { headers: authHeaders() })).json();
    const created = await api.post('/api/datasets', {
      headers: authHeaders(),
      data: {
        name: datasetName,
        owner_type: 'user',
        owner_id: String(me.id),
        visibility: 'private',
      },
    });
    expect(created.ok(), `dataset create failed: ${created.status()}`).toBeTruthy();
    datasetId = (await created.json()).id as string;
  });

  test.afterAll(async () => {
    await api?.dispose();
  });

  test('detects shapes uploaded as the second file, links them, and registers them in the Library', async ({
    page,
  }) => {
    await signIn(page);

    // Client-side nav into the wizard keeps the in-memory token alive.
    await page.getByRole('link', { name: 'Import data' }).click();
    await expect(page).toHaveURL(/\/import$/);

    // Step 1 — upload instances FIRST, shapes SECOND (two change events so the
    // file order is deterministic). The shapes file being last is the point of
    // this regression test.
    const fileInput = page.locator('input[type="file"][accept]').first();
    await fileInput.setInputFiles({
      name: 'instances.ttl',
      mimeType: 'text/turtle',
      buffer: Buffer.from(INSTANCES_TTL),
    });
    await expect(page.getByText('instances.ttl')).toBeVisible();
    await fileInput.setInputFiles({
      name: 'shapes.ttl',
      mimeType: 'text/turtle',
      buffer: Buffer.from(SHAPES_TTL),
    });
    await expect(page.getByText('shapes.ttl')).toBeVisible();

    const next = page.getByRole('button', { name: 'Next' });
    await expect(next).toBeEnabled();
    await next.click();

    // Step 2 — personal account, pick the dataset created via the API.
    await page.getByRole('button', { name: /Personal account/i }).click();
    await page
      .getByRole('button', { name: new RegExp(datasetName) })
      .first()
      .click();
    await page.getByRole('button', { name: 'Next' }).click();

    // Step 3 — run the import and wait for the success state.
    const importBtn = page.getByRole('button', { name: 'Import Now' });
    await expect(importBtn).toBeEnabled();
    await importBtn.click();
    await expect(page.getByText('Import completed successfully!')).toBeVisible({
      timeout: 20_000,
    });

    // The shapes-detected prompt card MUST appear even though shapes.ttl was
    // the second file (probe aggregates over every imported graph).
    await expect(
      page.getByRole('heading', { name: 'SHACL shapes detected' }),
    ).toBeVisible({ timeout: 15_000 });

    // Accept the link action: pick the target dataset in the prompt's
    // listbox, then link. (The dataset may already be marked as having shapes
    // — the imported shapes-role graph was auto-registered — so match by name
    // prefix, not exact label.)
    await page
      .getByRole('button', { name: /Select dataset to link/ })
      .click();
    await page
      .getByRole('option', { name: new RegExp(datasetName) })
      .first()
      .click();
    await page.getByRole('button', { name: 'Link shapes' }).click();

    // Success state with a pointer into the SHACL Studio.
    await expect(page.getByText('Shapes graph linked successfully.')).toBeVisible({
      timeout: 15_000,
    });
    const studioLink = page.getByRole('link', { name: 'Open in SHACL Studio' });
    await expect(studioLink).toBeVisible();

    // API check: the imported shapes graph was auto-registered in the Library
    // (shape_graphs, source='imported') under "<dataset name> shapes".
    const sgRes = await api.get('/api/shacl/shape-graphs', { headers: authHeaders() });
    expect(sgRes.ok()).toBeTruthy();
    const shapeGraphs = (await sgRes.json()) as Array<{
      name: string;
      source: string;
      graph_iri: string;
    }>;
    const registered = shapeGraphs.find((s) => s.name === `${datasetName} shapes`);
    expect(registered, 'imported shapes graph registered in the Library').toBeTruthy();
    expect(registered!.source).toBe('imported');
    expect(registered!.graph_iri).toContain(`/${datasetId}/`);

    // UI check: follow the success link (client-side) into the Library and
    // find the registered shape graph's card.
    await studioLink.click();
    await expect(page).toHaveURL(/\/shacl\/shapes$/);
    const card = page.locator('.set-card', { hasText: `${datasetName} shapes` });
    await expect(card).toBeVisible();
    await expect(card.getByText('imported', { exact: true })).toBeVisible();
  });
});
