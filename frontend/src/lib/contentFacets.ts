/**
 * What a registry version holds, as the API counts it (`facets` on an entry
 * and on a version): classes, properties, concepts, individuals, shapes and
 * rules. The registry page and a version's row show the non-zero ones as
 * chips, so an entry of 140 concepts says so instead of "0 classes".
 */

/** The `facets` object of `GET /api/models` and `GET /api/models/{id}/versions`. */
export interface ContentFacets {
  classes: number;
  properties: number;
  concepts: number;
  concept_schemes: number;
  individuals: number;
  shapes: number;
  rules: number;
  /** Triples the counts were made over. */
  triples: number;
  /** The content was too large to scan: no counts are given. */
  capped?: boolean;
}

export type FacetKey = 'classes' | 'properties' | 'concepts' | 'individuals' | 'shapes' | 'rules';

/** The order the chips are shown in. */
export const FACET_ORDER: readonly FacetKey[] = [
  'classes',
  'properties',
  'concepts',
  'individuals',
  'shapes',
  'rules',
];

export interface FacetChip {
  key: FacetKey;
  count: number;
}

/**
 * The facets worth showing: the non-zero ones, in {@link FACET_ORDER}. Nothing
 * for an entry that has not been counted yet (`null`) or has nothing in it.
 */
export function facetChips(facets: Partial<ContentFacets> | null | undefined): FacetChip[] {
  if (!facets || facets.capped) return [];
  const chips: FacetChip[] = [];
  for (const key of FACET_ORDER) {
    const count = Number(facets[key]) || 0;
    if (count > 0) chips.push({ key, count });
  }
  return chips;
}
