/**
 * Triple Browser scope helpers.
 *
 * The page itself is far too large to mount, so the two pieces of logic that
 * actually went wrong live in `browseScope.ts` and are tested here: turning the
 * scope selection into request parameters (every selected dataset AND org has to
 * reach the backend) and remembering that selection across a browser restart.
 */
import { describe, it, expect, vi, afterEach } from 'vitest';
import { browseFacets, browseResource, browseTriples } from '../api.js';
import {
  buildScopeParams,
  isPinnedVersion,
  loadScope,
  saveScope,
  reconcileScope,
  SCOPE_STORAGE_KEY,
} from '../browseScope.js';

const ds = (id: string) => ({ type: 'dataset' as const, id, name: id });
const org = (id: string) => ({ type: 'org' as const, id, name: id });

/** Minimal in-memory Storage stand-in; `fail` makes every access throw. */
function fakeStorage(fail = false) {
  const map = new Map<string, string>();
  return {
    map,
    getItem: (k: string) => { if (fail) throw new Error('denied'); return map.has(k) ? map.get(k)! : null; },
    setItem: (k: string, v: string) => { if (fail) throw new Error('denied'); map.set(k, v); },
    removeItem: (k: string) => { if (fail) throw new Error('denied'); map.delete(k); },
  };
}

describe('buildScopeParams', () => {
  it('keeps the single-dataset shape the API has always been sent', () => {
    const p = buildScopeParams({ scopeItems: [ds('d1')], datasetIds: ['d1'] });
    expect(p).toEqual({ dataset_id: 'd1' });
  });

  it('keeps the single-organisation shape', () => {
    const p = buildScopeParams({ scopeItems: [org('o1')], datasetIds: ['d1', 'd2'] });
    expect(p).toEqual({ org_id: 'o1' });
  });

  it('sends every selected dataset when several are picked', () => {
    const p = buildScopeParams({ scopeItems: [ds('d1'), ds('d2')], datasetIds: ['d1', 'd2'] });
    expect(p.dataset_ids).toBe('d1,d2');
    expect(p.org_ids).toBeUndefined();
  });

  it('sends datasets AND organisations together instead of dropping the org', () => {
    const p = buildScopeParams({
      scopeItems: [ds('d1'), ds('d2'), org('o1')],
      datasetIds: ['d1', 'd2', 'd9'],
    });
    expect(p.dataset_ids).toBe('d1,d2');
    expect(p.org_ids).toBe('o1');
    // The legacy single-org parameter still carries the first org so an older
    // backend degrades to today's behaviour instead of losing the scope.
    expect(p.org_id).toBe('o1');
  });

  it('sends a single dataset combined with an org (which used to send nothing)', () => {
    const p = buildScopeParams({ scopeItems: [ds('d1'), org('o1')], datasetIds: ['d1', 'd7'] });
    expect(p.dataset_ids).toBe('d1');
    expect(p.org_ids).toBe('o1');
  });

  it('sends every selected organisation, not just the first', () => {
    const p = buildScopeParams({ scopeItems: [org('o1'), org('o2')], datasetIds: [] });
    expect(p.org_ids).toBe('o1,o2');
    expect(p.org_id).toBe('o1');
    expect(p.dataset_ids).toBeUndefined();
  });

  it('is empty when nothing is in scope', () => {
    expect(buildScopeParams({ scopeItems: [], datasetIds: [] })).toEqual({});
  });

  it('pins only the datasets whose version is not a live sentinel', () => {
    const p = buildScopeParams({
      scopeItems: [ds('d1'), ds('d2')],
      datasetIds: ['d1', 'd2', 'd3'],
      versions: { d1: '1.2.0', d2: 'live', d3: '' },
    });
    expect(p.versions).toBe('d1:1.2.0');
  });

  it('treats the live sentinels as "not pinned"', () => {
    expect(isPinnedVersion('')).toBe(false);
    expect(isPinnedVersion('live')).toBe(false);
    expect(isPinnedVersion('latest')).toBe(false);
    expect(isPinnedVersion('current')).toBe(false);
    expect(isPinnedVersion('2.0.0')).toBe(true);
  });
});

describe('the whole scope reaches the wire', () => {
  let fetchSpy: any;
  afterEach(() => { fetchSpy?.mockRestore(); });

  const spy = () => {
    fetchSpy = vi.spyOn(globalThis, 'fetch').mockImplementation(async () =>
      new Response('{}', { status: 200, headers: { 'content-type': 'application/json' } }));
    return () => new URL(String(fetchSpy.mock.calls.at(-1)![0]), 'http://localhost').searchParams;
  };

  it('sends the same scope to the rows, the count and the facet rail', async () => {
    const lastQuery = spy();
    const params = buildScopeParams({
      scopeItems: [ds('d1'), org('o1')],
      datasetIds: ['d1', 'd7'],
    });
    await browseTriples({ ...params, limit: '25' });
    const rows = lastQuery();
    await browseTriples({ ...params, limit: '25', count: 'true' });
    const count = lastQuery();
    await browseFacets(params);
    const facets = lastQuery();
    for (const q of [rows, count, facets]) {
      expect(q.get('dataset_ids')).toBe('d1');
      expect(q.get('org_ids')).toBe('o1');
    }
  });

  it('forwards org_ids on a resource expansion instead of dropping it', async () => {
    const lastQuery = spy();
    await browseResource('http://ex.org/s', buildScopeParams({
      scopeItems: [ds('d1'), org('o1'), org('o2')],
      datasetIds: ['d1'],
    }));
    expect(lastQuery().get('org_ids')).toBe('o1,o2');
  });
});

describe('scope persistence', () => {
  it('round-trips the selection and its version pins', () => {
    const s = fakeStorage();
    saveScope([ds('d1'), org('o1')], { d1: '1.0.0' }, s);
    expect(loadScope(s)).toEqual({ items: [ds('d1'), org('o1')], versions: { d1: '1.0.0' } });
  });

  it('remembers an empty scope as a choice rather than as "nothing saved"', () => {
    const s = fakeStorage();
    saveScope([], {}, s);
    expect(loadScope(s)).toEqual({ items: [], versions: {} });
  });

  it('returns null when nothing was ever saved', () => {
    expect(loadScope(fakeStorage())).toBeNull();
  });

  it('ignores a snapshot written by an older build', () => {
    const s = fakeStorage();
    s.setItem(SCOPE_STORAGE_KEY, JSON.stringify({ v: 0, items: [ds('d1')] }));
    expect(loadScope(s)).toBeNull();
  });

  it('ignores a corrupt snapshot', () => {
    const s = fakeStorage();
    s.setItem(SCOPE_STORAGE_KEY, 'not json');
    expect(loadScope(s)).toBeNull();
  });

  it('survives a storage that throws (private windows)', () => {
    const s = fakeStorage(true);
    expect(() => saveScope([ds('d1')], {}, s)).not.toThrow();
    expect(loadScope(s)).toBeNull();
  });

  it('survives having no storage at all', () => {
    expect(() => saveScope([ds('d1')], {}, null)).not.toThrow();
    expect(loadScope(null)).toBeNull();
  });
});

describe('reconcileScope', () => {
  const inventory = {
    datasets: [{ id: 'd1' }, { id: 'd2' }],
    orgs: [{ id: 'o1' }],
  };

  it('drops a dataset that no longer exists or is no longer visible', () => {
    const out = reconcileScope(
      { items: [ds('d1'), ds('gone')], versions: { d1: '1.0.0', gone: '2.0.0' } },
      inventory,
    );
    expect(out.items).toEqual([ds('d1')]);
    expect(out.versions).toEqual({ d1: '1.0.0' });
  });

  it('drops an organisation that is no longer visible', () => {
    const out = reconcileScope({ items: [org('o1'), org('o9')], versions: {} }, inventory);
    expect(out.items).toEqual([org('o1')]);
  });

  it('compares ids as strings so a numeric inventory id still matches', () => {
    const out = reconcileScope(
      { items: [org('7')], versions: {} },
      { datasets: [], orgs: [{ id: 7 as unknown as string }] },
    );
    expect(out.items).toEqual([org('7')]);
  });

  it('keeps an empty scope empty', () => {
    expect(reconcileScope({ items: [], versions: {} }, inventory)).toEqual({ items: [], versions: {} });
  });
});
