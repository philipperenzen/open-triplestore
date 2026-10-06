import { test, expect, request as pwRequest, type APIRequestContext, type Page } from '@playwright/test';
import { readFileSync } from 'node:fs';

// The dataset page's Exchange section and the shape library's IDS actions,
// against a real backend: publish an LDES stream, ingest CityJSON (preview,
// then import), export the dataset as an ICDD container and validate the
// export, import an IDS (a refused one shows its audit report), export it
// back (a lossless re-export), and run it against a building's IDS
// projection. Auth and navigation follow operations.spec.ts: sign in through
// the form, move around the SPA client-side.

const BACKEND = process.env.OTS_BACKEND_URL ?? 'http://localhost:7878';
const ADMIN = { username: 'e2e-admin', password: 'e2e-password-123' };
const RUN = `${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;

const IDS = `<?xml version="1.0" encoding="UTF-8"?>
<ids:ids xmlns:ids="http://standards.buildingsmart.org/IDS" xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <ids:info><ids:title>E2E wall fire ratings ${RUN}</ids:title></ids:info>
  <ids:specifications>
    <ids:specification name="External walls need a fire rating" ifcVersion="IFC4">
      <ids:applicability>
        <ids:entity><ids:name><ids:simpleValue>IFCWALL</ids:simpleValue></ids:name></ids:entity>
      </ids:applicability>
      <ids:requirements>
        <ids:property cardinality="required" dataType="IFCLABEL"><ids:propertySet><ids:simpleValue>Pset_WallCommon</ids:simpleValue></ids:propertySet><ids:baseName><ids:simpleValue>FireRating</ids:simpleValue></ids:baseName></ids:property>
      </ids:requirements>
    </ids:specification>
  </ids:specifications>
</ids:ids>`;

const CITYJSON = JSON.stringify({
  type: 'CityJSON',
  version: '2.0',
  metadata: { referenceSystem: 'https://www.opengis.net/def/crs/EPSG/0/7415' },
  transform: { scale: [1, 1, 1], translate: [84000, 447000, 0] },
  CityObjects: {
    'NL.IMBAG.Pand.e2e': {
      type: 'Building',
      attributes: { bouwjaar: 1998 },
      geometry: [{ type: 'Solid', lod: '2.2', boundaries: [[[[0, 1, 2, 3]], [[4, 7, 6, 5]], [[0, 4, 5, 1]], [[1, 5, 6, 2]], [[2, 6, 7, 3]], [[3, 7, 4, 0]]]] }],
    },
  },
  vertices: [[0, 0, 0], [1, 0, 0], [1, 1, 0], [0, 1, 0], [0, 0, 1], [1, 0, 1], [1, 1, 1], [0, 1, 1]],
});

let api: APIRequestContext;
let token = '';
let datasetId = '';
const datasetName = `e2e-exchange-${RUN}`;
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

async function openExchange(page: Page) {
  await clientNavigate(page, `/datasets/${datasetId}`);
  const section = page.getByTestId('dataset-exchange');
  await expect(section).toBeVisible({ timeout: 20_000 });
  return section;
}

test.beforeAll(async () => {
  api = await pwRequest.newContext({ baseURL: BACKEND });
  const login = await api.post('/api/auth/login', { data: ADMIN });
  expect(login.ok(), `login failed: ${login.status()}`).toBeTruthy();
  token = (await login.json()).access_token;
  const me = await (await api.get('/api/auth/me', { headers: authHeaders() })).json();
  const created = await api.post('/api/datasets', {
    headers: authHeaders(),
    data: { name: datasetName, owner_type: 'user', owner_id: String(me.id), visibility: 'private' },
  });
  expect(created.ok(), `dataset create failed: ${created.status()}`).toBeTruthy();
  datasetId = (await created.json()).id as string;
  const graph = `${BACKEND}/dataset/${datasetId}/instances`;
  const put = await api.put(`/store?graph=${encodeURIComponent(graph)}`, {
    headers: { ...authHeaders(), 'content-type': 'text/turtle' },
    data: '<http://example.org/e2e#bridge1> a <http://example.org/e2e#Bridge> ; <http://www.w3.org/2000/01/rdf-schema#label> "Bridge one" .',
  });
  expect(put.ok(), `graph load failed: ${put.status()}`).toBeTruthy();
  const reg = await api.post(`/api/datasets/${datasetId}/graphs`, { headers: authHeaders(), data: { graph_iri: graph } });
  expect(reg.ok(), `graph register failed: ${reg.status()}`).toBeTruthy();
});

test.afterAll(async () => {
  await api?.dispose();
});

test.beforeEach(async ({ page }) => {
  await signIn(page);
});

test('publishes the dataset as an event stream from its settings', async ({ page }) => {
  const section = await openExchange(page);
  const ldes = section.getByTestId('ldes-panel');
  await expect(ldes.getByText('Not published')).toBeVisible();
  await ldes.getByLabel('Publish this dataset as an event stream').check();
  await ldes.getByRole('spinbutton', { name: /^Members per page/ }).fill('2');
  await ldes.getByRole('spinbutton', { name: /^Versions per entity/ }).fill('3');
  await ldes.getByRole('button', { name: 'Save stream settings' }).click();
  await expect(ldes.getByText('Publishing', { exact: true })).toBeVisible();
  await expect(ldes.getByText(`/api/datasets/${datasetId}/ldes`)).toBeVisible();

  const cfg = await (await api.get(`/api/datasets/${datasetId}/ldes/config`, { headers: authHeaders() })).json();
  expect(cfg.enabled).toBe(true);
  expect(cfg.page_size).toBe(2);
  expect(cfg.retention).toEqual({ version_amount: 3 });
  expect(cfg.members).toBeGreaterThan(0);
});

test('ingests CityJSON: preview first, then import', async ({ page }) => {
  const section = await openExchange(page);
  const panel = section.getByTestId('cityjson-panel');
  await panel.getByTestId('cityjson-file').setInputFiles({ name: 'block.city.json', mimeType: 'application/json', buffer: Buffer.from(CITYJSON) });
  await panel.getByRole('button', { name: 'Preview' }).click();
  const result = panel.getByTestId('cityjson-result');
  await expect(result).toContainText('nothing was written');
  await expect(result.locator('pre')).toContainText('NL.IMBAG.Pand.e2e');

  await panel.getByRole('button', { name: 'Import', exact: true }).click();
  await expect(result).toContainText('Imported', { timeout: 20_000 });
  const graph = (await result.locator('code').first().textContent())!.trim();
  const graphs = await (await api.get(`/api/datasets/${datasetId}/graphs`, { headers: authHeaders() })).json();
  expect(graphs.map((g: { graph_iri: string }) => g.graph_iri)).toContain(graph);
});

test('exports an ICDD container with its validation, and validates the export', async ({ page }) => {
  const section = await openExchange(page);
  const panel = section.getByTestId('container-panel');
  // The e2e backend sets no OTS_ICDD_ONTOLOGY_DIR: the page says so up front.
  await expect(panel.getByTestId('ontology-warning')).toContainText('OTS_ICDD_ONTOLOGY_DIR');

  const [download] = await Promise.all([
    page.waitForEvent('download'),
    panel.getByTestId('container-export').click(),
  ]);
  expect(download.suggestedFilename()).toBe(`${datasetId}.icdd`);
  const result = panel.getByTestId('container-export-result');
  await expect(result).toContainText(`${datasetId}.icdd`);
  await expect(result).toContainText('ontology-resource-missing');

  const path = await download.path();
  await panel.getByTestId('container-file').setInputFiles({ name: `${datasetId}.icdd`, mimeType: 'application/zip', buffer: readFileSync(path!) });
  await panel.getByRole('button', { name: 'Validate only' }).click();
  await expect(panel.getByText('Validation report (nothing was stored)')).toBeVisible({ timeout: 20_000 });
  await expect(panel.getByTestId('container-report')).toContainText('ontology-resource-missing');
});

test('imports an IDS (audit report on refusal), exports it losslessly and checks a building', async ({ page }) => {
  await clientNavigate(page, '/shacl/shapes');
  await page.getByRole('button', { name: 'Import IDS' }).click();
  const dialog = page.getByRole('dialog', { name: 'Import an IDS' });

  // A document no model could satisfy is refused with its audit report.
  await dialog.getByTestId('ids-file').setInputFiles({ name: 'bad.ids', mimeType: 'application/xml', buffer: Buffer.from(IDS.replace('IFCWALL', 'IFCWALLL')) });
  await dialog.getByRole('button', { name: 'Check' }).click();
  const audit = dialog.getByTestId('ids-audit-report');
  await expect(audit).toContainText('Fails the IDS audit rules');
  await expect(audit.locator('li').filter({ hasText: 'IFCWALLL' })).toHaveCount(1);

  // A good one is checked, then created.
  await dialog.getByTestId('ids-file').setInputFiles({ name: 'fire.ids', mimeType: 'application/xml', buffer: Buffer.from(IDS) });
  await dialog.getByRole('button', { name: 'Check' }).click();
  await expect(dialog.getByTestId('ids-checked')).toContainText('1 specification(s)');
  await dialog.getByRole('button', { name: 'Create shape graph' }).click();
  await expect(page).toHaveURL(/\/shacl\/shapes\/[^/]+$/);
  const shapeGraphId = page.url().split('/').pop()!;

  // Exported back: the recorded source is written back unchanged.
  await clientNavigate(page, '/shacl/shapes');
  const row = page.locator('li').filter({ hasText: `E2E wall fire ratings ${RUN}` }).first();
  await row.getByRole('button', { name: 'Export to IDS' }).click();
  const exportDialog = page.getByRole('dialog', { name: /Export .* to IDS/ });
  await expect(exportDialog.getByTestId('ids-lossless')).toBeVisible();
  const [download] = await Promise.all([
    page.waitForEvent('download'),
    exportDialog.getByRole('button', { name: 'Download .ids' }).click(),
  ]);
  expect(readFileSync((await download.path())!, 'utf8')).toContain('Pset_WallCommon');
  await exportDialog.getByRole('button', { name: 'Close' }).last().click();

  // The building: an IDS projection graph without any wall, so the required
  // specification has no applicable entity.
  const building = `${BACKEND}/dataset/${datasetId}/building`;
  const put = await api.put(`/store?graph=${encodeURIComponent(`${building}/ids`)}`, {
    headers: { ...authHeaders(), 'content-type': 'text/turtle' },
    data: `<${building}/model> a <https://opentriplestore.org/ns/ifc-ids/Model> .`,
  });
  expect(put.ok()).toBeTruthy();
  await api.post(`/api/datasets/${datasetId}/graphs`, { headers: authHeaders(), data: { graph_iri: `${building}/ids` } });

  const section = await openExchange(page);
  const check = section.getByTestId('ids-check-panel');
  await check.getByTestId('ids-shape-graph').selectOption(shapeGraphId);
  await expect(check.getByTestId('ids-graph')).toHaveValue(`${building}/ids`);
  await check.getByRole('button', { name: 'Run check' }).click();
  const results = check.getByTestId('ids-results');
  await expect(results).toBeVisible({ timeout: 30_000 });
  await expect(results).toContainText('External walls need a fire rating');
  await expect(check.getByRole('link', { name: 'Open the pipeline' })).toBeVisible();
});
