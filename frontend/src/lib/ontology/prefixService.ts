// Internal prefix-service lookup with localStorage cache.
// Seeded from NAMESPACES so offline always works for common prefixes; all
// remote lookups go to this platform's own /api/prefixes service (a bundled
// prefix.cc + LOV snapshot plus registered vocabularies) — the browser never
// calls prefix.cc directly (the public service is unreliable: expired TLS,
// long outages).

import { NAMESPACES, VOCAB_INFO } from './vocabularies.js';

// v2: entries now come from the internal /api/prefixes service (the old v1
// cache held direct prefix.cc responses; a stale v1 entry could shadow a
// platform-registered prefix, so the key is bumped).
const CACHE_KEY = 'prefix_cache_v2';
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
    const res = await fetch(`/api/prefixes/${encodeURIComponent(prefix)}`);
    if (!res.ok) throw new Error('not found');
    const data = await res.json();
    if (data && data.namespace) {
      c[prefix] = { iri: data.namespace, t: Date.now() };
      saveCache();
      return data.namespace;
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
 *  using the cached internal-service / NAMESPACES data. Returns null if unknown. */
export function prefixForNamespace(ns: string): string | null {
  if (!ns) return null;
  return reverseMap()[ns] || null;
}

/** Reverse-resolve an unknown namespace via the internal prefix service
 *  (best-effort, cached). Falls back to longest-namespace matching on the
 *  server, so term IRIs resolve too. */
export async function lookupNamespacePrefix(ns: string): Promise<string | null> {
  const known = prefixForNamespace(ns);
  if (known) return known;
  try {
    const res = await fetch(`/api/prefixes/reverse?uri=${encodeURIComponent(ns)}`);
    if (!res.ok) throw new Error('not found');
    const data = await res.json();
    if (data && data.prefix && data.namespace) {
      const c = loadCache();
      if (!c[data.prefix]) {
        c[data.prefix] = { iri: data.namespace, t: Date.now() };
        saveCache();
        reverse = null;
      }
      return data.prefix;
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

// ───────────────────────────────────────────────────────────────────────────
// Unified prefix / vocabulary search
//
// Surfaces prefix candidates from one query: the curated built-ins (always
// available, no network), the internal prefix service (bundled prefix.cc +
// LOV snapshot and platform-registered vocabularies), and any candidates the
// caller passes via `extra` (so this module stays free of API deps).
// ───────────────────────────────────────────────────────────────────────────

export type PrefixSource = 'builtin' | 'prefix.cc' | 'platform' | 'lov';

export interface PrefixCandidate {
  prefix: string;
  namespace: string;
  source: PrefixSource;
  title?: string;
  description?: string;
  homepage?: string;
}

/** A bare prefix label is what you can put before a ':' in SPARQL/Turtle. */
const BARE_PREFIX_RE = /^[a-zA-Z][\w-]*$/;

function scoreCandidate(c: PrefixCandidate, q: string): number {
  if (!q) return 0;
  const prefix = c.prefix.toLowerCase();
  const title = (c.title || '').toLowerCase();
  const desc = (c.description || '').toLowerCase();
  const ns = (c.namespace || '').toLowerCase();
  let s = 0;
  if (prefix === q) s += 1000;
  else if (prefix.startsWith(q)) s += 600;
  else if (prefix.includes(q)) s += 300;
  if (title === q) s += 250;
  else if (title.startsWith(q)) s += 120;
  else if (title.includes(q)) s += 60;
  if (ns.includes(q)) s += 40;
  if (desc.includes(q)) s += 20;
  // Prefer the trustworthy, offline-first sources on ties.
  if (c.source === 'builtin') s += 5;
  else if (c.source === 'platform') s += 3;
  return s;
}

/** Collect built-in matches from NAMESPACES + VOCAB_INFO. */
function builtinMatches(q: string): PrefixCandidate[] {
  const out: PrefixCandidate[] = [];
  for (const [prefix, namespace] of Object.entries(NAMESPACES)) {
    const info = VOCAB_INFO[prefix];
    const hay = `${prefix} ${info?.title || ''} ${info?.description || ''} ${namespace}`.toLowerCase();
    if (q && !hay.includes(q)) continue;
    out.push({
      prefix,
      namespace,
      source: 'builtin',
      title: info?.title,
      description: info?.description,
      homepage: info?.homepage,
    });
  }
  return out;
}

/** Map a server-side source tag onto the candidate source vocabulary. */
function serverSource(source: string): PrefixSource {
  if (source === 'platform') return 'platform';
  if (source === 'lov') return 'lov';
  return 'prefix.cc';
}

/**
 * Search for prefix/vocabulary candidates matching `query`.
 *
 * Always returns the matching built-ins synchronously-derivable from the local
 * tables, then (best-effort, swallowing failures) augments with ranked matches
 * from the internal prefix service. Any `extra` candidates (e.g. on-platform
 * vocabularies supplied by the caller) are merged and ranked alongside.
 *
 * De-duplicates by `prefix` (case-insensitive), preferring built-in > platform
 * > snapshot sources so a curated description always wins. Results are ranked
 * by relevance to `query`.
 */
export async function searchPrefixes(
  query: string,
  extra: PrefixCandidate[] = [],
  opts: { limit?: number; remote?: boolean } = {},
): Promise<PrefixCandidate[]> {
  const q = (query || '').trim().toLowerCase();
  const limit = opts.limit ?? 30;
  const remote = opts.remote !== false;

  const candidates: PrefixCandidate[] = [...builtinMatches(q)];

  // On-platform / caller-supplied candidates, filtered by the same query.
  for (const c of extra) {
    if (!c || !c.prefix || !c.namespace) continue;
    const hay = `${c.prefix} ${c.title || ''} ${c.description || ''} ${c.namespace}`.toLowerCase();
    if (q && !hay.includes(q)) continue;
    candidates.push(c);
  }

  // Internal prefix service: ranked substring search over ~3.7k prefixes
  // (bundled prefix.cc + LOV) plus platform-registered vocabularies.
  if (remote && q && (BARE_PREFIX_RE.test(q) || q.length >= 3)) {
    try {
      const res = await fetch(`/api/prefixes?q=${encodeURIComponent(q)}&limit=${limit}`);
      if (res.ok) {
        const data = await res.json();
        for (const r of data?.results || []) {
          if (!r?.prefix || !r?.namespace) continue;
          candidates.push({
            prefix: r.prefix,
            namespace: r.namespace,
            source: serverSource(r.source),
          });
        }
      }
    } catch { /* offline — built-ins still work */ }
  }

  // De-dupe by prefix, keeping the highest-priority source.
  const priority: Record<PrefixSource, number> = { builtin: 4, platform: 3, 'prefix.cc': 2, lov: 1 };
  const byPrefix = new Map<string, PrefixCandidate>();
  for (const c of candidates) {
    const key = c.prefix.toLowerCase();
    const existing = byPrefix.get(key);
    if (!existing || priority[c.source] > priority[existing.source]) {
      // Merge: keep a description/title if the winner lacks one.
      byPrefix.set(key, {
        ...c,
        title: c.title || existing?.title,
        description: c.description || existing?.description,
        homepage: c.homepage || existing?.homepage,
      });
    }
  }

  const ranked = [...byPrefix.values()].sort((a, b) => {
    const d = scoreCandidate(b, q) - scoreCandidate(a, q);
    if (d !== 0) return d;
    return a.prefix.localeCompare(b.prefix);
  });
  return ranked.slice(0, limit);
}
