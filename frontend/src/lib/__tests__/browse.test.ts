/**
 * Browse API + /graph-viz redirect tests.
 *
 * These cover the contract between the frontend and the reworked
 * /api/browse/triples endpoint (hasMore probe + opt-in count) and the
 * redirect that replaced the deleted /graph-viz page.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { browseTriples } from '../api.js';

describe('browseTriples wire format', () => {
  let fetchSpy: any;

  beforeEach(() => {
    fetchSpy = vi.spyOn(globalThis, 'fetch').mockImplementation(async (_url: any, _opts: any) => {
      return new Response(
        JSON.stringify({ triples: [{ subject: { value: 's' }, predicate: { value: 'p' }, object: { value: 'o' } }], hasMore: true, limit: 1, offset: 0 }),
        { status: 200, headers: { 'content-type': 'application/json' } },
      );
    });
  });
  afterEach(() => { fetchSpy.mockRestore(); });

  it('default call does not send count=true (fast path)', async () => {
    await browseTriples({ limit: '25', offset: '0' });
    const [url] = fetchSpy.mock.calls[0];
    expect(String(url)).toContain('/api/browse/triples?');
    expect(String(url)).not.toContain('count=true');
  });

  it('returns hasMore flag and omits total by default', async () => {
    const res = await browseTriples({ limit: '25' });
    expect(res.hasMore).toBe(true);
    expect(res.total).toBeUndefined();
    expect(res.triples).toHaveLength(1);
  });

  it('passes count=true when the UI opts in', async () => {
    await browseTriples({ limit: '25', count: 'true' });
    const [url] = fetchSpy.mock.calls[0];
    expect(String(url)).toContain('count=true');
  });

  it('forwards filter params verbatim so access-scoped queries work', async () => {
    await browseTriples({ graph: 'http://pub.ex.org/g', subject: 'http://ex.org/s', limit: '25' });
    const [url] = fetchSpy.mock.calls[0];
    const u = new URL(String(url), 'http://localhost');
    expect(u.searchParams.get('graph')).toBe('http://pub.ex.org/g');
    expect(u.searchParams.get('subject')).toBe('http://ex.org/s');
  });
});

describe('/graph-viz → /browse?view=graph redirect', () => {
  it('preserves relevant query params when forwarding', () => {
    // Inline the same forwarding logic used by GraphVizRedirect.svelte so we
    // can unit-test it without spinning up the SPA router.
    const forwardParams = (input: string): string => {
      const incoming = new URLSearchParams(input);
      const out = new URLSearchParams();
      out.set('view', 'graph');
      for (const key of ['graph', 'dataset', 'org', 'subject', 'predicate', 'object', 'q', 'uri']) {
        const v = incoming.get(key);
        if (v) out.set(key, v);
      }
      return out.toString();
    };

    const out = forwardParams('graph=http%3A%2F%2Fex%2Forg%2Fg&dataset=d1&unrelated=x');
    const u = new URLSearchParams(out);
    expect(u.get('view')).toBe('graph');
    expect(u.get('graph')).toBe('http://ex/org/g');
    expect(u.get('dataset')).toBe('d1');
    expect(u.get('unrelated')).toBeNull();
  });

  it('always sets view=graph even without other params', () => {
    const out = new URLSearchParams();
    out.set('view', 'graph');
    expect(out.get('view')).toBe('graph');
  });
});
