import { writable } from 'svelte/store';

interface Location {
  pathname: string;
  search: string;
  hash: string;
  href: string;
}

function readLocation(): Location {
  if (typeof window === 'undefined') {
    return { pathname: '/', search: '', hash: '', href: '/' };
  }

  const { pathname, search, hash } = window.location;
  return {
    pathname: pathname || '/',
    search: search || '',
    hash: hash || '',
    href: `${pathname || '/'}${search || ''}${hash || ''}`,
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

export function navigate(to: string, { replace = false }: { replace?: boolean } = {}): void {
  if (typeof window === 'undefined') {
    return;
  }

  const href = String(to || '/');
  if (replace) {
    window.history.replaceState({}, '', href);
  } else {
    window.history.pushState({}, '', href);
  }

  syncLocation();
}
