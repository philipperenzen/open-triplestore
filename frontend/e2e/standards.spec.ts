import { test, expect, type Page } from '@playwright/test';

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
// SPARQL SELECT, RDFS/OWL reasoning, SHACL, DCAT and SKOS. The exact service
// names and the asserted values come from src/saved_queries/seed_data.rs.

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

test('RDFS/OWL reasoning expands a transitive-property closure', async ({ page }) => {
  // ex:ancestorOf is an owl:TransitiveProperty with Alice→Bob→Carol→Dave, so the
  // property-path closure (ex:ancestorOf+) must reach Dave from an earlier ancestor.
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
