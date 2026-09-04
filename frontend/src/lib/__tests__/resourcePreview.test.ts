/**
 * The hover card's fact extraction (lib/resourcePreview.js).
 *
 * summarizeTriples distills a /api/browse/triples page into what the card
 * shows; getting the language preference or predicate sets wrong silently
 * degrades every hover in the app, so the distillation is pinned here.
 */
import { describe, it, expect, beforeAll } from 'vitest';
import { init, addMessages, locale } from 'svelte-i18n';
import en from '../i18n/en.json';
import { summarizeTriples } from '../resourcePreview.js';

const RDFS = 'http://www.w3.org/2000/01/rdf-schema#';
const RDF = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#';
const SKOS = 'http://www.w3.org/2004/02/skos/core#';

const uri = (value: string) => ({ type: 'uri', value });
const lit = (value: string, language?: string) =>
  language ? { type: 'literal', value, language } : { type: 'literal', value };
const triple = (predicate: string, object: object) => ({
  subject: uri('http://ex.org/bridge/1'),
  predicate: uri(predicate),
  object,
  graph: uri('http://ex.org/g'),
});

beforeAll(() => {
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

describe('summarizeTriples', () => {
  it('extracts label, types and description from a triples page', () => {
    const res = {
      hasMore: true,
      triples: [
        triple(`${RDF}type`, uri('http://ex.org/def#Bridge')),
        triple(`${RDFS}label`, lit('Waalbrug', 'nl')),
        triple(`${RDFS}label`, lit('Waal Bridge', 'en')),
        triple(`${RDFS}comment`, lit('An arch bridge across the Waal.', 'en')),
        triple('http://ex.org/def#span', lit('244')),
      ],
    };
    const p = summarizeTriples(res);
    expect(p.known).toBe(true);
    // Locale is 'en': the English label wins over the first-seen Dutch one.
    expect(p.label).toBe('Waal Bridge');
    expect(p.types).toEqual(['http://ex.org/def#Bridge'].map(() => expect.any(String)));
    expect(p.description).toContain('arch bridge');
    expect(p.facts).toBe(5);
    expect(p.more).toBe(true);
  });

  it('falls back through en to any language, and skos:prefLabel counts as a label', () => {
    locale.set('de');
    const res = {
      triples: [triple(`${SKOS}prefLabel`, lit('Brug', 'nl'))],
    };
    expect(summarizeTriples(res).label).toBe('Brug');
    locale.set('en');
  });

  it('an empty page is an unknown resource, not an empty card', () => {
    const p = summarizeTriples({ triples: [] });
    expect(p.known).toBe(false);
    expect(p.label).toBeNull();
    expect(p.types).toEqual([]);
  });

  it('caps the type chips and dedupes repeated types', () => {
    const res = {
      triples: [1, 1, 2, 3, 4, 5].map((n) => triple(`${RDF}type`, uri(`http://ex.org/def#T${n}`))),
    };
    const p = summarizeTriples(res);
    expect(p.types.length).toBe(3);
    expect(new Set(p.types).size).toBe(3);
  });
});
