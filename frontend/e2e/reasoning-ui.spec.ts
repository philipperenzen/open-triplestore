import { test, expect, request as pwRequest, type APIRequestContext, type Page } from '@playwright/test';

// The UI for what used to be API-only (card U1): a dataset's reasoning page
// (entailment, stored SWRL rules, SWRL runs with a regime), ShEx validation,
// RDF Patch with the patch log, and version diff and clean-up.
//
// Every flow is driven through the browser and checked against the engines
// over the API afterwards: a materialisation must have derived the triple, a
// stored rule must have fired (and its conclusion gone with it), ShEx must
// have judged the planted bad node, a patch must have changed the data and
// landed in the log, and clean-up must really have deleted versions.
//
// Sign-in and navigation follow dataset-validate.spec.ts: the access token
// lives in memory, so the SPA is navigated client-side after one form login.

const BACKEND = process.env.OTS_BACKEND_URL ?? 'http://localhost:7878';
const ADMIN = { username: 'e2e-admin', password: 'e2e-password-123' };
const RUN = `${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;
const EX = `https://example.org/e2e-u1/${RUN}/`;

const DATA_TTL = `@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix ex: <${EX}> .

ex:Dog rdfs:subClassOf ex:Animal .
ex:rex a ex:Dog ; ex:name "Rex" ; ex:age 7 .
ex:fido a ex:Dog ; ex:age 2 .
`;

let api: APIRequestContext;
let token = '';
let datasetId = '';
let dataGraph = '';

const auth = () => ({ authorization: `Bearer ${token}` });

async function ask(query: string): Promise<boolean> {
  const res = await api.get('/sparql', { params: { query }, headers: { ...auth(), accept: 'application/sparql-results+json' } });
  expect(res.ok(), `ASK failed: ${res.status()} ${await res.text()}`).toBeTruthy();
  return (await res.json()).boolean === true;
}

/** ASK with the dataset's inference graph (regime and stored rules) joined in. */
async function askInferred(pattern: string): Promise<boolean> {
  const res = await api.get('/sparql', {
    params: { query: `ASK { ${pattern} }`, entailment_dataset: datasetId },
    headers: { ...auth(), accept: 'application/sparql-results+json' },
  });
  expect(res.ok(), `ASK failed: ${res.status()} ${await res.text()}`).toBeTruthy();
  return (await res.json()).boolean === true;
}

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

/** Pick an option in the app's <Select> listbox. */
async function choose(page: Page, label: string, option: string): Promise<void> {
  await page.getByRole('button', { name: label, exact: true }).click();
  await page.getByRole('option', { name: option, exact: true }).click();
}

test.describe('API-only features in the UI', () => {
  test.beforeAll(async () => {
    api = await pwRequest.newContext({ baseURL: BACKEND });
    const login = await api.post('/api/auth/login', { data: ADMIN });
    expect(login.ok(), `login failed: ${login.status()}`).toBeTruthy();
    token = (await login.json()).access_token;
    const me = await (await api.get('/api/auth/me', { headers: auth() })).json();
    const created = await api.post('/api/datasets', {
      headers: auth(),
      data: { name: `e2e-u1-${RUN}`, owner_type: 'user', owner_id: String(me.id), visibility: 'private' },
    });
    expect(created.ok(), await created.text()).toBeTruthy();
    datasetId = (await created.json()).id;
    const detail = await (await api.get(`/api/datasets/${datasetId}`, { headers: auth() })).json();
    dataGraph = `${detail.dataset_iri}/graphs/data`;
    const imported = await api.post('/api/import/bulk', {
      headers: auth(),
      multipart: {
        file: { name: 'dogs.ttl', mimeType: 'text/turtle', buffer: Buffer.from(DATA_TTL) },
        meta: JSON.stringify({ dataset_id: datasetId, default_target_graph: dataGraph, targets: { 'dogs.ttl': dataGraph } }),
      },
    });
    expect(imported.ok(), await imported.text()).toBeTruthy();
  });

  test.afterAll(async () => {
    if (datasetId) await api.delete(`/api/datasets/${datasetId}`, { headers: auth() });
    await api?.dispose();
  });

  test.beforeEach(async ({ page }) => {
    await signIn(page);
  });

  test('entailment: select RDFS, run it, and the dataset graph holds the inference', async ({ page }) => {
    await clientNavigate(page, `/datasets/${datasetId}`);
    await page.getByRole('link', { name: /^Reasoning & rules/ }).click();
    await expect(page).toHaveURL(new RegExp(`/datasets/${datasetId}/reasoning$`));
    const card = page.locator('#entailment-settings');
    await expect(card.getByText('None configured', { exact: true })).toBeVisible();
    await choose(page, 'Regime', 'RDFS');
    await card.getByRole('button', { name: 'Save and run' }).click();
    await expect(card.getByTestId('entailment-status')).toHaveText(/OK/, { timeout: 20_000 });
    expect(await askInferred(`<${EX}rex> a <${EX}Animal>`)).toBe(true);
  });

  test('stored SWRL rules: add, see the run, remove after the app dialog', async ({ page }) => {
    await clientNavigate(page, `/datasets/${datasetId}/reasoning`);
    const rules = page.locator('#rules');
    await rules.getByRole('button', { name: 'Add rules' }).click();
    await rules.getByLabel('Name').fill('Seniors');
    await rules.getByLabel('Prefixes').fill(`ex: ${EX}`);
    await rules.getByLabel('Rules').fill('ex:Dog(?d) ^ ex:age(?d, ?a) ^ swrlb:greaterThan(?a, 5) -> ex:Senior(?d)');
    await rules.getByRole('button', { name: 'Store and run' }).click();
    await expect(rules.getByTestId('rule-set')).toContainText('Seniors', { timeout: 20_000 });
    await expect(rules.getByTestId('rules-report')).toBeVisible();
    // The rule ran with the dataset's regime (RDFS, previous test) into its inference graph.
    const fired = `<${EX}rex> a <${EX}Senior>`;
    await expect.poll(() => askInferred(fired), { timeout: 20_000 }).toBe(true);
    expect(await askInferred(`<${EX}fido> a <${EX}Senior>`)).toBe(false);

    // A refused rule names the built-in and writes nothing.
    await rules.getByRole('button', { name: 'Add rules' }).click();
    await rules.getByLabel('Prefixes').fill(`ex: ${EX}`);
    await rules.getByLabel('Rules').fill('ex:Dog(?d) ^ swrlb:noSuchThing(?d) -> ex:Odd(?d)');
    await rules.getByRole('button', { name: 'Store and run' }).click();
    await expect(rules.getByRole('alert')).toContainText('noSuchThing');
    await rules.getByRole('button', { name: 'Cancel' }).click();
    await expect(rules.getByTestId('rule-set')).toHaveCount(1);

    // Remove: the app's own dialog, then the conclusion goes too.
    await rules.getByRole('button', { name: 'Remove Seniors' }).click();
    const dialog = page.getByRole('dialog');
    await expect(dialog).toContainText('Remove these rules?');
    await dialog.getByRole('button', { name: 'Delete' }).click();
    await expect(rules.getByTestId('rule-set')).toHaveCount(0, { timeout: 20_000 });
    await expect.poll(() => askInferred(fired), { timeout: 20_000 }).toBe(false);
  });

  test('a one-off SWRL run with a regime reports its rounds', async ({ page }) => {
    await clientNavigate(page, `/datasets/${datasetId}/reasoning`);
    const run = page.locator('#swrl-run');
    await run.getByLabel('Prefixes').fill(`ex: ${EX}`);
    await run.getByLabel('Rules').fill('ex:Animal(?x) -> ex:Pet(?x)');
    await run.getByRole('button', { name: 'Run jointly with', exact: true }).click();
    await page.getByRole('option', { name: 'OWL 2 RL', exact: true }).click();
    await run.getByLabel('Target graph').fill(`urn:e2e:u1:swrl:${RUN}`);
    await run.getByRole('button', { name: 'Run rules' }).click();
    const report = run.getByTestId('swrl-report');
    await expect(report).toBeVisible({ timeout: 20_000 });
    await expect(run.getByTestId('swrl-rounds')).toHaveText(/\d+/);
    // OWL 2 RL makes rex an Animal; the rule makes it a Pet, in the same fixed point.
    expect(await ask(`ASK FROM NAMED <urn:e2e:u1:swrl:${RUN}> { GRAPH <urn:e2e:u1:swrl:${RUN}> { <${EX}rex> a <${EX}Pet> } }`)).toBe(true);
    await api.post('/sparql', { headers: { ...auth(), 'content-type': 'application/sparql-update' }, data: `DROP SILENT GRAPH <urn:e2e:u1:swrl:${RUN}>` });
  });

  test('ShEx validation judges the planted node', async ({ page }) => {
    await clientNavigate(page, `/datasets/${datasetId}`);
    await page.getByRole('link', { name: /^ShEx validation Validate/ }).click();
    await expect(page).toHaveURL(new RegExp(`/datasets/${datasetId}/shex$`));
    await page.getByLabel('Schema', { exact: true }).fill(`PREFIX ex: <${EX}>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>
ex:DogShape { ex:name xsd:string ; ex:age xsd:integer }`);
    await page.getByRole('button', { name: 'Node and shape' }).click();
    await page.getByLabel('Node 1').fill(`${EX}rex`);
    await page.getByLabel('Shape 1').fill(`${EX}DogShape`);
    await page.getByRole('button', { name: 'Add a pair' }).click();
    await page.getByLabel('Node 2').fill(`${EX}fido`);
    await page.getByLabel('Shape 2').fill(`${EX}DogShape`);
    await page.getByRole('button', { name: 'Validate' }).click();
    const report = page.getByTestId('shex-report');
    await expect(report).toContainText('1 of 2 do not conform.', { timeout: 20_000 });
    await page.getByRole('button', { name: 'Not conforming' }).click();
    await expect(page.getByTestId('shex-row')).toHaveCount(1);
    await expect(page.getByTestId('shex-row')).toContainText('fido');
  });

  test('RDF Patch: apply and record, then read it back from the log', async ({ page }) => {
    await clientNavigate(page, `/datasets/${datasetId}`);
    await page.getByRole('link', { name: /^RDF Patch Apply/ }).click();
    await expect(page).toHaveURL(new RegExp(`/datasets/${datasetId}/patch$`));
    await page.getByLabel('Patch', { exact: true }).fill(`TX .\nA <${EX}rex> <${EX}nickname> "Rexy" <${dataGraph}> .\nTC .`);
    await page.getByRole('button', { name: 'Apply patch' }).click();
    await expect(page.getByTestId('patch-result')).toContainText('Applied: 1 added, 0 removed', { timeout: 20_000 });
    expect(await ask(`ASK { GRAPH <${dataGraph}> { <${EX}rex> <${EX}nickname> "Rexy" } }`)).toBe(true);
    const entry = page.getByTestId('log-entry').last();
    await expect(entry).toContainText('Patch');
    await entry.getByRole('button', { name: /Show patch/ }).click();
    await expect(page.getByTestId('log-patch-text')).toContainText('Rexy');
  });

  test('versions: compare with the live graphs, then clean up behind the app dialog', async ({ page }) => {
    for (const v of ['1.0.0', '1.1.0', '1.2.0']) {
      const res = await api.post(`/api/datasets/${datasetId}/versions`, { headers: auth(), data: { version: v } });
      expect(res.ok(), `version ${v}: ${res.status()} ${await res.text()}`).toBeTruthy();
    }
    await api.post('/sparql', {
      headers: { ...auth(), 'content-type': 'application/sparql-update' },
      data: `INSERT DATA { GRAPH <${dataGraph}> { <${EX}bello> a <${EX}Dog> } }`,
    });
    await clientNavigate(page, `/datasets/${datasetId}`);
    const versions = page.locator('#dataset-versions');
    await versions.getByRole('link', { name: 'Diff' }).first().click();
    await expect(page).toHaveURL(new RegExp(`/datasets/${datasetId}/versions/diff\\?from=`));
    const diff = page.getByTestId('version-diff');
    await expect(diff).toContainText('+1', { timeout: 20_000 });
    await diff.getByRole('button', { name: 'Show the changes' }).click();
    await expect(page.getByTestId('diff-row').filter({ hasText: 'bello' })).toBeVisible();

    await clientNavigate(page, `/datasets/${datasetId}`);
    await versions.getByRole('button', { name: 'Clean up' }).click();
    await versions.getByLabel('Unpublished versions to keep').fill('1');
    // Every unpublished version but the newest goes: the three cut here less
    // one, plus the patch log's version 0 (the patch test), which is flagged.
    await expect(versions.getByTestId('version-gc')).toContainText('3 versions will be deleted');
    await expect(versions.getByTestId('version-gc')).toContainText("version 0 of the dataset's RDF Patch log");
    await versions.getByTestId('version-gc').getByRole('button', { name: 'Clean up' }).click();
    const dialog = page.getByRole('dialog');
    await expect(dialog).toContainText('Delete old versions?');
    await dialog.getByRole('button', { name: 'Delete versions' }).click();
    await expect.poll(async () => {
      const list = await (await api.get(`/api/datasets/${datasetId}/versions`, { headers: auth() })).json();
      return list.length;
    }, { timeout: 20_000 }).toBe(1);
  });
});
