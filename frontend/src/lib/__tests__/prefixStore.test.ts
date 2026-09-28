import { describe, it, expect, vi, afterEach } from 'vitest';
import { get } from 'svelte/store';
import { shortenIRI, expandPrefix, loadPrefixCcPrefixes, prefixesVersion } from '../rdf-utils.js';

// `loadPrefixCcPrefixes` memoises the one load for the life of the module, so
// observing what the fill accepts needs a module nothing else has warmed —
// which is why this lives in its own file rather than beside the other
// rdf-utils tests.
describe('the prefix store keeps one label bound to one namespace', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('refuses a snapshot label the well-known table binds elsewhere, and signals when warm', async () => {
    expect(get(prefixesVersion)).toBe(0);
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok: true,
        json: async () => ({
          // `sh` is already bound by COMMON_PREFIXES to the SHACL namespace.
          // Taking this would make one label mean two namespaces, so a
          // serializer could declare one and write the other.
          sh: 'http://example.org/other/',
          qudt: 'http://qudt.org/schema/qudt/',
        }),
      }),
    );
    await loadPrefixCcPrefixes();

    expect(shortenIRI('http://www.w3.org/ns/shacl#NodeShape')).toBe('sh:NodeShape');
    expect(shortenIRI('http://example.org/other/Thing')).not.toBe('sh:Thing');
    // A label that collides with nothing is taken.
    expect(shortenIRI('http://qudt.org/schema/qudt/Unit')).toBe('qudt:Unit');
    // For every label both tiers agree on, shortening and expanding are inverses.
    for (const full of [
      'http://www.w3.org/ns/shacl#NodeShape',
      'http://qudt.org/schema/qudt/Unit',
    ]) {
      expect(expandPrefix(shortenIRI(full))).toBe(full);
    }
    // Subscribers are told once the store is warm; the load is memoised, so a
    // second call is a no-op rather than a second notification.
    expect(get(prefixesVersion)).toBe(1);
    await loadPrefixCcPrefixes();
    expect(get(prefixesVersion)).toBe(1);
  });
});
