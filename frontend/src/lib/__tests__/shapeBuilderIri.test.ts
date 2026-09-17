/**
 * ShapeBuilder IRI rendering + the "Used by" line.
 *
 * Two defects this covers:
 *  1. A private `shortIriTail()` truncated every usage target to its last path
 *     segment, so the namespace was gone — `sh:NodeShape` and `ex:NodeShape`
 *     both read as "NodeShape".
 *  2. `disp()` abbreviates IRIs for editable inputs, so whatever it shows must
 *     survive `expand()` on the way back. A CURIE whose prefix is only in the
 *     well-known table (never declared in the document) used to be stored
 *     verbatim as the IRI.
 */
import { describe, it, expect, beforeAll, vi } from 'vitest';
import { render, fireEvent } from '@testing-library/svelte';
import { tick } from 'svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';
import { parseShapesGraph } from '../shaclModel.ts';
import { loadPrefixCcPrefixes } from '../rdf-utils.js';
import ShapeBuilder from '../../components/ShapeBuilder.svelte';

beforeAll(() => {
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

const TTL = `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix ex: <http://example.org/> .

ex:PersonShape a sh:NodeShape ;
  sh:targetClass ex:Person ;
  sh:property [ sh:path ex:name ; sh:datatype xsd:string ] .
`;

const usageText = (container: HTMLElement) =>
  [...container.querySelectorAll('.usage .usage-chip, .usage a')].map((e) => e.textContent?.trim());

describe('ShapeBuilder — IRI display', () => {
  it('renders a declared-prefix IRI as a CURIE in the shape name field', () => {
    const { container } = render(ShapeBuilder, { turtle: TTL });
    const input = container.querySelector<HTMLInputElement>('.name-input');
    expect(input?.value).toBe('ex:PersonShape');
  });

  // THE ROUND-TRIP GUARD. `disp()` must never show something `expand()` cannot
  // turn back into the same IRI, or committing the field corrupts the shape.
  it('expands a well-known prefix that the document never declared', async () => {
    const onChange = vi.fn();
    const { container } = render(ShapeBuilder, { turtle: TTL, onChange });
    const input = container.querySelector<HTMLInputElement>('.name-input')!;

    input.value = 'foaf:Person';
    await fireEvent.change(input);

    expect(onChange).toHaveBeenCalled();
    // Assert on the re-parsed model, not the Turtle text: the serializer is free
    // to emit the IRI as a CURIE plus an @prefix line, which is equally correct.
    const ttl = onChange.mock.calls.at(-1)![0] as string;
    expect(parseShapesGraph(ttl).shapes.map((s) => s.iri)).toEqual(['http://xmlns.com/foaf/0.1/Person']);
  });

  it('what the field displays survives a no-op re-commit', async () => {
    const onChange = vi.fn();
    const { container } = render(ShapeBuilder, { turtle: TTL, onChange });
    const input = container.querySelector<HTMLInputElement>('.name-input')!;

    const shown = input.value;
    input.value = shown; // re-commit exactly what the user sees
    await fireEvent.change(input);

    const ttl = onChange.mock.calls.at(-1)![0] as string;
    expect(parseShapesGraph(ttl).shapes.map((s) => s.iri)).toEqual(['http://example.org/PersonShape']);
  });
});

// A target IRI whose SCHEME happens to collide with a prefix label. curie()
// cannot shorten these (no '#' or '/' boundary to split on, or no registered
// namespace), so disp() shows them verbatim — and expand() must therefore hand
// them straight back. Committing the field must not turn a geo: URI into a
// GeoSPARQL term.
const nodeTargetTtl = (iri: string) => `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://example.org/> .

ex:PlaceShape a sh:NodeShape ;
  sh:targetNode <${iri}> .
`;

/** Re-commit the target field exactly as it is displayed, and return the
 *  targets of the re-parsed model the component emitted. */
async function recommitTarget(turtle: string, typed?: string) {
  const onChange = vi.fn();
  const { container } = render(ShapeBuilder, { turtle, onChange });
  const input = container.querySelector<HTMLInputElement>('.target-row input')!;
  const shown = input.value;
  if (typed !== undefined) {
    input.value = typed;
    await fireEvent.input(input);
  }
  await fireEvent.blur(input); // Combobox commits on blur, like a native input
  expect(onChange).toHaveBeenCalled();
  const ttl = onChange.mock.calls.at(-1)![0] as string;
  return { shown, targets: parseShapesGraph(ttl).shapes[0].targets };
}

describe('ShapeBuilder — non-http IRIs whose scheme collides with a prefix', () => {
  it('leaves a geo: URI alone instead of expanding it as a CURIE', async () => {
    const { shown, targets } = await recommitTarget(nodeTargetTtl('geo:52.37,4.89'));
    expect(shown).toBe('geo:52.37,4.89');
    expect(targets).toEqual([{ kind: 'node', value: 'geo:52.37,4.89' }]);
  });

  it('still expands a well-known CURIE typed into the same field', async () => {
    const { targets } = await recommitTarget(nodeTargetTtl('geo:52.37,4.89'), 'foaf:Person');
    expect(targets).toEqual([{ kind: 'node', value: 'http://xmlns.com/foaf/0.1/Person' }]);
  });
});

describe('ShapeBuilder — "Used by" targets', () => {
  it('prefers a display label the parent already resolved', () => {
    const { container } = render(ShapeBuilder, {
      turtle: TTL,
      usageTargets: [
        { iri: 'https://example.org/api/dataset/ds-7f3a', datasetId: 'ds-7f3a', label: 'Bridges 2024' },
      ],
    });
    expect(usageText(container)).toEqual(['Bridges 2024']);
    const link = container.querySelector<HTMLAnchorElement>('.usage a')!;
    expect(link.getAttribute('href')).toBe('/datasets/ds-7f3a');
    expect(link.getAttribute('title')).toBe('https://example.org/api/dataset/ds-7f3a');
  });

  it('still parses a bare IRI string when the parent resolved nothing', () => {
    const { container } = render(ShapeBuilder, {
      turtle: TTL,
      usageTargets: ['https://example.org/api/dataset/ds-7f3a/graphs/instances'],
    });
    expect(usageText(container)).toEqual(['ds-7f3a / instances']);
    expect(container.querySelector('.usage a')?.getAttribute('href')).toBe('/datasets/ds-7f3a');
  });

  // Fails on the pre-fix component, which rendered the bare tail "NodeShape".
  it('keeps the namespace on a non-dataset target instead of showing a bare tail', () => {
    const { container } = render(ShapeBuilder, {
      turtle: TTL,
      usageTargets: ['http://www.w3.org/ns/shacl#NodeShape'],
    });
    expect(usageText(container)).toEqual(['sh:NodeShape']);
    expect(container.querySelector('.usage-chip')?.getAttribute('title'))
      .toBe('http://www.w3.org/ns/shacl#NodeShape');
  });
});

// Everything below warms the prefix store, which is module-global and one-way —
// everything above it is written against the cold store.
const QUDT = 'http://qudt.org/schema/qudt/';
async function warmPrefixes() {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue({
    ok: true,
    json: async () => ({ doi: 'https://doi.org/', qudt: QUDT }),
  }));
  await loadPrefixCcPrefixes();
  vi.unstubAllGlobals();
}

describe('ShapeBuilder — when the prefix snapshot lands', () => {
  // The snapshot arrives after the first render. Without a reactive dependency
  // on it, a CURIE rendered against the cold tables kept its weaker form for
  // the life of the page.
  it('re-renders a CURIE only the snapshot can shorten', async () => {
    const turtle = `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://example.org/> .

ex:PlaceShape a sh:NodeShape ;
  sh:targetNode <${QUDT}Unit> ;
  sh:property [ sh:path ex:unit ; sh:node <${QUDT}Unit> ] .
`;
    const { container } = render(ShapeBuilder, { turtle });
    const input = () => container.querySelector<HTMLInputElement>('.target-row input')!;
    const chip = () => container.querySelector('.chip-shape')?.textContent?.trim();
    expect(input().value).toBe(QUDT + 'Unit');
    expect(chip()).toContain(QUDT + 'Unit');

    await warmPrefixes();
    await tick();
    // Both the editable field and the read-only chip — they recompute through
    // different code paths.
    expect(input().value).toBe('qudt:Unit');
    expect(chip()).toContain('qudt:Unit');
  });
});

describe('ShapeBuilder — with the prefix service warm', () => {
  beforeAll(warmPrefixes); // idempotent: already warm by the time this runs

  // The ~3700-entry snapshot turns far more schemes into prefix labels, so the
  // same round trip has to hold for 'doi:', 'file:', 'did:', … too.
  it('leaves a doi: URI alone instead of expanding it as a CURIE', async () => {
    const { shown, targets } = await recommitTarget(nodeTargetTtl('doi:10.1000/182'));
    expect(shown).toBe('doi:10.1000/182');
    expect(targets).toEqual([{ kind: 'node', value: 'doi:10.1000/182' }]);
  });

  it('still expands a CURIE the snapshot introduced', async () => {
    const { targets } = await recommitTarget(nodeTargetTtl('doi:10.1000/182'), 'qudt:Unit');
    expect(targets).toEqual([{ kind: 'node', value: 'http://qudt.org/schema/qudt/Unit' }]);
  });
});

describe('ShapeBuilder — a local name that starts with a digit', () => {
  it('expands a CURIE whose local part is legal Turtle but starts with a digit', async () => {
    // PN_LOCAL allows a leading digit, so `foaf:2ndName` is a CURIE the author
    // meant. Rejecting it stored the token verbatim as a bogus IRI.
    const { targets } = await recommitTarget(nodeTargetTtl('geo:52.37,4.89'), 'foaf:2ndName');
    expect(targets).toEqual([
      { kind: 'node', value: 'http://xmlns.com/foaf/0.1/2ndName' },
    ]);
  });
});
