// prefix.cc-backed prefix lookup with localStorage cache.
// Seeded from NAMESPACES so offline always works for common prefixes.

import { NAMESPACES } from './vocabularies.js';

const CACHE_KEY = 'prefixcc_cache_v1';
const NEG_TTL_MS = 1000 * 60 * 60 * 24; // 24h for misses

interface CacheEntry {
  iri: string | null;
  t: number;
}

let cache: Record<string, CacheEntry> | null = null;

function loadCache() {
  if (cache) return cache;
  try {
    cache = JSON.parse(localStorage.getItem(CACHE_KEY) || '{}');
  } catch { cache = {}; }
  for (const [p, iri] of Object.entries(NAMESPACES)) {
    if (!cache[p]) cache[p] = { iri, t: 0 };
  }
  return cache;
}

function saveCache() {
  try { localStorage.setItem(CACHE_KEY, JSON.stringify(cache)); } catch {}
}

export function lookupPrefixSync(prefix: string): string | null {
  const c = loadCache();
  const hit = c[prefix];
  return hit && hit.iri ? hit.iri : null;
}

export async function lookupPrefix(prefix: string): Promise<string | null> {
  const c = loadCache();
  const hit = c[prefix];
  if (hit && hit.iri) return hit.iri;
  if (hit && !hit.iri && Date.now() - hit.t < NEG_TTL_MS) return null;
  try {
    const res = await fetch(`https://prefix.cc/${encodeURIComponent(prefix)}.file.json`);
    if (!res.ok) throw new Error('not found');
    const data = await res.json();
    const iri = data[prefix];
    if (iri) {
      c[prefix] = { iri, t: Date.now() };
      saveCache();
      return iri;
    }
  } catch {}
  c[prefix] = { iri: null, t: Date.now() };
  saveCache();
  return null;
}

export function warmPrefix(prefix: string): void {
  if (!lookupPrefixSync(prefix)) lookupPrefix(prefix);
}

// namespace IRI -> prefix label, inverted from the same cache + NAMESPACES seed.
let reverse: Record<string, string> | null = null;
function reverseMap(): Record<string, string> {
  const c = loadCache();
  reverse = {};
  for (const [p, entry] of Object.entries(c)) {
    if (entry.iri && !(entry.iri in reverse)) reverse[entry.iri] = p;
  }
  return reverse;
}

/** Resolve a namespace IRI to its prefix label (e.g. the SKOS namespace → "skos"),
 *  using the cached prefix.cc / NAMESPACES data. Returns null if unknown. */
export function prefixForNamespace(ns: string): string | null {
  if (!ns) return null;
  return reverseMap()[ns] || null;
}

/** Kick off a prefix.cc reverse lookup for an unknown namespace (best-effort,
 *  cached). prefix.cc's reverse endpoint maps a URI back to its prefix. */
export async function lookupNamespacePrefix(ns: string): Promise<string | null> {
  const known = prefixForNamespace(ns);
  if (known) return known;
  try {
    const res = await fetch(`https://prefix.cc/reverse?uri=${encodeURIComponent(ns)}&format=json`);
    if (!res.ok) throw new Error('not found');
    const data = await res.json();
    const prefix = Object.keys(data || {})[0];
    if (prefix && data[prefix]) {
      const c = loadCache();
      if (!c[prefix]) { c[prefix] = { iri: data[prefix], t: Date.now() }; saveCache(); reverse = null; }
      return prefix;
    }
  } catch {}
  return null;
}

export function extractDeclaredPrefixes(query: string): Record<string, string> {
  const out = {};
  const re = /PREFIX\s+([a-zA-Z_][\w-]*)\s*:\s*<([^>]+)>/gi;
  let m;
  while ((m = re.exec(query))) out[m[1]] = m[2];
  return out;
}

export function extractUsedPrefixes(query: string): string[] {
  const out = new Set<string>();
  const stripped = query.replace(/<[^>]*>/g, '').replace(/"(?:[^"\\]|\\.)*"/g, '');
  const re = /\b([a-zA-Z_][\w-]*):([a-zA-Z_][\w-]*)/g;
  let m;
  while ((m = re.exec(stripped))) {
    if (m[1].toUpperCase() === 'PREFIX' || m[1].toUpperCase() === 'BASE') continue;
    out.add(m[1]);
  }
  return [...out];
}
