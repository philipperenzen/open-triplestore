import { describe, it, expect } from 'vitest';
import { formatShaclPath } from '../validationReport.js';

// The backend serialises `sh:path` in SPARQL path syntax, so a predicate
// arrives wrapped in angle brackets. `http://ex.org/` is not a registered
// prefix; shortenIRI falls back to the last namespace segment, `ex.org:`.
const A = '<http://ex.org/a>';
const B = '<http://ex.org/b>';

describe('formatShaclPath', () => {
  it('shortens a bracketed IRI without leaving a stray ">"', () => {
    expect(formatShaclPath('<http://ex.org/label>')).toBe('ex.org:label');
  });

  it('shortens a plain (unbracketed) IRI', () => {
    expect(formatShaclPath('http://ex.org/label')).toBe('ex.org:label');
    expect(formatShaclPath('http://xmlns.com/foaf/0.1/name')).toBe('foaf:name');
  });

  it('uses registered prefixes for bracketed terms', () => {
    expect(formatShaclPath('<http://www.w3.org/2000/01/rdf-schema#label>')).toBe('rdfs:label');
  });

  it('passes a prefixed name through unchanged', () => {
    expect(formatShaclPath('ex:label')).toBe('ex:label');
    expect(formatShaclPath('rdfs:label')).toBe('rdfs:label');
  });

  it('keeps the inverse operator', () => {
    expect(formatShaclPath(`^${A}`)).toBe('^ex.org:a');
  });

  it('keeps the sequence operator', () => {
    expect(formatShaclPath(`${A}/${B}`)).toBe('ex.org:a/ex.org:b');
  });

  it('keeps the alternative operator', () => {
    expect(formatShaclPath(`${A}|${B}`)).toBe('ex.org:a|ex.org:b');
  });

  it('keeps the cardinality modifiers', () => {
    expect(formatShaclPath(`${A}*`)).toBe('ex.org:a*');
    expect(formatShaclPath(`${A}+`)).toBe('ex.org:a+');
    expect(formatShaclPath(`${A}?`)).toBe('ex.org:a?');
  });

  it('handles nested combinations', () => {
    expect(formatShaclPath(`^${A}/(${B}|<http://ex.org/c>)*`)).toBe('^ex.org:a/(ex.org:b|ex.org:c)*');
  });

  it('accepts a custom shortener', () => {
    const upper = (iri: string) => iri.toUpperCase();
    expect(formatShaclPath(`${A}/${B}`, upper)).toBe('HTTP://EX.ORG/A/HTTP://EX.ORG/B');
  });

  it('is safe on empty and non-string input', () => {
    expect(formatShaclPath('')).toBe('');
    expect(formatShaclPath(null as unknown as string)).toBe('');
    expect(formatShaclPath(undefined as unknown as string)).toBe('');
  });
});
