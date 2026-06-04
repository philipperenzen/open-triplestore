// Service-registry client — optional cross-app discovery for companion tools, kept
// in sync with any companion frontends that share this file.
//
// Replaces hardcoded http://localhost:PORT for sibling services with discovery:
//   getService('form-app')  ->  wherever the registry currently says a sibling app lives.
// On boot it fetches the resolve-all map then subscribes to the registry's SSE change stream,
// re-broadcasting each change as a window CustomEvent 'ldapps-service-change' (same idiom as
// 'ldapps-theme-change') so components can react live. This app already talks to its own backend
// same-origin via the Vite proxy; the client mainly powers cross-links + the change event.
//
// Fail-soft: every name has a localhost DEFAULT, so first paint is correct and a missing
// registry never breaks the app. Precedence at each call site: explicit VITE_* > registry > DEFAULT.

export type ServiceName =
  | 'triplestore' | 'form-service' | 'form-app' | 'validation-api' | 'validation-app'
  | 'viewer-app' | 'llm-gateway' | 'llm-backend' | 'ollama' | 'lm-studio'

const DEFAULTS: Record<ServiceName, string> = {
  'triplestore': 'http://localhost:7878',
  'form-service': 'http://localhost:8090',
  'form-app': 'http://localhost:5174',
  'validation-api': 'http://localhost:8080',
  'validation-app': 'http://localhost:5180',
  'viewer-app': 'http://localhost:5190',
  'llm-gateway': 'http://localhost:8000',
  'llm-backend': 'http://localhost:8100',
  'ollama': 'http://localhost:11434',
  'lm-studio': 'http://localhost:1234',
}

// Same-origin path the registry is reachable at (proxied by Vite in dev / nginx in prod), so no
// cross-origin registry host is baked into the bundle and there is no CORS to configure.
const REGISTRY_BASE = '/registry'

export const SERVICE_CHANGE_EVENT = 'ldapps-service-change'

const MAP: Record<string, string> = { ...DEFAULTS }
let started = false

/** Resolve a logical service name to its current base URL (registry value, else localhost default). */
export function getService(name: ServiceName): string {
  return MAP[name] || DEFAULTS[name]
}

/** A copy of the whole current map (debugging / bulk reads). */
export function getServiceMap(): Record<string, string> {
  return { ...MAP }
}

function applyServices(services: Record<string, { url?: string }> | undefined): boolean {
  if (!services) return false
  let changed = false
  for (const [name, entry] of Object.entries(services)) {
    const url = entry && entry.url
    if (typeof url === 'string' && url && MAP[name] !== url) {
      MAP[name] = url
      changed = true
    }
  }
  return changed
}

function broadcast(): void {
  if (typeof window !== 'undefined') {
    window.dispatchEvent(new CustomEvent(SERVICE_CHANGE_EVENT, { detail: { map: { ...MAP } } }))
  }
}

/**
 * Initialise discovery: a one-shot resolve plus a live SSE subscription. Safe to call once at
 * boot. All failures are swallowed — the app keeps the seeded DEFAULTS (fail-soft).
 */
export function initServiceRegistry(): void {
  if (started || typeof window === 'undefined') return
  started = true

  // 1) One-shot resolve so the live map is in place ASAP (even before SSE connects).
  fetch(`${REGISTRY_BASE}/resolve`, { headers: { accept: 'application/json' } })
    .then((r) => (r.ok ? r.json() : null))
    .then((data) => { if (data && applyServices(data.services)) broadcast() })
    .catch(() => { /* registry down — keep DEFAULTS */ })

  // 2) Live updates via SSE, re-broadcast as a window CustomEvent. EventSource auto-reconnects.
  try {
    const es = new EventSource(`${REGISTRY_BASE}/events`)
    const onMsg = (ev: MessageEvent) => {
      try {
        const data = JSON.parse(ev.data)
        if (applyServices(data.services)) broadcast()
      } catch { /* ignore a malformed frame */ }
    }
    es.addEventListener('snapshot', onMsg as EventListener)
    es.addEventListener('change', onMsg as EventListener)
    window.addEventListener('pagehide', () => es.close(), { once: true })
  } catch { /* EventSource unsupported/blocked — the one-shot fetch above still applied */ }
}
