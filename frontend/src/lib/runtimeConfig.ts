// Runtime configuration — lets a container operator customize a deployed
// instance (backend URLs + branding) WITHOUT rebuilding the frontend bundle.
//
// On boot, fetches `/config.json`. This app's own backend serves the static
// frontend from `frontend/dist` (see `src/server/mod.rs`'s `ServeDir` +
// SPA-fallback), so an operator can drop a `config.json` file straight into
// that directory — e.g. a Docker volume mount over the built image — with NO
// backend code changes and NO rebuild:
//
//   {
//     "services": { "triplestore": "https://api.example.com" },
//     "branding": { "title": "Acme Graph", "logoUrl": "/acme-logo.svg", "accent": "#7a2fe0" }
//   }
//
// Both keys are optional; anything omitted keeps the existing default/registry
// value. Fail-soft: no file (this app's own SPA fallback then serves
// `index.html`'s `text/html` instead of JSON) or a malformed one is treated as
// "no runtime config" — never a hard error, never a broken boot.
//
// Precedence for backend URLs (highest first): VITE_<SERVICE>_URL build-time
// env > this runtime config > `/registry` discovery > localhost defaults. See
// `serviceRegistry.ts`, which this module feeds via `setRuntimeServiceOverrides`.

import { writable } from 'svelte/store';
import { setRuntimeServiceOverrides } from './serviceRegistry.js';

export interface RuntimeBranding {
  title: string;
  /** Absolute or root-relative URL to a logo image; `null` = use the built-in mark. */
  logoUrl: string | null;
  /** CSS color (hex/rgb/etc.) applied as the app's primary brand accent; `null` = built-in. */
  accent: string | null;
}

const DEFAULT_BRANDING: RuntimeBranding = { title: 'Open Triplestore', logoUrl: null, accent: null };

/** Reactive branding — components (e.g. the header/title) subscribe to this. */
export const runtimeBranding = writable<RuntimeBranding>(DEFAULT_BRANDING);

let started = false;

interface RuntimeConfigDoc {
  services?: Record<string, string>;
  branding?: { title?: string; logoUrl?: string; accent?: string };
}

function applyBranding(branding: RuntimeConfigDoc['branding']): void {
  if (!branding) return;
  const next: RuntimeBranding = {
    title: branding.title?.trim() || DEFAULT_BRANDING.title,
    logoUrl: branding.logoUrl?.trim() || null,
    accent: branding.accent?.trim() || null,
  };
  runtimeBranding.set(next);

  if (typeof document === 'undefined') return;
  document.title = next.title;
  if (next.logoUrl) {
    const favicon = document.querySelector<HTMLLinkElement>('link[rel="icon"]');
    if (favicon) favicon.href = next.logoUrl;
  }
  if (next.accent) {
    // A single override for the app's primary brand swatch (theme.css's
    // `--brand-600`) — see theme.css for the full ramp this sits alongside.
    document.documentElement.style.setProperty('--brand-600', next.accent);
  }
}

/**
 * Fetch and apply `/config.json`. Safe to call once at boot; a no-op (keeps
 * defaults) when the file is absent or malformed. Does not block app mount —
 * call without awaiting, same as `initServiceRegistry`'s one-shot resolve.
 */
export function loadRuntimeConfig(): void {
  if (started || typeof window === 'undefined') return;
  started = true;

  fetch('/config.json', { headers: { accept: 'application/json' } })
    .then((r) => {
      // This app's SPA fallback serves `index.html` (text/html, 200) for any
      // path with no matching static file — including a missing config.json —
      // so `r.ok` alone can't distinguish "no config" from "real config".
      if (!r.ok || !r.headers.get('content-type')?.includes('application/json')) return null;
      return r.json();
    })
    .then((doc: RuntimeConfigDoc | null) => {
      if (!doc) return;
      if (doc.services) setRuntimeServiceOverrides(doc.services);
      applyBranding(doc.branding);
    })
    .catch(() => { /* no runtime config — keep built-in defaults */ });
}
