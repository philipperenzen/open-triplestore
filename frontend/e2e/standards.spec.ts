import { test, expect, request as pwRequest, type APIRequestContext, type Page } from '@playwright/test';

// Standards-conformance smoke tests driven through the real browser.
//
// Each test opens a seeded category dataset, jumps to its API Services (saved
// queries), expands one service, runs it against the live backend, and asserts on
// a value the underlying standard must produce. This exercises the full stack —
// SvelteKit UI → SPARQL Protocol endpoint → opengraph/oxigraph engine → result
// rendering — for one standard per test.
//
// The stack (backend + frontend) and the public demo seed are provided by
// playwright.config.ts + global-setup.ts; the demo org is public, so no sign-in
// is needed. demo.spec.ts already covers GeoSPARQL (the spatial dataset) and the
// capabilities "Supported standards" service, so this file broadens coverage to
// SPARQL SELECT, SPARQL property paths, SHACL, DCAT and SKOS. The exact service
// names and the asserted values come from src/saved_queries/seed_data.rs.
//
// A saved query only reads the seeded triples, so it cannot show that a
// reasoner works. The OWL 2 RL test at the end calls the engine itself
// (POST /api/reasoning/materialize) over the seeded ontology and asks for a
// triple that only the reasoner can produce.

/** Open `dataset` → API Services, expand `service`, and click Run. */
async function runService(page: Page, dataset: string, service: string): Promise<void> {
  await page.goto('/datasets');

  await page.getByRole('link', { name: dataset }).click();
  await expect(page).toHaveURL(/\/datasets\/[^/]+$/);

  await page.getByRole('link', { name: /API Services/i }).click();
  await expect(page).toHaveURL(/\/api-services$/);

  // Service cards are collapsed by default — expand before the Run control exists.
  await expect(page.getByText(service)).toBeVisible();
  await page.getByText(service).click();
  await page.getByRole('button', { name: /^Run$/i }).first().click();
}

test('SPARQL 1.1 SELECT returns the seeded RDF statements', async ({ page }) => {
  // SELECT ?s ?p ?o over the core graph — ex:Ada rdfs:label "Ada Lovelace"@en.
  await runService(page, 'Core RDF & SPARQL', 'All statements');
  await expect(page.getByText(/Ada Lovelace/).first()).toBeVisible();
});

test('SPARQL property paths compute a transitive closure at query time', async ({ page }) => {
  // ex:ancestorOf links Alice→Bob→Carol→Dave; the property path ex:ancestorOf+
  // (plain SPARQL 1.1, no reasoner) must reach Dave from an earlier ancestor.
  await runService(page, 'Reasoning & Ontologies', 'Transitive ancestors');
  await expect(page.getByText(/Dave/).first()).toBeVisible();
});

test('SHACL surfaces the declared node shapes and their target class', async ({ page }) => {
  // ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person.
  await runService(page, 'Validation (SHACL & ShEx)', 'Declared shapes');
  await expect(page.getByText(/PersonShape/).first()).toBeVisible();
});

test('DCAT lists the datasets advertised by the bundled catalog', async ({ page }) => {
  // dcat:Catalog → dcat:dataset whose dct:title is "Cities".
  await runService(page, 'Linked Data & Catalog', 'Catalog datasets');
  await expect(page.getByText(/Cities/).first()).toBeVisible();
});

test('SKOS exposes the controlled-vocabulary concepts', async ({ page }) => {
  // skos:Concept prefLabels include the graph roles and conformance levels.
  await runService(page, 'Open Triplestore Ontology & Vocabulary', 'Vocabulary concepts');
  await expect(page.getByText(/Instances|Model|Full/).first()).toBeVisible();
});

// ── The OWL 2 RL engine ─────────────────────────────────────────────────────────

const BACKEND = process.env.OTS_BACKEND_URL ?? 'http://localhost:7878';
const ADMIN = { username: 'e2e-admin', password: 'e2e-password-123' };
const OWL_RL_GRAPH = 'https://opentriplestore.org/demo/reasoning/owl-rl';
const EX = 'https://opentriplestore.org/demo/reasoning#';

test.describe('OWL 2 RL materialisation', () => {
  let api: APIRequestContext;
  let auth: Record<string, string>;

  test.beforeAll(async () => {
    api = await pwRequest.newContext({ baseURL: BACKEND });
    const res = await api.post('/api/auth/login', { data: ADMIN });
    expect(res.ok(), `login failed: ${res.status()}`).toBeTruthy();
    auth = { authorization: `Bearer ${(await res.json()).access_token}` };
  });

  test.afterAll(async () => {
    await api.dispose();
  });

  /** ASK over /sparql as the admin, optionally with an entailment regime. */
  async function ask(query: string, entailment?: string): Promise<boolean> {
    const params: Record<string, string> = { query };
    if (entailment) params.entailment = entailment;
    const res = await api.get('/sparql', { params, headers: { ...auth, accept: 'application/sparql-results+json' } });
    expect(res.ok(), `ASK failed: ${res.status()} ${await res.text()}`).toBeTruthy();
    return (await res.json()).boolean === true;
  }

  test('derives what the seeded ontology entails but does not assert', async () => {
    // Seeded (seed_data.rs OWL_RL_TTL): ex:ancestorOf is an owl:TransitiveProperty
    // with Alice→Bob→Carol→Dave, and ex:William owl:sameAs ex:Bill, Bill→Erin.
    const aliceDave = `ASK { <${EX}Alice> <${EX}ancestorOf> <${EX}Dave> }`;
    const williamErin = `ASK { <${EX}William> <${EX}ancestorOf> <${EX}Erin> }`;
    // Neither is asserted in the graph the reasoner reads.
    expect(await ask(`ASK { GRAPH <${OWL_RL_GRAPH}> { <${EX}Alice> <${EX}ancestorOf> <${EX}Dave> } }`)).toBe(false);
    expect(await ask(`ASK { GRAPH <${OWL_RL_GRAPH}> { <${EX}William> <${EX}ancestorOf> <${EX}Erin> } }`)).toBe(false);

    const run = await api.post('/api/reasoning/materialize', {
      headers: auth,
      data: { regime: 'owl2-rl', source_graphs: [OWL_RL_GRAPH] },
    });
    expect(run.ok(), `materialize failed: ${run.status()} ${await run.text()}`).toBeTruthy();

    // prp-trp (transitivity) and eq-rep-s (sameAs substitution), read through
    // the regime's entailment graph.
    expect(await ask(aliceDave, 'owl2-rl')).toBe(true);
    expect(await ask(williamErin, 'owl2-rl')).toBe(true);
  });
});
