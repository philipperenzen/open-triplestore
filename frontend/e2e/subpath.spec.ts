import { test, expect, type Page, type Request } from '@playwright/test';

// Sub-path deploy smoke test (run with `npm run e2e:subpath`, see
// playwright.subpath.config.ts): the UI is built with OTS_BASE_PATH=/ots/ and
// served below /ots/. A deep link must render its page, every request the page
// makes to its own origin must stay below /ots/, and links must keep the base.
//
// Before the router was base-aware, /ots/docs/… matched no route and showed an
// empty shell, and config.json, the API and the links all went to the root.

const BASE = '/ots';

const DOCS = [
  { slug: 'smoke', title: 'Smoke doc', category: 'Guides', sort_order: 0 },
  { slug: 'shacl', title: 'SHACL', category: 'Guides', sort_order: 1 },
];

function docBody(slug: string): string {
  if (slug === 'smoke') {
    return [
      '# Smoke doc',
      '',
      'Browse [the datasets](/datasets) or read about [SHACL](shacl.md).',
    ].join('\n');
  }
  return `# ${slug.toUpperCase()} page\n\nLoaded below the base.`;
}

/**
 * Answer the app's backend calls (only below the base: a call that escaped it
 * is not answered here and is reported by `ownRequests` instead) and keep the
 * page off the network for everything on another origin.
 */
async function stubBackend(page: Page): Promise<void> {
  // Web fonts, basemap tiles …: not this test's business, and no network needed.
  await page.route((url) => url.hostname !== '127.0.0.1', (route) => route.abort());
  await page.route(new RegExp(`^https?://[^/]+${BASE}/(api|health|sparql|store|registry)(/|\\?|$)`), async (route) => {
    const req = route.request();
    // Page loads (a hard reload of /ots/sparql) are the SPA's, not the API's.
    if (req.resourceType() === 'document') return route.fallback();
    const path = new URL(req.url()).pathname.slice(BASE.length);
    const json = (status: number, body: unknown) =>
      route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
    if (path === '/health') return json(200, { status: 'ok' });
    if (path === '/api/auth/me' || path === '/api/auth/refresh') return json(401, { error: 'not signed in' });
    if (path === '/api/docs') return json(200, DOCS);
    const doc = path.match(/^\/api\/docs\/([^/]+)$/);
    if (doc) {
      const slug = decodeURIComponent(doc[1]);
      const meta = DOCS.find((d) => d.slug === slug) ?? { slug, title: slug, category: 'Guides', sort_order: 9 };
      return json(200, { ...meta, body_md: docBody(slug) });
    }
    return json(404, { error: `not stubbed: ${path}` });
  });
}

/** Every request the page makes to its own origin. */
function ownRequests(page: Page, origin: string): Request[] {
  const seen: Request[] = [];
  page.on('request', (r) => {
    if (new URL(r.url()).origin === origin) seen.push(r);
  });
  return seen;
}

function escapedTheBase(reqs: Request[]): string[] {
  return reqs
    .map((r) => new URL(r.url()).pathname)
    .filter((p) => p !== BASE && !p.startsWith(`${BASE}/`));
}

test.describe('sub-path deploy (OTS_BASE_PATH=/ots/)', () => {
  test('a deep link renders its page and keeps every request below the base', async ({ page, baseURL }) => {
    const origin = new URL(baseURL!).origin;
    const reqs = ownRequests(page, origin);
    await stubBackend(page);

    const res = await page.goto(`${BASE}/docs/smoke`);
    expect(res?.status()).toBe(200);
    await expect(page.locator('article h1', { hasText: 'Smoke doc' })).toBeVisible();

    // Links inside the rendered docs, and the app's own <Link>s, carry the base.
    await expect(page.locator('article a', { hasText: 'the datasets' })).toHaveAttribute('href', `${BASE}/datasets`);
    await expect(page.locator('article a', { hasText: 'SHACL' })).toHaveAttribute('href', `${BASE}/docs/shacl`);
    const brand = page.locator('a.brand-link').first();
    await expect(brand).toHaveAttribute('href', `${BASE}/`);

    // In-app navigation stays below the base and renders the next route.
    await page.locator('article a', { hasText: 'SHACL' }).click();
    await expect(page).toHaveURL(`${origin}${BASE}/docs/shacl`);
    await expect(page.locator('article h1', { hasText: 'SHACL page' })).toBeVisible();

    // Back/forward go through the router too.
    await page.goBack();
    await expect(page).toHaveURL(`${origin}${BASE}/docs/smoke`);
    await expect(page.locator('article h1', { hasText: 'Smoke doc' })).toBeVisible();

    const paths = reqs.map((r) => new URL(r.url()).pathname);
    expect(paths).toContain(`${BASE}/config.json`);
    expect(paths.some((p) => p.startsWith(`${BASE}/assets/`))).toBe(true);
    expect(paths).toContain(`${BASE}/api/docs/smoke`);
    expect(escapedTheBase(reqs)).toEqual([]);
  });

  test('a page load of an API-shared path gets the SPA, not the backend', async ({ page, baseURL }) => {
    const reqs = ownRequests(page, new URL(baseURL!).origin);
    await stubBackend(page);
    // /sparql is both the SPARQL endpoint and the editor page; a browser load
    // must be answered with the app (the backend here does not exist).
    const res = await page.goto(`${BASE}/sparql`);
    expect(res?.status()).toBe(200);
    expect(res?.headers()['content-type'] ?? '').toContain('text/html');
    await expect(page.locator('#app')).not.toBeEmpty();
    expect(escapedTheBase(reqs)).toEqual([]);
  });

  test('an /embed deep link below the base mounts the embed app', async ({ page, baseURL }) => {
    const reqs = ownRequests(page, new URL(baseURL!).origin);
    await stubBackend(page);
    await page.goto(`${BASE}/embed`);
    // EmbedApp's own usage hint (no viewer kind given): the full app's chrome is absent.
    await expect(page.getByText('Open Triplestore embed')).toBeVisible();
    await expect(page.locator('a.brand-link')).toHaveCount(0);
    expect(escapedTheBase(reqs)).toEqual([]);
  });
});
