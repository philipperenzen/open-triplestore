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
// registry never breaks the app. Precedence at each call site, highest first:
//   1. VITE_<SERVICE>_URL   — build-time env var (e.g. VITE_TRIPLESTORE_URL), baked into the
//                             bundle. For static deploys (GitLab Pages, no registry proxy)
//                             that point at an external backend.
//   2. runtime config       — /config.json's "services" map, applied WITHOUT a rebuild (see
//                             runtimeConfig.ts); set via setRuntimeServiceOverrides.
//   3. /registry discovery  — live, opt-in (LD_DISCOVERY) cross-app SSE resolution.
//   4. DEFAULTS             — localhost dev ports.

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

/** `my-service` -> `VITE_MY_SERVICE_URL`, matched against `import.meta.env` at build time. */
function viteEnvVarName(name: string): string {
  return `VITE_${name.toUpperCase().replace(/-/g, '_')}_URL`
}

/**
 * Highest-precedence overrides: `VITE_<SERVICE>_URL` build-time env vars.
 * Resolved once at module load (Vite bakes `import.meta.env` in at build
 * time, so this can never change at runtime).
 */
const VITE_OVERRIDES: Partial<Record<ServiceName, string>> = (() => {
  const out: Partial<Record<ServiceName, string>> = {}
  const env = import.meta.env as unknown as Record<string, string | undefined>
  for (const name of Object.keys(DEFAULTS) as ServiceName[]) {
    const v = env[viteEnvVarName(name)]
    if (typeof v === 'string' && v.trim()) out[name] = v.trim().replace(/\/+$/, '')
  }
  return out
})()

/** Second-highest precedence: the runtime `/config.json` "services" map (see runtimeConfig.ts). */
let RUNTIME_OVERRIDES: Partial<Record<ServiceName, string>> = {}

/**
 * Apply the runtime-config service map (called by `runtimeConfig.ts` once
 * `/config.json` resolves). Values here are outranked by `VITE_*_URL` but
 * outrank `/registry` discovery and the localhost DEFAULTS.
 */
export function setRuntimeServiceOverrides(services: Record<string, string>): void {
  let changed = false
  for (const [name, url] of Object.entries(services)) {
    if (typeof url === 'string' && url && RUNTIME_OVERRIDES[name as ServiceName] !== url) {
      RUNTIME_OVERRIDES = { ...RUNTIME_OVERRIDES, [name]: url }
      changed = true
    }
  }
  if (changed) broadcast()
}

/** Resolve a logical service name to its current base URL, per the precedence above. */
export function getService(name: ServiceName): string {
  return VITE_OVERRIDES[name] || RUNTIME_OVERRIDES[name] || MAP[name] || DEFAULTS[name]
}

/** A copy of the whole current, precedence-resolved map (debugging / bulk reads). */
export function getServiceMap(): Record<string, string> {
  const out: Record<string, string> = {}
  for (const name of Object.keys(DEFAULTS) as ServiceName[]) out[name] = getService(name)
  return out
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
    // The full precedence-resolved map (VITE_*_URL / runtime config / registry
    // / defaults) — not just the registry-tier MAP — so a listener sees the
    // same values getService() would return.
    window.dispatchEvent(new CustomEvent(SERVICE_CHANGE_EVENT, { detail: { map: getServiceMap() } }))
  }
}

/**
 * Initialise discovery: a one-shot resolve plus a live SSE subscription. Safe to call once at
 * boot. Opt-in — a no-op unless LD_DISCOVERY is set (Vite injects `__LD_DISCOVERY__`); this keeps
 * the app from contacting a registry that isn't running. All failures are swallowed — the app
 * keeps the seeded DEFAULTS (fail-soft).
 */
export function initServiceRegistry(): void {
  if (started || typeof window === 'undefined') return
  // Discovery is opt-in: with it off, keep the localhost DEFAULTS and contact no registry — this
  // avoids the /registry/events SSE reconnect loop when no registry is running.
  if (typeof __LD_DISCOVERY__ === 'undefined' || !__LD_DISCOVERY__) return
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
