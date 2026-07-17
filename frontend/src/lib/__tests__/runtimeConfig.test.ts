import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { get } from 'svelte/store';

function mockFetch(response: { ok: boolean; contentType: string; body?: unknown }) {
  const fetchMock = vi.fn().mockResolvedValue({
    ok: response.ok,
    headers: { get: (h: string) => (h.toLowerCase() === 'content-type' ? response.contentType : null) },
    json: async () => response.body,
  });
  // @ts-expect-error jsdom global
  global.fetch = fetchMock;
  return fetchMock;
}

// loadRuntimeConfig() guards `started` with module-level state, so each test
// needs a fresh module instance (and a fresh DOM) to observe its own fetch.
async function freshModule() {
  vi.resetModules();
  return import('../runtimeConfig.js');
}

describe('runtimeConfig', () => {
  beforeEach(() => {
    // Order matters: replacing head.innerHTML AFTER setting document.title
    // would discard the <title> element jsdom auto-creates for it.
    document.head.innerHTML = '<link rel="icon" href="/favicon.svg" />';
    document.title = 'Open Triplestore';
    document.documentElement.style.removeProperty('--brand-600');
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.resetModules();
  });

  it('applies branding from a real /config.json response', async () => {
    mockFetch({
      ok: true,
      contentType: 'application/json; charset=utf-8',
      body: { branding: { title: 'Acme Graph', logoUrl: '/acme-logo.svg', accent: '#7a2fe0' } },
    });
    const { loadRuntimeConfig, runtimeBranding } = await freshModule();
    loadRuntimeConfig();
    // The fetch chain is a microtask queue — flush it.
    await new Promise((r) => setTimeout(r, 0));
    await new Promise((r) => setTimeout(r, 0));

    expect(get(runtimeBranding)).toEqual({
      title: 'Acme Graph',
      logoUrl: '/acme-logo.svg',
      accent: '#7a2fe0',
    });
    expect(document.title).toBe('Acme Graph');
    expect(document.querySelector('link[rel="icon"]')?.getAttribute('href')).toBe('/acme-logo.svg');
    expect(document.documentElement.style.getPropertyValue('--brand-600')).toBe('#7a2fe0');
  });

  it('forwards the services map to serviceRegistry', async () => {
    mockFetch({
      ok: true,
      contentType: 'application/json',
      body: { services: { triplestore: 'https://runtime-cfg.example.com' } },
    });
    const { loadRuntimeConfig } = await freshModule();
    const { getService } = await import('../serviceRegistry.js');
    loadRuntimeConfig();
    await new Promise((r) => setTimeout(r, 0));
    await new Promise((r) => setTimeout(r, 0));

    expect(getService('triplestore')).toBe('https://runtime-cfg.example.com');
  });

  it('treats the SPA-fallback text/html response (no config.json present) as absent', async () => {
    // This app's own ServeDir + SPA fallback returns index.html (200, text/html)
    // for any unmatched path, including a missing config.json — must not be
    // mistaken for real JSON config.
    mockFetch({ ok: true, contentType: 'text/html; charset=utf-8' });
    const { loadRuntimeConfig, runtimeBranding } = await freshModule();
    loadRuntimeConfig();
    await new Promise((r) => setTimeout(r, 0));
    await new Promise((r) => setTimeout(r, 0));

    expect(get(runtimeBranding)).toEqual({ title: 'Open Triplestore', logoUrl: null, accent: null });
    expect(document.title).toBe('Open Triplestore');
  });

  it('is a no-op when the fetch itself fails (e.g. offline)', async () => {
    // @ts-expect-error jsdom global
    global.fetch = vi.fn().mockRejectedValue(new Error('network down'));
    const { loadRuntimeConfig, runtimeBranding } = await freshModule();
    expect(() => loadRuntimeConfig()).not.toThrow();
    await new Promise((r) => setTimeout(r, 0));
    await new Promise((r) => setTimeout(r, 0));

    expect(get(runtimeBranding)).toEqual({ title: 'Open Triplestore', logoUrl: null, accent: null });
  });

  it('only fetches once even if called twice', async () => {
    const f = mockFetch({ ok: true, contentType: 'application/json', body: {} });
    const { loadRuntimeConfig } = await freshModule();
    loadRuntimeConfig();
    loadRuntimeConfig();
    await new Promise((r) => setTimeout(r, 0));
    expect(f).toHaveBeenCalledTimes(1);
  });
});
