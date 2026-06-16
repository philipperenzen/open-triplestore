import { describe, it, expect, vi, beforeEach } from 'vitest';
import { getViewerFeed } from '../api.ts';

// Speed/responsiveness regression guard for the dataset 3D/map explorer.
//
// The explorer paints the map fast by first fetching ONLY the coordinate-bearing
// elements (`?located=true`) — the small subset the map renders — and loading the
// full structure feed (thousands of IFC sub-elements) in the background. If the
// `located` fast path is dropped, or the param is sent in a form the backend
// rejects (it parses a bool, so `located=1` is a 400 — only `true` works), the
// map silently falls back to waiting on the whole-building feed and "takes
// forever" again. These tests pin the request contract that keeps it fast.

function mockFetch() {
  const fetchMock = vi.fn().mockResolvedValue({
    ok: true,
    status: 200,
    headers: { get: () => 'application/json' },
    json: async () => ({ dataset_id: 'd', count: 0, elements: [] }),
  });
  // @ts-expect-error jsdom global
  global.fetch = fetchMock;
  return fetchMock;
}

function lastUrl(fetchMock: ReturnType<typeof vi.fn>): string {
  return String(fetchMock.mock.calls.at(-1)?.[0] ?? '');
}

describe('getViewerFeed request contract (map load speed)', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it('the full feed (structure tree) sends no located param', async () => {
    const f = mockFetch();
    await getViewerFeed('viewer-3d-demo');
    const url = lastUrl(f);
    expect(url).toBe('/api/datasets/viewer-3d-demo/viewer-feed');
    expect(url).not.toContain('located');
  });

  it('the map fast path requests located=true (NOT located=1, which 400s)', async () => {
    const f = mockFetch();
    await getViewerFeed('viewer-3d-demo', null, { located: true });
    const url = lastUrl(f);
    expect(url).toContain('/api/datasets/viewer-3d-demo/viewer-feed?');
    expect(url).toContain('located=true');
    // The backend deserialises this as a bool; '1' is rejected as a 400.
    expect(url).not.toContain('located=1');
  });

  it('encodes the dataset id and keeps root + located together', async () => {
    const f = mockFetch();
    await getViewerFeed('a b/c', 'http://ex.org/Site', { located: true });
    const url = lastUrl(f);
    expect(url).toContain('/api/datasets/a%20b%2Fc/viewer-feed?');
    expect(url).toContain('root=http%3A%2F%2Fex.org%2FSite');
    expect(url).toContain('located=true');
  });
});
