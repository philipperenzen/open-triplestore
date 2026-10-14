import { test, expect, request as pwRequest, type APIRequestContext, type Page } from '@playwright/test';

// Extended standards-conformance smoke tests (browser-driven, full stack).
//
// Companion to standards.spec.ts. That file covers SPARQL SELECT, RDFS/OWL
// transitive reasoning, SHACL Core node shapes, DCAT catalog and SKOS; demo.spec.ts
// covers GeoSPARQL and capabilities. This file broadens coverage to the remaining
// standards surfaced as seeded saved queries: RDF-star / SPARQL 1.2, the RDFS
// subclass closure, SHACL Core property constraints, SHACL Advanced (sh:sparql)
// constraints, the stored SWRL rule, the seeded LDP container triples and DCAT
// distributions.
//
// Each asserted token is a LITERAL or local name guaranteed by the demo seed
// (src/saved_queries/seed_data.rs), so assertions are stable across renderings.
//
// Reading seeded triples does not exercise an engine, so the SWRL and LDP
// sections at the end call them: POST /api/swrl/execute must derive a triple,
// and the /ldp HTTP layer must create, list and delete container members.

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

test('the seeded SWRL rule is stored as RDF (swrl:Imp)', async ({ page }) => {
  // ex:GrandparentRule a swrl:Imp — hasParent ∘ hasParent ⇒ hasGrandparent.
  await runService(page, 'Rules (SWRL)', 'SWRL rules');
  await expect(page.getByText(/GrandparentRule/).first()).toBeVisible();
});

test('the seeded LDP container triples are queryable', async ({ page }) => {
  // ex:notes a ldp:BasicContainer ; ldp:contains ex:note-1, ex:note-2 — data
  // only; the /ldp HTTP layer is exercised below.
  await runService(page, 'Linked Data & Catalog', 'LDP container members');
  await expect(page.getByText(/note-/).first()).toBeVisible();
});

test('DCAT distributions advertise their media type', async ({ page }) => {
  // ex:cities-ttl dcat:mediaType "text/turtle".
  await runService(page, 'Linked Data & Catalog', 'Dataset distributions');
  await expect(page.getByText(/text\/turtle/).first()).toBeVisible();
});

// ── The SWRL engine and the LDP HTTP layer ──────────────────────────────────────

const BACKEND = process.env.OTS_BACKEND_URL ?? 'http://localhost:7878';
const ADMIN = { username: 'e2e-admin', password: 'e2e-password-123' };
const RUN = `${Date.now().toString(36)}${Math.floor(Math.random() * 1e6).toString(36)}`;

test.describe('engines', () => {
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

  async function update(sparql: string): Promise<void> {
    const res = await api.post('/sparql', {
      headers: { ...auth, 'content-type': 'application/sparql-update' },
      data: sparql,
    });
    expect(res.ok(), `update failed: ${res.status()} ${await res.text()}`).toBeTruthy();
  }

  async function ask(query: string): Promise<boolean> {
    const res = await api.get('/sparql', { params: { query }, headers: { ...auth, accept: 'application/sparql-results+json' } });
    expect(res.ok(), `ASK failed: ${res.status()} ${await res.text()}`).toBeTruthy();
    return (await res.json()).boolean === true;
  }

  test('SWRL execution derives the rule head from its body', async () => {
    // The engine evaluates rule bodies over the default graph, so the facts go
    // there (the seeded ones sit in a named graph); the derived triples go to a
    // fresh target graph named in FROM NAMED for the admin's ASK.
    const ex = `https://example.org/e2e/swrl/${RUN}#`;
    const target = `urn:e2e:swrl:${RUN}`;
    const derived = `ASK FROM NAMED <${target}> { GRAPH <${target}> { <${ex}tom> <${ex}hasGrandparent> <${ex}sophie> } }`;
    await update(`INSERT DATA { <${ex}tom> <${ex}hasParent> <${ex}mary> . <${ex}mary> <${ex}hasParent> <${ex}sophie> . }`);
    try {
      expect(await ask(derived)).toBe(false);
      const res = await api.post('/api/swrl/execute', {
        headers: auth,
        data: {
          rules: `${ex}hasParent(?x, ?y) ^ ${ex}hasParent(?y, ?z) -> ${ex}hasGrandparent(?x, ?z)`,
          format: 'text',
          target_graph: target,
        },
      });
      const body = await res.json();
      expect(res.ok(), `execute failed: ${res.status()} ${JSON.stringify(body)}`).toBeTruthy();
      expect(body.triples_inferred).toBeGreaterThanOrEqual(1);
      expect(body.rule_results.every((r: { success: boolean }) => r.success)).toBe(true);
      expect(await ask(derived)).toBe(true);
    } finally {
      await update(`DELETE DATA { <${ex}tom> <${ex}hasParent> <${ex}mary> . <${ex}mary> <${ex}hasParent> <${ex}sophie> . } ; DROP SILENT GRAPH <${target}>`);
    }
  });

  test('LDP creates, lists and deletes a basic container member', async () => {
    const container = `/ldp/e2e-notes-${RUN}`;
    const created = await api.post(container, {
      headers: { ...auth, 'content-type': 'text/turtle', slug: 'note-1' },
      data: '<> <http://purl.org/dc/terms/title> "First note" .',
    });
    expect(created.status(), await created.text()).toBe(201);
    const location = created.headers()['location'];
    expect(location).toBeTruthy();
    const memberPath = new URL(location).pathname;

    // The container (auto-created as a Basic Container) lists the new member.
    const listed = await api.get(container, { headers: { ...auth, accept: 'text/turtle' } });
    expect(listed.ok()).toBeTruthy();
    expect(listed.headers()['link']).toContain('ldp#BasicContainer');
    const listing = await listed.text();
    expect(listing).toContain('http://www.w3.org/ns/ldp#contains');
    expect(listing).toContain(location);

    // The member serves what was posted.
    const member = await api.get(memberPath, { headers: { ...auth, accept: 'text/turtle' } });
    expect(member.ok()).toBeTruthy();
    expect(await member.text()).toContain('First note');

    // Deleting it drops it from the container.
    const deleted = await api.delete(memberPath, { headers: auth });
    expect(deleted.ok(), `delete failed: ${deleted.status()}`).toBeTruthy();
    const after = await api.get(container, { headers: { ...auth, accept: 'text/turtle' } });
    expect(await after.text()).not.toContain(location);
    await api.delete(container, { headers: auth });
  });
});
