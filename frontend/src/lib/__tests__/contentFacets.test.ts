/**
 * The chips that say what a registry entry or version holds.
 */
import { describe, it, expect } from 'vitest';
import { facetChips, type ContentFacets } from '../contentFacets';

const NONE: ContentFacets = {
  classes: 0,
  properties: 0,
  concepts: 0,
  concept_schemes: 0,
  individuals: 0,
  shapes: 0,
  rules: 0,
  triples: 0,
};

describe('facetChips', () => {
  it('shows nothing for an entry that has not been counted', () => {
    expect(facetChips(null)).toEqual([]);
    expect(facetChips(undefined)).toEqual([]);
  });

  it('shows nothing for content with nothing in it', () => {
    expect(facetChips(NONE)).toEqual([]);
  });

  it('shows only the non-zero facets, in a fixed order', () => {
    expect(
      facetChips({ ...NONE, shapes: 4, classes: 12, concepts: 3, properties: 30 }),
    ).toEqual([
      { key: 'classes', count: 12 },
      { key: 'properties', count: 30 },
      { key: 'concepts', count: 3 },
      { key: 'shapes', count: 4 },
    ]);
  });

  it('says concepts for a scheme with no classes, not "0 classes"', () => {
    expect(facetChips({ ...NONE, concepts: 143, concept_schemes: 1 })).toEqual([
      { key: 'concepts', count: 143 },
    ]);
  });

  it('says individuals for a profile that declares no class or property', () => {
    expect(facetChips({ ...NONE, individuals: 74 })).toEqual([{ key: 'individuals', count: 74 }]);
  });

  it('gives no counts for content too large to scan', () => {
    expect(facetChips({ ...NONE, classes: 5, capped: true })).toEqual([]);
  });
});
