// The inline viz affordances on RdfTerm: a WKT geometry literal gets a map
// chip and a 3D-model file URL gets a 3D chip — these are what surface the
// previews in the triple table, the graph explorer and resource panels, so
// lock them in with a real component render.
import { describe, it, expect, beforeAll } from 'vitest';
import { render } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';
import RdfTerm from '../../components/RdfTerm.svelte';

beforeAll(() => {
  addMessages('en', en as Record<string, unknown>);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

describe('RdfTerm viz chips', () => {
  it('shows a map chip for a wktLiteral', () => {
    const { container } = render(RdfTerm, {
      term: {
        type: 'literal',
        value: 'POINT(5.86 51.85)',
        datatype: 'http://www.opengis.net/ont/geosparql#wktLiteral',
      },
    });
    expect(container.querySelector('.viz-chip')).toBeTruthy();
    expect(container.querySelector('.viz-chip.model')).toBeFalsy();
  });

  it('shows a 3D chip for a glb URL term', () => {
    const { container } = render(RdfTerm, {
      term: { type: 'uri', value: 'https://files.example/boog-noord.glb' },
    });
    expect(container.querySelector('.viz-chip.model')).toBeTruthy();
  });

  it('shows a 3D chip for an STL anyURI literal', () => {
    const { container } = render(RdfTerm, {
      term: {
        type: 'literal',
        value: 'https://upload.wikimedia.org/wikipedia/commons/c/c6/Big_Ben.stl',
        datatype: 'http://www.w3.org/2001/XMLSchema#anyURI',
      },
    });
    expect(container.querySelector('.viz-chip.model')).toBeTruthy();
  });

  it('shows no chips for an ordinary literal or IRI', () => {
    const plain = render(RdfTerm, { term: { type: 'literal', value: 'hello' } });
    expect(plain.container.querySelector('.viz-chip')).toBeFalsy();
    const iri = render(RdfTerm, { term: { type: 'uri', value: 'https://example.org/x' } });
    expect(iri.container.querySelector('.viz-chip')).toBeFalsy();
  });
});
