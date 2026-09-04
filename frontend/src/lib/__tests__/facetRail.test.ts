/**
 * Regression cover for the facet rail's filter box.
 *
 * The bug: the `$:` statements that filtered the lists read the query only from
 * inside a `match()` helper, so Svelte never registered it as a dependency and
 * the box did nothing — you could type anything and the full list stayed put.
 * A pure unit test of the matcher would NOT have caught that, so this drives the
 * real component through a real input event.
 */
import { describe, it, expect, beforeAll } from 'vitest';
import { render, fireEvent } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';
import FacetRail from '../../components/browse/FacetRail.svelte';

beforeAll(() => {
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

const facets = {
  classes: [
    { iri: 'http://www.opengis.net/ont/sf#Point', count: 12 },
    { iri: 'https://data.example/def/Gebouwdeel', count: 7 },
  ],
  properties: [
    { iri: 'http://www.w3.org/2000/01/rdf-schema#label', count: 40 },
    { iri: 'http://www.opengis.net/ont/geosparql#asWKT', count: 5 },
  ],
  graphs: [{ iri: 'https://data.example/graph/instances', count: 99, role: 'instances', roleLabel: 'instances' }],
};

/** Visible item labels in the rail (classes + properties + graphs lists). */
const itemNames = (container: HTMLElement) =>
  [...container.querySelectorAll('.fitem-name')].map((n) => n.textContent?.trim());

async function type(container: HTMLElement, value: string) {
  const input = container.querySelector('.rail-search') as HTMLInputElement;
  await fireEvent.input(input, { target: { value } });
  return input;
}

describe('FacetRail filter box', () => {
  it('lists everything before a query is typed', () => {
    const { container } = render(FacetRail, { facets });
    const names = itemNames(container);
    expect(names).toContain('sf:Point');
    expect(names).toContain('rdfs:label');
    expect(names.length).toBeGreaterThanOrEqual(5);
  });

  it('narrows the lists as you type — the reactivity the bug broke', async () => {
    const { container } = render(FacetRail, { facets });
    await type(container, 'point');

    const names = itemNames(container);
    expect(names).toContain('sf:Point');
    expect(names).not.toContain('rdfs:label');
    expect(names).not.toContain('geo:asWKT');
  });

  it('matches on the full IRI, not just the shortened label', async () => {
    const { container } = render(FacetRail, { facets });
    await type(container, 'opengis');

    const names = itemNames(container);
    expect(names).toContain('sf:Point');
    expect(names).toContain('geo:asWKT');
    expect(names).not.toContain('rdfs:label');
  });

  it('ignores case and accents', async () => {
    const { container } = render(FacetRail, { facets });
    await type(container, 'GEBOUWDEÉL');
    // Accents in the QUERY must not defeat an unaccented label, and vice versa.
    expect(itemNames(container)).toEqual(['def:Gebouwdeel']);
  });

  it('shows the empty state when nothing matches', async () => {
    const { container } = render(FacetRail, { facets });
    await type(container, 'nothing-matches-this');

    expect(itemNames(container)).toEqual([]);
    expect(container.querySelectorAll('.fitem-empty').length).toBeGreaterThan(0);
  });

  it('restores the full list when the query is cleared', async () => {
    const { container } = render(FacetRail, { facets });
    await type(container, 'point');
    expect(itemNames(container)).not.toContain('rdfs:label');

    await type(container, '');
    expect(itemNames(container)).toContain('rdfs:label');
  });
});
