// The prefixed name stays the label; the full IRI is one click away and stays
// there. A `title` tooltip is not "on demand" — it needs a pointer, it cannot
// be reached from a keyboard, and it is invisible on a touch screen, so it was
// never an answer for someone who needs to read the IRI itself.
import { describe, it, expect, beforeEach, beforeAll } from 'vitest';
import { get } from 'svelte/store';
import { render } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';
import RdfTerm from '../../components/RdfTerm.svelte';
import {
  iriDisplay,
  initIriDisplay,
  setIriDisplay,
  toggleIriDisplay,
} from '../iriDisplay';

const IRI = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#type';
const CURIE = 'rdf:type';

beforeAll(() => {
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

beforeEach(() => {
  localStorage.clear();
  setIriDisplay('curie');
});

describe('the IRI display preference', () => {
  it('starts on prefixed names', () => {
    localStorage.clear();
    initIriDisplay();
    expect(get(iriDisplay)).toBe('curie');
  });

  it('survives a reload', () => {
    setIriDisplay('full');
    // A fresh boot reads the same storage.
    iriDisplay.set('curie');
    initIriDisplay();
    expect(get(iriDisplay)).toBe('full');
  });

  it('ignores a stored value it does not recognise', () => {
    localStorage.setItem('ots-iri-display', 'shortened-ish');
    initIriDisplay();
    expect(get(iriDisplay)).toBe('curie');
  });

  it('toggles between the two', () => {
    toggleIriDisplay();
    expect(get(iriDisplay)).toBe('full');
    toggleIriDisplay();
    expect(get(iriDisplay)).toBe('curie');
  });

  it('does not throw when storage is unavailable', () => {
    const real = Storage.prototype.setItem;
    Storage.prototype.setItem = () => {
      throw new DOMException('denied', 'SecurityError');
    };
    try {
      expect(() => setIriDisplay('full')).not.toThrow();
      expect(get(iriDisplay)).toBe('full');
    } finally {
      Storage.prototype.setItem = real;
    }
  });
});

describe('RdfTerm follows the preference', () => {
  it('renders the prefixed name by default', () => {
    const { container } = render(RdfTerm, { term: { type: 'uri', value: IRI } });
    expect(container.querySelector('.rdf-term')?.textContent).toBe(CURIE);
  });

  it('renders the whole IRI when asked for it', async () => {
    const { container } = render(RdfTerm, { term: { type: 'uri', value: IRI } });
    setIriDisplay('full');
    await Promise.resolve();
    expect(container.querySelector('.rdf-term')?.textContent).toBe(IRI);
  });

  it('leaves literals and blank nodes alone', async () => {
    setIriDisplay('full');
    const lit = render(RdfTerm, { term: { type: 'literal', value: 'Amsterdam' } });
    expect(lit.container.querySelector('.rdf-term')?.textContent).toBe('"Amsterdam"');
    const bnode = render(RdfTerm, { term: { type: 'bnode', value: 'b0' } });
    expect(bnode.container.querySelector('.rdf-term')?.textContent).toBe('_:b0');
  });

  it('copies the full IRI either way, because that is what a copy is for', async () => {
    // The copy control already copied `term.value` rather than the label; the
    // switch must not turn it into a copy of whatever happens to be on screen.
    const { container } = render(RdfTerm, { term: { type: 'uri', value: IRI } });
    setIriDisplay('full');
    await Promise.resolve();
    expect(container.querySelector('.rdf-term')?.textContent).toBe(IRI);
  });
});
