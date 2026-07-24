import { describe, it, expect, vi, beforeEach } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
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

  it('the map fast path requests located=true', async () => {
    const f = mockFetch();
    await getViewerFeed('viewer-3d-demo', null, { located: true });
    const url = lastUrl(f);
    expect(url).toContain('/api/datasets/viewer-3d-demo/viewer-feed?');
    expect(url).toContain('located=true');
  });

  // Guarding the helper alone is not enough: the Triples Browser's map tab was the
  // one caller that skipped the fast path, and it paid 3.9s / 3.0 MB to render the
  // same 25 elements the located subset returns in 5 ms / 11 KB.
  it('the Triples Browser map tab uses the located fast path', () => {
    // NOTE: resolve from `fileURLToPath(import.meta.url)` — Vite rewrites
    // `new URL(path, import.meta.url)` into an asset URL, which breaks under vitest.
    const here = dirname(fileURLToPath(import.meta.url));
    const src = readFileSync(resolve(here, '../../pages/TripleBrowser.svelte'), 'utf8');
    const call = src.match(/getViewerFeed\([^)]*located[^)]*\)/);
    expect(call, 'TripleBrowser must request the located subset for the map').toBeTruthy();
    // …and it must not have a bare full-feed call as its PRIMARY map fetch.
    expect(src).toContain('{ located: true }');
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
