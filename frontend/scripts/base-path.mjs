// OTS_BASE_PATH → the `base` Vite wants: a leading and a trailing slash, so
// `ots`, `/ots` and `/ots/` all publish the UI under https://host/ots/. Empty
// or `/` is the root. Shared by vite.config.js and base-path.test.mjs.

// First path segments the app itself uses (the SPA's routes, the backend's
// routes, the static directories). The UI tells an app path from one that
// already carries the base by its first segment (src/lib/basePath.ts), so a
// base that starts with one of these would make `/api/…` look prefixed.
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
