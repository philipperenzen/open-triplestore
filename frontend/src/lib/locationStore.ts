import { writable } from 'svelte/store';
import { stripBase, withBase } from './basePath';

interface Location {
  /** The app path: `window.location.pathname` without the deployment base (see basePath.ts). */
  pathname: string;
  search: string;
  hash: string;
  /** App-relative too: `pathname` + `search` + `hash`. */
  href: string;
}

function readLocation(): Location {
  if (typeof window === 'undefined') {
    return { pathname: '/', search: '', hash: '', href: '/' };
  }

  const { search, hash } = window.location;
  const pathname = stripBase(window.location.pathname || '/');
  return {
    pathname,
    search: search || '',
    hash: hash || '',
    href: `${pathname}${search || ''}${hash || ''}`,
  };
}

export const location = writable<Location>(readLocation());

let listening = false;

export function syncLocation(): void {
  location.set(readLocation());
}

export function ensureRouterListener(): void {
  if (listening || typeof window === 'undefined') {
    return;
  }

  listening = true;
  window.addEventListener('popstate', syncLocation);
}

/**
 * Go to an app path (`/datasets/x?tab=y`). A root-absolute path gets the
 * deployment base prefix before it reaches the history API, so callers never
 * spell it out; a path that already carries it is left alone.
 */
export function navigate(to: string, { replace = false }: { replace?: boolean } = {}): void {
  if (typeof window === 'undefined') {
    return;
  }

  const href = withBase(String(to || '/'));
  if (replace) {
    window.history.replaceState({}, '', href);
  } else {
    window.history.pushState({}, '', href);
  }

  syncLocation();
}
