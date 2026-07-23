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

  it('never contacts prefix.cc directly', async () => {
    mockFetch({});
    const svc = await freshService();
    await svc.lookupPrefix('zzz-unknown');
    await svc.lookupNamespacePrefix('http://example.org/unknown#');
    await svc.searchPrefixes('zzz');
    expect(fetchCalls.every((u) => !u.includes('prefix.cc'))).toBe(true);
  });
});
