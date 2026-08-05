/**
 * Shared filter-as-you-type matching used by the in-page search boxes.
 */
import { describe, it, expect } from 'vitest';
import { matchesQuery, filterByQuery, queryTokens } from '../searchMatch';

describe('queryTokens', () => {
  it('splits on whitespace and drops empties', () => {
    expect(queryTokens('  geo   point ')).toEqual(['geo', 'point']);
    expect(queryTokens('')).toEqual([]);
    expect(queryTokens('   ')).toEqual([]);
  });

  it('folds case and accents', () => {
    expect(queryTokens('Gebouwdeél')).toEqual(['gebouwdeel']);
  });
});

describe('matchesQuery', () => {
  it('matches everything for an empty or blank query', () => {
    expect(matchesQuery('', 'anything')).toBe(true);
    expect(matchesQuery('   ', 'anything')).toBe(true);
    // …even with no fields at all.
    expect(matchesQuery('')).toBe(true);
  });

  it('is case-insensitive', () => {
    expect(matchesQuery('dwars', 'Dwarsprofiel')).toBe(true);
    expect(matchesQuery('DWARS', 'dwarsprofiel')).toBe(true);
  });

  it('is diacritic-insensitive both ways', () => {
    expect(matchesQuery('gebouwdeel', 'Gebouwdeél')).toBe(true);
    expect(matchesQuery('gebouwdeél', 'Gebouwdeel')).toBe(true);
  });

  it('requires every token, in any order, across any field', () => {
    expect(matchesQuery('geo point', 'Point geometry', 'http://ex/geo')).toBe(true);
    expect(matchesQuery('point geo', 'Point geometry', 'http://ex/geo')).toBe(true);
    expect(matchesQuery('geo missing', 'Point geometry', 'http://ex/geo')).toBe(false);
  });

  it('ignores empty fields and fails when there is nothing to match', () => {
    expect(matchesQuery('x', null, undefined, '')).toBe(false);
    expect(matchesQuery('geo', '', 'http://ex/geo')).toBe(true);
  });

  it('matches against an IRI as well as its label', () => {
    const iri = 'http://www.opengis.net/ont/geosparql#asWKT';
    expect(matchesQuery('geosparql', 'asWKT', iri)).toBe(true);
    expect(matchesQuery('aswkt', 'asWKT', iri)).toBe(true);
  });
});

describe('filterByQuery', () => {
  const items = [
    { iri: 'http://ex/Wall', label: 'Wall' },
    { iri: 'http://ex/Door', label: 'Deur' },
    { iri: 'http://other/Window', label: 'Raam' },
  ];
  const fields = (i: (typeof items)[number]) => [i.label, i.iri];

  it('returns the SAME array for an empty query (cheap identity check)', () => {
    expect(filterByQuery(items, '', fields)).toBe(items);
    expect(filterByQuery(items, '  ', fields)).toBe(items);
  });

  it('filters on label or IRI', () => {
    expect(filterByQuery(items, 'deur', fields).map((i) => i.label)).toEqual(['Deur']);
    expect(filterByQuery(items, 'ex/', fields).map((i) => i.label)).toEqual(['Wall', 'Deur']);
  });

  it('returns an empty array when nothing matches', () => {
    expect(filterByQuery(items, 'nothing here', fields)).toEqual([]);
  });

  it('preserves input order', () => {
    expect(filterByQuery(items, 'http', fields).map((i) => i.label)).toEqual(['Wall', 'Deur', 'Raam']);
  });

  it('tolerates missing fields on an item', () => {
    const sparse = [{ iri: 'http://ex/A', label: undefined }];
    expect(filterByQuery(sparse, 'ex', (i) => [i.label, i.iri])).toHaveLength(1);
    expect(filterByQuery(sparse, 'zzz', (i) => [i.label, i.iri])).toHaveLength(0);
  });
});
