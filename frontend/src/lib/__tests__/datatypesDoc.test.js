import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';
import { renderMarkdown } from '../markdown.js';

// Renders the real docs/datatypes.md through the production renderer and checks
// the "view examples" affordances actually survive (collapsible blocks, code
// highlighting, runnable links) and that every in-app link targets a real page.

// The slugs the backend seeds (src/docs/mod.rs BUILTINS). A /docs/<slug> link to
// anything not in here would 404 in-app.
const SEEDED_SLUGS = new Set([
  'overview', 'named-graphs', 'datasets', 'organisations', 'versioning',
  'modelling', 'data-modeling', 'linked-data-modelling-styleguide', 'models',
  'dcat', 'formats', 'import', 'search-syntax', 'full-text-search', 'geosparql',
  'spark', 'reasoning', 'shacl', 'auth', 'security', 'api-services',
  'api-reference', 'operations', 'standards', 'datatypes', 'faq',
  'dataset-governance',
]);

// ../../.. → frontend/, then ../docs → repo-root docs/. Resolving against this
// file (not process.cwd()) keeps the test launch-directory independent. (new URL(
// ..., import.meta.url) can't be used: vite rewrites it into a dev-server asset URL.)
const FRONTEND_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const md = readFileSync(resolve(FRONTEND_ROOT, '../docs/datatypes.md'), 'utf8');
const { html, headings } = renderMarkdown(md);

describe('docs/datatypes.md', () => {
  it('renders to non-empty HTML with a table of contents', () => {
    expect(html.length).toBeGreaterThan(1000);
    expect(headings.length).toBeGreaterThan(5);
  });

  it('keeps collapsible "View example" blocks (details/summary)', () => {
    expect(html.match(/<details>/g)?.length ?? 0).toBeGreaterThanOrEqual(5);
    expect(html).toContain('<summary>');
  });

  it('syntax-highlights the fenced turtle/sparql/json examples', () => {
    expect(html).toContain('class="tok-pname"'); // turtle prefixed names
    expect(html).toContain('<span class="tok-kw">SELECT</span>'); // sparql
    expect(html).toContain('class="tok-key"'); // json keys
  });

  it('preserves runnable /sparql?query= links verbatim (not rewritten to /docs)', () => {
    const tryLinks = [...html.matchAll(/href="(\/sparql\?query=[^"]+)"/g)].map((m) => m[1]);
    expect(tryLinks.length).toBeGreaterThanOrEqual(4);
    // The encoded query must round-trip back to valid SPARQL text.
    for (const href of tryLinks) {
      const q = decodeURIComponent(href.replace('/sparql?query=', ''));
      expect(q).toMatch(/SELECT/);
    }
  });

  it('every in-app /docs link points to a seeded page (no dead links)', () => {
    const docLinks = [...html.matchAll(/href="\/docs\/([\w-]+)"/g)].map((m) => m[1]);
    expect(docLinks.length).toBeGreaterThan(0);
    for (const slug of docLinks) expect(SEEDED_SLUGS).toContain(slug);
  });

  it('resolves its only in-page anchor to a real heading id', () => {
    const ids = new Set(headings.map((h) => h.id));
    const anchors = [...html.matchAll(/href="#([\w-]+)"/g)].map((m) => m[1]);
    for (const a of anchors) expect(ids).toContain(a);
  });
});
