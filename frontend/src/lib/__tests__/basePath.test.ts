/**
 * Sub-path deploys (OTS_BASE_PATH): the UI published under `/ots/` has to
 * match its routes on the path *below* the prefix, put the prefix back on every
 * link, history entry and fetch, and find /embed pages and config.json under it.
 *
 * The bug this covers: only Vite knew about the base, so the router matched the
 * raw `/ots/datasets/x` against `/datasets/:id`, found nothing, and the page was
 * an empty shell; links and fetches went to the host root.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { get } from 'svelte/store';
import {
  normaliseBase,
  withBase,
  stripBase,
  withBaseUrl,
  isEmbedPath,
  absoluteUrl,
} from '../basePath';

describe('basePath helpers', () => {
  it('normalises Vite BASE_URL into a prefix without a trailing slash', () => {
    expect(normaliseBase('/')).toBe('');
    expect(normaliseBase(undefined)).toBe('');
    expect(normaliseBase('')).toBe('');
    expect(normaliseBase('./')).toBe('');
    expect(normaliseBase('https://cdn.example.org/ots/')).toBe('');
    expect(normaliseBase('//cdn.example.org/ots/')).toBe('');
    expect(normaliseBase('/ots/')).toBe('/ots');
    expect(normaliseBase('/tools/ots')).toBe('/tools/ots');
  });

  it('is a no-op at the root', () => {
    expect(withBase('/datasets/x', '')).toBe('/datasets/x');
    expect(stripBase('/datasets/x', '')).toBe('/datasets/x');
    expect(stripBase('', '')).toBe('/');
    expect(withBaseUrl('http://h/api/x', 'http://h', '')).toBe('http://h/api/x');
  });

  it('prefixes root-absolute app paths only', () => {
    const b = '/ots';
    expect(withBase('/', b)).toBe('/ots/');
    expect(withBase('/datasets/x?tab=y#z', b)).toBe('/ots/datasets/x?tab=y#z');
    expect(withBase('/api/auth/me', b)).toBe('/ots/api/auth/me');
    // Absolute, protocol-relative, relative, query/hash-only and data URLs stay.
    for (const u of ['https://example.org/x', '//cdn.example.org/x', 'rel/x', '?q=1', '#frag', 'data:text/plain,x', 'blob:http://h/1']) {
      expect(withBase(u, b)).toBe(u);
    }
  });

  it('does not prefix twice', () => {
    const b = '/ots';
    expect(withBase('/ots', b)).toBe('/ots');
    expect(withBase('/ots/', b)).toBe('/ots/');
    expect(withBase('/ots/datasets/x', b)).toBe('/ots/datasets/x');
    expect(withBase('/ots?x=1', b)).toBe('/ots?x=1');
    expect(withBase(withBase('/sparql', b), b)).toBe('/ots/sparql');
    // Same leading letters, different segment: still an app path.
    expect(withBase('/otsx/a', b)).toBe('/ots/otsx/a');
  });

  it('strips the base off a browser pathname', () => {
    const b = '/ots';
    expect(stripBase('/ots', b)).toBe('/');
    expect(stripBase('/ots/', b)).toBe('/');
    expect(stripBase('/ots/datasets/x', b)).toBe('/datasets/x');
    // Outside the base: left alone, so no route matches it.
    expect(stripBase('/datasets/x', b)).toBe('/datasets/x');
    expect(stripBase('/otsx/a', b)).toBe('/otsx/a');
  });

  it('rebases same-origin URLs a library resolved against the root', () => {
    const b = '/ots';
    expect(withBaseUrl('http://h/api/datasets/x/sparql?q=1', 'http://h', b)).toBe('http://h/ots/api/datasets/x/sparql?q=1');
    expect(withBaseUrl('http://h/ots/api/x', 'http://h', b)).toBe('http://h/ots/api/x');
    expect(withBaseUrl('https://other.example/api/x', 'http://h', b)).toBe('https://other.example/api/x');
  });

  it('detects embed pages below the base', () => {
    expect(isEmbedPath('/embed/map/ds', '')).toBe(true);
    expect(isEmbedPath('/ots/embed/map/ds', '/ots')).toBe(true);
    expect(isEmbedPath('/ots/embed', '/ots')).toBe(true);
    expect(isEmbedPath('/ots/embedded', '/ots')).toBe(false);
    expect(isEmbedPath('/ots/datasets/x', '/ots')).toBe(false);
    // Root-anchored `/embed` is not this app's embed page under a base.
    expect(isEmbedPath('/embed/map/ds', '/ots')).toBe(false);
  });

  it('builds copyable absolute URLs with the base', () => {
    expect(absoluteUrl('/api/models/m/versions', 'https://example.org', '/ots')).toBe('https://example.org/ots/api/models/m/versions');
    expect(absoluteUrl('/sparql', 'https://example.org', '')).toBe('https://example.org/sparql');
  });
});

// The router, Link and the fetches read the base from import.meta.env.BASE_URL
// when their module loads, so each case below stubs it and imports afresh —
// the testing library included, so it renders with the same fresh Svelte
// runtime the harness component was compiled against.
async function freshDom() {
  return import('@testing-library/svelte');
}
describe('router under a sub-path deploy (BASE_URL=/ots/)', () => {
  beforeEach(() => {
    vi.stubEnv('BASE_URL', '/ots/');
    vi.resetModules();
  });

  afterEach(async () => {
    (await freshDom()).cleanup();
    vi.unstubAllEnvs();
    vi.restoreAllMocks();
    vi.resetModules();
    window.history.replaceState({}, '', '/');
  });

  it('reads the base from import.meta.env', async () => {
    const { BASE_PATH } = await import('../basePath');
    expect(BASE_PATH).toBe('/ots');
  });

  it('exposes the app path to routes and prefixes navigation', async () => {
    window.history.replaceState({}, '', '/ots/datasets/abc?tab=versions#v2');
    const { location, navigate, syncLocation } = await import('../locationStore');
    syncLocation();
    expect(get(location)).toMatchObject({
      pathname: '/datasets/abc',
      search: '?tab=versions',
      hash: '#v2',
      href: '/datasets/abc?tab=versions#v2',
    });

    navigate('/sparql?query=x');
    expect(window.location.pathname).toBe('/ots/sparql');
    expect(window.location.search).toBe('?query=x');
    expect(get(location).pathname).toBe('/sparql');

    // A path that already carries the base (e.g. built from window.location)
    // is not prefixed twice.
    navigate('/ots/datasets/abc', { replace: true });
    expect(window.location.pathname).toBe('/ots/datasets/abc');
    expect(get(location).pathname).toBe('/datasets/abc');

    navigate('/');
    expect(window.location.pathname).toBe('/ots/');
    expect(get(location).pathname).toBe('/');
  });

  it('renders the route of a deep link and keeps Link clicks under the base', async () => {
    window.history.replaceState({}, '', '/ots/datasets/abc');
    const { syncLocation } = await import('../locationStore');
    syncLocation();
    const { render, screen, fireEvent } = await freshDom();
    const { default: Harness } = await import('./fixtures/RouterHarness.svelte');
    render(Harness);

    expect(screen.getByTestId('page').textContent).toBe('dataset abc');

    const sparql = screen.getByText('sparql link');
    expect(sparql.getAttribute('href')).toBe('/ots/sparql?query=ASK%7B%7D');
    expect(screen.getByText('dataset link').getAttribute('href')).toBe('/ots/datasets/abc');

    await fireEvent.click(sparql);
    expect(window.location.pathname).toBe('/ots/sparql');
    expect(screen.getByTestId('page').textContent).toBe('sparql');
  });

  it('renders the home route at the bare base, with or without its slash', async () => {
    for (const entry of ['/ots/', '/ots']) {
      window.history.replaceState({}, '', entry);
      vi.resetModules();
      const { syncLocation } = await import('../locationStore');
      syncLocation();
      const { render, screen, cleanup } = await freshDom();
      const { default: Harness } = await import('./fixtures/RouterHarness.svelte');
      render(Harness);
      expect(screen.getByTestId('page').textContent).toBe('home');
      cleanup();
    }
  });

  it('fetches config.json and the API below the base', async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      headers: { get: () => 'application/json' },
      json: async () => ({}),
      text: async () => '{}',
    });
    global.fetch = fetchMock;

    const { loadRuntimeConfig } = await import('../runtimeConfig');
    loadRuntimeConfig();
    expect(fetchMock.mock.calls[0][0]).toBe('/ots/config.json');

    const api = await import('../api');
    await api.getHealth().catch(() => {});
    const urls = fetchMock.mock.calls.map((c) => String(c[0]));
    expect(urls.some((u) => /^\/ots\/(health|api\/)/.test(u))).toBe(true);
    expect(urls.every((u) => u.startsWith('/ots/'))).toBe(true);
    expect(api.getDatasetImageUrl('d1')).toBe('/ots/api/datasets/d1/image');
    expect(api.getDatasetVersionDataUrl('d1', 'v1')).toBe('/ots/api/datasets/d1/versions/v1/data?format=trig');
  });
});
