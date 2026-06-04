// App-wide dark mode controller (Svelte). Mirrors a shared `theme.ts` so
// both apps behave identically: persist the user's explicit choice to
// localStorage; until they choose, follow the OS preference. The signal is
// applied as BOTH a `data-theme="dark"` attribute and a `.dark` class on
// <html>, so the SPARQL editor (which reads either) stays in sync.

import { writable, get } from 'svelte/store';

const STORAGE_KEY = 'ldapps-theme';

function getStored(): 'dark' | 'light' | null {
  try {
    const v = localStorage.getItem(STORAGE_KEY);
    return v === 'dark' || v === 'light' ? v : null;
  } catch {
    return null;
  }
}

function resolve(stored: 'dark' | 'light' | null): boolean {
  if (stored) return stored === 'dark';
  if (typeof window === 'undefined' || !window.matchMedia) return false;
  return window.matchMedia('(prefers-color-scheme: dark)').matches;
}

function apply(dark: boolean): void {
  const el = document.documentElement;
  el.classList.toggle('dark', dark);
  el.setAttribute('data-theme', dark ? 'dark' : 'light');
}

/** Whether the editor/UI should render dark. Reactive (subscribe in components). */
export const isDark = writable(false);

/** Apply the persisted/OS-derived theme. Call once, early in app boot. */
export function initTheme(): void {
  const dark = resolve(getStored());
  apply(dark);
  isDark.set(dark);
  if (typeof window !== 'undefined' && window.matchMedia) {
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    mq.addEventListener?.('change', () => {
      if (getStored() === null) {
        const next = resolve(null);
        apply(next);
        isDark.set(next);
      }
    });
  }
}

export function setTheme(theme: 'dark' | 'light'): void {
  try { localStorage.setItem(STORAGE_KEY, theme); } catch { /* private mode etc. */ }
  const dark = theme === 'dark';
  apply(dark);
  isDark.set(dark);
}

export function toggleTheme(): void {
  setTheme(get(isDark) ? 'light' : 'dark');
}
