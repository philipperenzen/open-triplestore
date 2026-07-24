import { describe, it, expect, vi } from 'vitest';
import { createResourceCache, resourceCacheKey } from '../viewer/resourceCache';

/** A fetcher whose promises resolve on demand, so races are deterministic. */
function deferredFetcher() {
  const pending: { resolve: (v: unknown) => void; reject: (e: unknown) => void; signal?: AbortSignal }[] = [];
  const fn = vi.fn((_iri: string, _scope: unknown, init: { signal?: AbortSignal }) => {
    return new Promise((resolve, reject) => {
      pending.push({ resolve, reject, signal: init.signal });
    });
  });
  return { fn, pending };
}

describe('resourceCacheKey', () => {
  it('separates the same IRI resolved under different scopes', () => {
    const iri = 'https://example.org/a';
    expect(resourceCacheKey(iri, { dataset_id: 'one' })).not.toBe(resourceCacheKey(iri, { dataset_id: 'two' }));
    expect(resourceCacheKey(iri, { dataset_id: 'one' })).not.toBe(resourceCacheKey(iri, { graph: 'one' }));
    expect(resourceCacheKey(iri)).not.toBe(resourceCacheKey(iri, { dataset_id: 'one' }));
  });

  it('is stable for an equivalent scope', () => {
    const a = resourceCacheKey('urn:x', { dataset_id: 'd', graph: 'g' });
    const b = resourceCacheKey('urn:x', { graph: 'g', dataset_id: 'd' });
    expect(a).toBe(b);
  });

  it('cannot be forged by values that look like a delimiter', () => {
    // The fields are joined on NUL, which no IRI or id can contain.
    expect(resourceCacheKey('b', { dataset_id: 'a' })).not.toBe(resourceCacheKey('a/b'));
  });
});

describe('createResourceCache', () => {
  it('shares one request between concurrent callers for the same resource', async () => {
    const { fn, pending } = deferredFetcher();
    const cache = createResourceCache({ fetcher: fn });

    const a = cache.get('urn:a');
    const b = cache.get('urn:a');
    expect(fn).toHaveBeenCalledTimes(1);
    expect(cache.stats().inflight).toBe(1);

    pending[0].resolve({ ok: true });
    expect(await a).toEqual({ ok: true });
    expect(await b).toEqual({ ok: true });
    expect(cache.stats()).toEqual({ entries: 1, inflight: 0 });
  });

  it('serves a repeat lookup from cache while it is fresh', async () => {
    const { fn, pending } = deferredFetcher();
    let clock = 1000;
    const cache = createResourceCache({ fetcher: fn, ttlMs: 60_000, now: () => clock });

    const first = cache.get('urn:a');
    pending[0].resolve({ n: 1 });
    await first;

    clock += 59_000;
    expect(await cache.get('urn:a')).toEqual({ n: 1 });
    expect(fn).toHaveBeenCalledTimes(1);
  });

  it('refetches once the entry has gone stale', async () => {
    const { fn, pending } = deferredFetcher();
    let clock = 0;
    const cache = createResourceCache({ fetcher: fn, ttlMs: 60_000, now: () => clock });

    pending.length = 0;
    const first = cache.get('urn:a');
    pending[0].resolve({ n: 1 });
    await first;

    clock += 60_001;
    const second = cache.get('urn:a');
    expect(fn).toHaveBeenCalledTimes(2);
    pending[1].resolve({ n: 2 });
    expect(await second).toEqual({ n: 2 });
  });

  it('keys entries by scope, not by IRI alone', async () => {
    const { fn, pending } = deferredFetcher();
    const cache = createResourceCache({ fetcher: fn });

    const one = cache.get('urn:a', { dataset_id: 'one' });
    const two = cache.get('urn:a', { dataset_id: 'two' });
    expect(fn).toHaveBeenCalledTimes(2);

    pending[0].resolve({ from: 'one' });
    pending[1].resolve({ from: 'two' });
    expect(await one).toEqual({ from: 'one' });
    expect(await two).toEqual({ from: 'two' });
    expect(cache.stats().entries).toBe(2);
  });

  it('does not cache a rejection, and lets the next attempt retry', async () => {
    const { fn, pending } = deferredFetcher();
    const cache = createResourceCache({ fetcher: fn });

    const failing = cache.get('urn:a');
    pending[0].reject(new Error('boom'));
    await expect(failing).rejects.toThrow('boom');
    expect(cache.stats()).toEqual({ entries: 0, inflight: 0 });

    const retry = cache.get('urn:a');
    expect(fn).toHaveBeenCalledTimes(2);
    pending[1].resolve({ ok: true });
    expect(await retry).toEqual({ ok: true });
  });

  it('evicts least-recently-used entries past `max`', async () => {
    const { fn, pending } = deferredFetcher();
    const cache = createResourceCache({ fetcher: fn, max: 2 });

    for (const iri of ['urn:a', 'urn:b']) {
      const p = cache.get(iri);
      pending[pending.length - 1].resolve({ iri });
      await p;
    }
    // Touch 'urn:a' so 'urn:b' becomes the least recently used.
    await cache.get('urn:a');

    const c = cache.get('urn:c');
    pending[pending.length - 1].resolve({ iri: 'urn:c' });
    await c;

    expect(cache.stats().entries).toBe(2);
    const callsBefore = fn.mock.calls.length;
    await cache.get('urn:a'); // still cached
    expect(fn).toHaveBeenCalledTimes(callsBefore);
    cache.get('urn:b'); // evicted → refetched
    expect(fn).toHaveBeenCalledTimes(callsBefore + 1);
  });

  it('invalidates one IRI across every scope, and clears everything on demand', async () => {
    const { fn, pending } = deferredFetcher();
    const cache = createResourceCache({ fetcher: fn });

    const a1 = cache.get('urn:a', { dataset_id: 'one' });
    const a2 = cache.get('urn:a', { dataset_id: 'two' });
    const b = cache.get('urn:b');
    pending.forEach((p, i) => p.resolve({ i }));
    await Promise.all([a1, a2, b]);
    expect(cache.stats().entries).toBe(3);

    cache.invalidate('urn:a');
    expect(cache.stats().entries).toBe(1);
    cache.clear();
    expect(cache.stats().entries).toBe(0);
  });

  it('lets a caller abort without cancelling the request other callers still want', async () => {
    const { fn, pending } = deferredFetcher();
    const cache = createResourceCache({ fetcher: fn });
    const ctrl = new AbortController();

    const leaving = cache.get('urn:a', {}, { signal: ctrl.signal });
    const staying = cache.get('urn:a');
    expect(fn).toHaveBeenCalledTimes(1);

    ctrl.abort();
    await expect(leaving).rejects.toMatchObject({ name: 'AbortError' });
    expect(pending[0].signal?.aborted).toBe(false);

    pending[0].resolve({ ok: true });
    expect(await staying).toEqual({ ok: true });
  });

  it('cancels the underlying request once the last interested caller leaves', async () => {
    const { fn, pending } = deferredFetcher();
    const cache = createResourceCache({ fetcher: fn });
    const ctrl = new AbortController();

    const only = cache.get('urn:a', {}, { signal: ctrl.signal });
    ctrl.abort();
    await expect(only).rejects.toMatchObject({ name: 'AbortError' });
    expect(pending[0].signal?.aborted).toBe(true);
  });

  it('rejects immediately for a signal that is already aborted', async () => {
    const { fn } = deferredFetcher();
    const cache = createResourceCache({ fetcher: fn });
    const ctrl = new AbortController();
    ctrl.abort();

    await expect(cache.get('urn:a', {}, { signal: ctrl.signal })).rejects.toMatchObject({ name: 'AbortError' });
  });
});
