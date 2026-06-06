import { test, expect, type Page } from '@playwright/test';

// Extended standards-conformance smoke tests (browser-driven, full stack).
//
// Companion to standards.spec.ts. That file covers SPARQL SELECT, RDFS/OWL
// transitive reasoning, SHACL Core node shapes, DCAT catalog and SKOS; demo.spec.ts
// covers GeoSPARQL and capabilities. This file broadens coverage to the remaining
// standards surfaced as seeded saved queries: RDF-star / SPARQL 1.2, the RDFS
// subclass closure, SHACL Core property constraints, SHACL Advanced (sh:sparql)
// constraints, SWRL rules, LDP containers and DCAT distributions.
//
// Each asserted token is a LITERAL or local name guaranteed by the demo seed
// (src/saved_queries/seed_data.rs), so assertions are stable across renderings.

/** Open `dataset` → API Services, expand `service`, and click Run. */
async function runService(page: Page, dataset: string, service: string): Promise<void> {
  await page.goto('/datasets');
  await page.getByRole('link', { name: dataset }).click();
  await expect(page).toHaveURL(/\/datasets\/[^/]+$/);
  await page.getByRole('link', { name: /API Services/i }).click();
  await expect(page).toHaveURL(/\/api-services$/);
  await expect(page.getByText(service)).toBeVisible();
  await page.getByText(service).click();
  await page.getByRole('button', { name: /^Run$/i }).first().click();
}

test('SPARQL 1.2 / RDF-star queries metadata asserted about a quoted triple', async ({ page }) => {
  // << Ada knows Charles >> ex:confidence "0.9" — the quoted-triple annotation.
  await runService(page, 'Core RDF & SPARQL', 'Statements about statements');
  await expect(page.getByText(/0\.9/).first()).toBeVisible();
});

test('RDFS subclass closure reaches a transitive superclass', async ({ page }) => {
  // Dog ⊑ Mammal ⊑ Animal — rdfs:subClassOf+ must surface Mammal.
  await runService(page, 'Reasoning & Ontologies', 'Class hierarchy');
  await expect(page.getByText(/Mammal/).first()).toBeVisible();
});

test('SHACL Core property constraints expose path datatypes', async ({ page }) => {
  // ex:AgeConstraint sh:datatype xsd:integer.
  await runService(page, 'Validation (SHACL & ShEx)', 'Property constraints');
  await expect(page.getByText(/integer/).first()).toBeVisible();
});

test('SHACL Advanced (sh:sparql) constraint carries its message', async ({ page }) => {
  // ex:AdultConstraint a sh:SPARQLConstraint ; sh:message "Person must be at least 18.".
  await runService(page, 'Validation (SHACL & ShEx)', 'SPARQL-based constraints');
  await expect(page.getByText(/Person must be at least 18/).first()).toBeVisible();
});

test('SWRL exposes the declared rule implication', async ({ page }) => {
  // ex:GrandparentRule a swrl:Imp — hasParent ∘ hasParent ⇒ hasGrandparent.
  await runService(page, 'Rules (SWRL)', 'SWRL rules');
  await expect(page.getByText(/GrandparentRule/).first()).toBeVisible();
});

test('LDP lists the members of a basic container', async ({ page }) => {
  // ex:notes a ldp:BasicContainer ; ldp:contains ex:note-1, ex:note-2.
  await runService(page, 'Linked Data & Catalog', 'LDP container members');
  await expect(page.getByText(/note-/).first()).toBeVisible();
});

test('DCAT distributions advertise their media type', async ({ page }) => {
  // ex:cities-ttl dcat:mediaType "text/turtle".
  await runService(page, 'Linked Data & Catalog', 'Dataset distributions');
  await expect(page.getByText(/text\/turtle/).first()).toBeVisible();
});
