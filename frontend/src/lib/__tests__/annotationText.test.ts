/**
 * Annotation prose parsing — the light markup real vocabularies put in
 * rdfs:comment / skos:definition literals (schema.org being the loudest user).
 */
import { describe, it, expect } from 'vitest';
import {
  parseAnnotation,
  hasAnnotationMarkup,
  resolveTermRef,
  type AnnotationSegment,
} from '../annotationText';

const kinds = (segs: AnnotationSegment[]) => segs.map((s) => s.kind);
const texts = (segs: AnnotationSegment[]) => segs.map((s) => s.text);

describe('parseAnnotation', () => {
  it('returns nothing for empty input', () => {
    expect(parseAnnotation('')).toEqual([]);
    expect(parseAnnotation(null)).toEqual([]);
    expect(parseAnnotation('   \n\n  ')).toEqual([]);
  });

  it('keeps plain prose as a single text segment', () => {
    const [p] = parseAnnotation('An offer to transfer some rights to an item.');
    expect(p).toEqual([{ kind: 'text', text: 'An offer to transfer some rights to an item.' }]);
  });

  it('splits paragraphs on blank lines and on escaped \\n\\n', () => {
    expect(parseAnnotation('one\n\ntwo')).toHaveLength(2);
    expect(parseAnnotation('one\\n\\ntwo')).toHaveLength(2);
    expect(parseAnnotation('one\r\n\r\ntwo')).toHaveLength(2);
  });

  it('reads markdown links', () => {
    const [p] = parseAnnotation('see [Check Digit calculator](http://www.gs1.org/check) now');
    expect(kinds(p)).toEqual(['text', 'link', 'text']);
    expect(p[1]).toEqual({
      kind: 'link',
      text: 'Check Digit calculator',
      href: 'http://www.gs1.org/check',
    });
  });

  it('reads [[term]] references, with optional |label', () => {
    const [p] = parseAnnotation('As the [[businessFunction]] property, …');
    expect(p[1]).toEqual({ kind: 'term', text: 'businessFunction', name: 'businessFunction' });

    const [q] = parseAnnotation('see [[businessFunction|the function]] here');
    expect(q[1]).toEqual({ kind: 'term', text: 'the function', name: 'businessFunction' });
  });

  it('linkifies bare URLs without swallowing sentence punctuation', () => {
    const [p] = parseAnnotation('defaults to http://purl.org/goodrelations/v1#Sell; and so on');
    const link = p.find((s) => s.kind === 'link');
    expect(link).toMatchObject({ href: 'http://purl.org/goodrelations/v1#Sell' });
    expect(texts(p).join('')).toContain('; and so on');
  });

  it('keeps balanced parentheses inside a bare URL', () => {
    const [p] = parseAnnotation('see https://en.wikipedia.org/wiki/Offer_(law) for more');
    expect(p.find((s) => s.kind === 'link')).toMatchObject({
      href: 'https://en.wikipedia.org/wiki/Offer_(law)',
    });
  });

  it('parses the schema.org Offer comment end to end', () => {
    const raw =
      'An offer to transfer some rights to an item or to provide a service — for ' +
      'example, an offer to sell tickets to an event.\\n\\nNote: As the ' +
      '[[businessFunction]] property, which identifies the form of offer (e.g. sell, ' +
      'lease, repair, dispose), defaults to http://purl.org/goodrelations/v1#Sell; an ' +
      'Offer without a defined businessFunction value can be assumed to be an offer ' +
      'to sell.\\n\\nFor [GTIN](http://www.gs1.org/barcodes/technical/idkeys/gtin)-related ' +
      'fields, see [Check Digit calculator](http://www.gs1.org/barcodes/support/check_digit_calculator) ' +
      'and [validation guide](http://www.gs1us.org/resources/standards/gtin-validation-guide) ' +
      'from [GS1](http://www.gs1.org/).';

    const paras = parseAnnotation(raw);
    expect(paras).toHaveLength(3);

    // Paragraph 2: a term ref plus a bare URL.
    expect(paras[1].some((s) => s.kind === 'term' && s.name === 'businessFunction')).toBe(true);
    expect(paras[1].some((s) => s.kind === 'link' && s.href.includes('goodrelations'))).toBe(true);

    // Paragraph 3: four markdown links, labels preserved.
    const links = paras[2].filter((s) => s.kind === 'link');
    expect(links).toHaveLength(4);
    expect(texts(links)).toEqual(['GTIN', 'Check Digit calculator', 'validation guide', 'GS1']);

    // Nothing is lost: the '-related fields' text after the GTIN link survives.
    expect(texts(paras[2]).join('')).toContain('-related fields');
  });

  it('leaves bracket-looking prose alone', () => {
    const [p] = parseAnnotation('a value in [square brackets] is not a link');
    expect(kinds(p)).toEqual(['text']);
  });
});

describe('hasAnnotationMarkup', () => {
  it('is false for plain prose and true when links or refs are present', () => {
    expect(hasAnnotationMarkup('just words')).toBe(false);
    expect(hasAnnotationMarkup('see [[Thing]]')).toBe(true);
    expect(hasAnnotationMarkup('see https://example.org')).toBe(true);
  });
});

describe('resolveTermRef', () => {
  it('resolves against a slash namespace', () => {
    expect(resolveTermRef('businessFunction', 'https://schema.org/Offer')).toBe(
      'https://schema.org/businessFunction',
    );
  });

  it('resolves against a hash namespace', () => {
    expect(resolveTermRef('label', 'http://www.w3.org/2000/01/rdf-schema#Class')).toBe(
      'http://www.w3.org/2000/01/rdf-schema#label',
    );
  });

  it('returns empty when there is no usable base', () => {
    expect(resolveTermRef('x', '')).toBe('');
    expect(resolveTermRef('', 'https://schema.org/Offer')).toBe('');
    expect(resolveTermRef('x', 'urn:noseparator')).toBe('');
  });
});
