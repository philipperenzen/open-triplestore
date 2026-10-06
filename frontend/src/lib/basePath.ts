// Sub-path deployments (OTS_BASE_PATH, see docs/operations.md).
//
// The web UI can be published under a path prefix such as
// `https://example.org/ots/`. Vite stamps that prefix into the bundle as
// `import.meta.env.BASE_URL` (`/ots/`; `/` for a root deploy), and this module
// is the one place the rest of the app learns it from.
//
// The convention everywhere else: a root-absolute path written in this code base
// (`/datasets/x`, `/api/auth/me`, `/sparql`) or handed over by the server (the
// viewer feed's `/api/…/download`, an API service's `/api/…/run`) is an *app*
// path, relative to the deployment's base. It gets the prefix when it leaves
// for the browser: a fetch, an `<a href>`, `history.pushState`,
// `window.location`. The router works the other way round: it strips the prefix
// off `window.location.pathname` before matching routes, so `<Route path>` and
// `<Link to>` never mention it.

/**
 * Turn Vite's `BASE_URL` into a path prefix without a trailing slash: `/` → `''`,
 * `/ots/` → `/ots`, `/a/b` → `/a/b`. A relative base (`./`, `''`) or a full URL
 * cannot be resolved into one fixed prefix, so it counts as the root.
 */
export function normaliseBase(raw: string | undefined | null): string {
  const b = String(raw ?? '').trim();
  if (!b.startsWith('/') || b.startsWith('//')) return '';
  return b.replace(/\/+$/, '');
}

function viteBase(): string | undefined {
  // Spelled out in full: Vite replaces exactly `import.meta.env.BASE_URL` (and
  // vitest's stubEnv reaches only that form too); a cast or `?.` would not be.
  try {
    return import.meta.env.BASE_URL;
  } catch {
    return undefined;
  }
}

/** The deployment's path prefix: `''` at the root, `/ots` under `/ots/`. */
export const BASE_PATH: string = normaliseBase(viteBase());

/** True when `path` already sits under `base` (`/ots`, `/ots/…`, `/ots?…`, `/ots#…`). */
function underBase(path: string, base: string): boolean {
  if (!path.startsWith(base)) return false;
  const next = path.charAt(base.length);
  return next === '' || next === '/' || next === '?' || next === '#';
}

/**
 * Prefix a root-absolute app path with the deployment base. Anything else is
 * returned unchanged: absolute URLs (`https://…`), protocol-relative `//host`,
 * relative paths, `?query` / `#hash` only, `data:`/`blob:` URLs. A path that is
 * already under the base is left alone too, so applying this twice is harmless
 * — which is why the base must not be the same as one of the app's own top
 * level paths (`/api`, `/sparql`, `/datasets` …).
 */
export function withBase(path: string, base: string = BASE_PATH): string {
  if (!base || typeof path !== 'string') return path;
  if (!path.startsWith('/') || path.startsWith('//')) return path;
  if (underBase(path, base)) return path;
  return base + path;
}

/**
 * The app path of a browser pathname: the base prefix removed, `/` for the base
 * itself. A pathname outside the base is returned as it is (the app does not
 * own it, and no route will match it).
 */
export function stripBase(pathname: string, base: string = BASE_PATH): string {
  const p = pathname || '/';
  if (!base) return p;
  if (p === base) return '/';
  if (p.startsWith(base + '/')) return p.slice(base.length);
  return p;
}

/**
 * Like [withBase] for a full same-origin URL (what Swagger UI and other
 * libraries build from a server-relative spec): a URL on this origin whose
 * path is not under the base gets the prefix; other origins are untouched.
 */
export function withBaseUrl(url: string, origin: string, base: string = BASE_PATH): string {
  if (!base) return url;
  let u: URL;
  try {
    u = new URL(url, origin);
  } catch {
    return url;
  }
  if (u.origin !== origin || underBase(u.pathname, base)) return url;
  u.pathname = base + u.pathname;
  return u.href;
}

/** Whether a browser pathname is one of the chrome-less `/embed/*` pages (see main.ts). */
export function isEmbedPath(pathname: string, base: string = BASE_PATH): boolean {
  const p = pathname || '/';
  if (base && p !== base && !p.startsWith(base + '/')) return false;
  return /^\/embed(\/|$)/.test(stripBase(p, base));
}

/**
 * The full browser-facing URL of an app path on this origin — for the "copy
 * endpoint URL" buttons and embed snippets, which leave the app as text.
 */
export function absoluteUrl(path: string, origin: string = typeof window !== 'undefined' ? window.location.origin : '', base: string = BASE_PATH): string {
  return `${origin}${withBase(path, base)}`;
}
