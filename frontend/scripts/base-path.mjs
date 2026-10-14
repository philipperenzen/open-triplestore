// OTS_BASE_PATH → the `base` Vite wants: a leading and a trailing slash, so
// `ots`, `/ots` and `/ots/` all publish the UI under https://host/ots/. Empty
// or `/` is the root. Shared by vite.config.js and base-path.test.mjs.

// First path segments the app itself uses (the SPA's routes, the backend's
// routes, the static directories). The UI leaves a path that already starts
// with the base alone, so prefixing twice is harmless (withBase in
// src/lib/basePath.ts); a base such as `/api/` would make every `/api/…` call
// look prefixed already. Refusing these first segments keeps that unambiguous.
export const RESERVED_FIRST_SEGMENTS = new Set([
  'admin', 'api', 'api-docs', 'assets', 'browse', 'cesium', 'chat', 'datasets', 'docs', 'embed',
  'files', 'forgot-password', 'graph-viz', 'graphs', 'groups', 'health', 'import', 'ldp', 'livez',
  'login', 'models', 'oauth', 'organisations', 'register', 'registry', 'reset-password',
  'resource', 'samples', 'settings', 'shacl', 'sources', 'sparql', 'store', 'validation',
  'verify-email', 'vocab', 'vocabularies', 'wasm', '.well-known',
]);

export function normaliseBasePath(raw) {
  const trimmed = String(raw ?? '').trim().replace(/^\/+|\/+$/g, '');
  if (!trimmed) return '/';
  if (/[?#\s\\%]|:\/\/|\/\/|(^|\/)\.\.?(\/|$)/.test(trimmed)) {
    throw new Error(`OTS_BASE_PATH must be a plain URL path such as /ots/, got ${JSON.stringify(raw)}`);
  }
  const first = trimmed.split('/')[0];
  if (RESERVED_FIRST_SEGMENTS.has(first)) {
    throw new Error(
      `OTS_BASE_PATH ${JSON.stringify(raw)} starts with /${first}/, a path the app itself uses; pick another prefix`,
    );
  }
  return `/${trimmed}/`;
}
