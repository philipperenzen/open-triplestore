import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';
import { renderMarkdown } from '../markdown.js';

// Renders the real docs/spark.md through the production renderer and checks the
// widget-grammar examples (three-backtick fences shown literally inside
// four-backtick fences) survive rendering, and that every link resolves.

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
const md = readFileSync(resolve(FRONTEND_ROOT, '../docs/spark.md'), 'utf8');
const { html, headings } = renderMarkdown(md);

describe('docs/spark.md', () => {
  it('renders to non-empty HTML with a table of contents', () => {
    expect(html.length).toBeGreaterThan(1000);
    expect(headings.length).toBeGreaterThan(8);
  });

  it('shows every widget-block fence literally (nested-fence examples survive)', () => {
    for (const tag of ['```sparql', '```api', '```chart', '```map', '```card', '```csv']) {
      expect(html).toContain(tag);
    }
  });

  it('documents the LLM configuration variables', () => {
    for (const v of ['LLM_GATEWAY_URL', 'LLM_MODEL', 'LLM_API_KEY']) {
      expect(html).toContain(v);
    }
  });

  it('every in-app /docs link points to a seeded page (no dead links)', () => {
    const docLinks = [...html.matchAll(/href="\/docs\/([\w-]+)"/g)].map((m) => m[1]);
    expect(docLinks.length).toBeGreaterThan(0);
    for (const slug of docLinks) expect(SEEDED_SLUGS).toContain(slug);
  });

  it('resolves every in-page anchor to a real heading id', () => {
    const ids = new Set(headings.map((h) => h.id));
    const anchors = [...html.matchAll(/href="#([\w-]+)"/g)].map((m) => m[1]);
    expect(anchors.length).toBeGreaterThan(0);
    for (const a of anchors) expect(ids).toContain(a);
  });
});
