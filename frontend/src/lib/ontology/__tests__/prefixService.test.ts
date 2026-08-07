// The prefix service must resolve exclusively against this platform's own
// /api/prefixes endpoints — never the public prefix.cc (expired TLS, outages).

import { describe, it, expect, vi, beforeEach } from 'vitest';

const fetchCalls: string[] = [];

function mockFetch(routes: Record<string, unknown>) {
  fetchCalls.length = 0;
  vi.stubGlobal('fetch', vi.fn(async (url: string) => {
    fetchCalls.push(String(url));
    for (const [match, body] of Object.entries(routes)) {
      if (String(url).startsWith(match)) {
        return { ok: true, json: async () => body } as Response;
      }
    }
    return { ok: false, status: 404, json: async () => ({}) } as Response;
  }));
}

async function freshService() {
  vi.resetModules();
  localStorage.clear();
  return await import('../prefixService');
}

describe('prefixService (internal endpoints)', () => {
  beforeEach(() => {
    vi.unstubAllGlobals();
  });

  it('resolves an unknown prefix via /api/prefixes/{label}', async () => {
    mockFetch({
      '/api/prefixes/bibo': {
        prefix: 'bibo',
        namespace: 'http://purl.org/ontology/bibo/',
        source: 'prefix.cc',
      },
    });
    const svc = await freshService();
    expect(await svc.lookupPrefix('bibo')).toBe('http://purl.org/ontology/bibo/');
    // Cached: a second lookup makes no further request.
    const calls = fetchCalls.length;
    expect(await svc.lookupPrefix('bibo')).toBe('http://purl.org/ontology/bibo/');
    expect(fetchCalls.length).toBe(calls);
  });

  it('serves built-in prefixes without any network call', async () => {
    mockFetch({});
    const svc = await freshService();
    expect(await svc.lookupPrefix('foaf')).toBe('http://xmlns.com/foaf/0.1/');
    expect(fetchCalls.length).toBe(0);
  });

  it('reverse-resolves via /api/prefixes/reverse', async () => {
    mockFetch({
      '/api/prefixes/reverse': {
        prefix: 'gr',
        namespace: 'http://purl.org/goodrelations/v1#',
        source: 'lov',
      },
    });
    const svc = await freshService();
    expect(await svc.lookupNamespacePrefix('http://purl.org/goodrelations/v1#')).toBe('gr');
    expect(fetchCalls[0]).toContain('/api/prefixes/reverse?uri=');
  });

  it('searchPrefixes merges internal-service hits with built-ins, ranked', async () => {
    mockFetch({
      '/api/prefixes?q=': {
        total_known: 3695,
        results: [
          { prefix: 'foafrealm', namespace: 'http://notitio.us/foafrealm/', rank: 900, source: 'prefix.cc' },
          { prefix: 'foaf', namespace: 'http://xmlns.com/foaf/0.1/', rank: 3, source: 'prefix.cc' },
        ],
      },
    });
    const svc = await freshService();
    const hits = await svc.searchPrefixes('foaf');
    expect(hits[0].prefix).toBe('foaf');
    // The built-in entry (with curated title) wins the dedupe for 'foaf'.
    expect(hits[0].source).toBe('builtin');
    expect(hits.some((h) => h.prefix === 'foafrealm')).toBe(true);
  });

  // The facet rail resolves one namespace per row and re-runs whenever any row
  // resolves, so an unresolved namespace used to be re-requested on every pass —
  // enough repeats of the same URL to trip the server's rate limiter (429).
  it('coalesces concurrent reverse lookups of one namespace into a single fetch', async () => {
    mockFetch({
      '/api/prefixes/reverse': {
        prefix: 'gr',
        namespace: 'http://purl.org/goodrelations/v1#',
        source: 'lov',
      },
    });
    const svc = await freshService();
    const ns = 'http://purl.org/goodrelations/v1#';

    // Ten components mount in the same tick and each asks for the same namespace.
    const results = await Promise.all(
      Array.from({ length: 10 }, () => svc.lookupNamespacePrefix(ns)),
    );

    expect(results).toEqual(Array(10).fill('gr'));
    expect(fetchCalls.filter((u) => u.includes('/api/prefixes/reverse')).length).toBe(1);
  });

  it('caches a reverse hit whose prefix label is already taken by another namespace', async () => {
    // 'foaf' is a built-in, so the prefix-keyed cache slot is occupied. The hit
    // still has to be recorded, or the namespace never resolves and the caller
    // re-fetches forever.
    const ns = 'https://example.org/foaf-alike#';
    mockFetch({
      '/api/prefixes/reverse': { prefix: 'foaf', namespace: ns, source: 'lov' },
    });
    const svc = await freshService();

    expect(await svc.lookupNamespacePrefix(ns)).toBe('foaf');
    expect(svc.prefixForNamespace(ns)).toBe('foaf');
    // The built-in mapping must not be clobbered by the colliding label.
    expect(await svc.lookupPrefix('foaf')).toBe('http://xmlns.com/foaf/0.1/');

    const calls = fetchCalls.length;
    expect(await svc.lookupNamespacePrefix(ns)).toBe('foaf');
    expect(fetchCalls.length).toBe(calls);
  });

  it('remembers a 404 namespace so it is not requested again', async () => {
    mockFetch({}); // everything 404s
    const svc = await freshService();
    const ns = 'http://example.org/nothing-here#';

    expect(await svc.lookupNamespacePrefix(ns)).toBe(null);
    const calls = fetchCalls.length;
    expect(calls).toBe(1);
    expect(await svc.lookupNamespacePrefix(ns)).toBe(null);
    expect(await svc.lookupNamespacePrefix(ns)).toBe(null);
    expect(fetchCalls.length).toBe(calls);
  });

  it('does not cache a rate-limited lookup as a miss', async () => {
    // A 429 says "ask later", not "unknown" — caching it would blank the label
    // for the whole negative TTL.
    vi.stubGlobal('fetch', vi.fn(async (url: string) => {
      fetchCalls.push(String(url));
      return { ok: false, status: 429, json: async () => ({}) } as Response;
    }));
    fetchCalls.length = 0;
    const svc = await freshService();
    const ns = 'http://example.org/later#';

    expect(await svc.lookupNamespacePrefix(ns)).toBe(null);
    expect(fetchCalls.length).toBe(1);

    // Once the limiter clears, the namespace resolves rather than staying blank.
    mockFetch({
      '/api/prefixes/reverse': { prefix: 'later', namespace: ns, source: 'lov' },
    });
    expect(await svc.lookupNamespacePrefix(ns)).toBe('later');
  });

  it('never contacts prefix.cc directly', async () => {
    mockFetch({});
    const svc = await freshService();
    await svc.lookupPrefix('zzz-unknown');
    await svc.lookupNamespacePrefix('http://example.org/unknown#');
    await svc.searchPrefixes('zzz');
    expect(fetchCalls.every((u) => !u.includes('prefix.cc'))).toBe(true);
  });
});
