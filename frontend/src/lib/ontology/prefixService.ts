// prefix.cc-backed prefix lookup with localStorage cache.
// Seeded from NAMESPACES so offline always works for common prefixes.

import { NAMESPACES, VOCAB_INFO } from './vocabularies.js';

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

// ───────────────────────────────────────────────────────────────────────────
// Unified prefix / vocabulary search
//
// Surfaces three kinds of prefix candidates from one query: the curated
// built-ins (always available, no network), prefix.cc (the community registry,
// best-effort over the network), and on-platform registered vocabularies
// (passed in by the caller via `extra` so this module stays free of API deps).
// ───────────────────────────────────────────────────────────────────────────

export type PrefixSource = 'builtin' | 'prefix.cc' | 'platform';

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

/**
 * Search for prefix/vocabulary candidates matching `query`.
 *
 * Always returns the matching built-ins synchronously-derivable from the local
 * tables, then (best-effort, swallowing failures) augments with a direct
 * prefix.cc resolution when the query looks like a bare prefix label. Any
 * `extra` candidates (e.g. on-platform vocabularies supplied by the caller) are
 * merged and ranked alongside the rest.
 *
 * De-duplicates by `prefix` (case-insensitive), preferring built-in > platform
 * > prefix.cc so a curated description always wins. Results are ranked by
 * relevance to `query`.
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

  // prefix.cc direct label resolution — only when the query is a plausible bare
  // prefix and we don't already have a built-in/platform hit for that exact label.
  if (remote && q && BARE_PREFIX_RE.test(q)) {
    const haveExact = candidates.some((c) => c.prefix.toLowerCase() === q);
    if (!haveExact) {
      try {
        const ns = await lookupPrefix(q);
        if (ns) candidates.push({ prefix: q, namespace: ns, source: 'prefix.cc' });
      } catch { /* offline / not found — ignore */ }
    }
  }

  // De-dupe by prefix, keeping the highest-priority source.
  const priority: Record<PrefixSource, number> = { builtin: 3, platform: 2, 'prefix.cc': 1 };
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
