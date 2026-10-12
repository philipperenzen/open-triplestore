// The pure helpers behind the reasoning, SWRL, ShEx, RDF Patch and
// version-diff views (lib/reasoning.ts).
import { describe, it, expect } from 'vitest';
import {
  normaliseIgnored, statusTone, shexVerdict, parsePrefixLines, prefixLines,
  parsePatchRows, splitTerms, patchHeader, withLogHeaders, gcPreview, shortIri,
} from '../reasoning';

describe('normaliseIgnored', () => {
  it('reads the ignored rows', () => {
    const rows = normaliseIgnored({ ignored: [{ construct: 'ObjectUnionOf', count: 3, example: 'ex:A ⊔ ex:B' }] });
    expect(rows).toEqual([{ construct: 'ObjectUnionOf', count: 3, example: 'ex:A ⊔ ex:B' }]);
  });
  it('is empty when nothing was left out', () => {
    expect(normaliseIgnored({ ignored: [] })).toEqual([]);
    expect(normaliseIgnored(null)).toEqual([]);
  });
});

describe('statusTone', () => {
  it('maps run and job statuses to badge tones', () => {
    expect(statusTone('ok')).toBe('ok');
    expect(statusTone('succeeded')).toBe('ok');
    expect(statusTone('running')).toBe('busy');
    expect(statusTone('timeout')).toBe('warn');
    expect(statusTone('inconsistent')).toBe('bad');
    expect(statusTone(null)).toBe('idle');
  });
});

describe('shexVerdict', () => {
  it('reads the serde enum', () => {
    expect(shexVerdict('Conformant')).toEqual({ ok: true, reason: '' });
    expect(shexVerdict({ NonConformant: 'missing ex:name' })).toEqual({ ok: false, reason: 'missing ex:name' });
  });
});

describe('prefix lines', () => {
  it('parses label and IRI per line, reporting the rest', () => {
    const { prefixes, bad } = parsePrefixLines('ex: https://example.org/\n@prefix foaf: <http://xmlns.com/foaf/0.1/> .\n: http://d/\nnonsense');
    expect(prefixes).toEqual({ ex: 'https://example.org/', foaf: 'http://xmlns.com/foaf/0.1/', '': 'http://d/' });
    expect(bad).toEqual(['nonsense']);
  });
  it('round-trips', () => {
    const lines = prefixLines({ ex: 'https://example.org/' });
    expect(parsePrefixLines(lines).prefixes).toEqual({ ex: 'https://example.org/' });
  });
});

describe('RDF Patch rows', () => {
  const patch = [
    'H id <urn:uuid:1> .',
    'H prev <urn:uuid:0> .',
    'TX .',
    'PA "ex" <http://example.org/> .',
    'A <http://e/s> <http://e/p> "a b . c"@en <http://e/g> .',
    'D _:b1 <http://e/p> "1"^^<http://www.w3.org/2001/XMLSchema#integer> <http://e/g> .',
    'TC .',
  ].join('\n');

  it('keeps the change rows and their graphs', () => {
    const rows = parsePatchRows(patch);
    expect(rows.map((r) => r.op)).toEqual(['PA', 'A', 'D']);
    expect(rows[1].graph).toBe('<http://e/g>');
    expect(splitTerms(rows[1].terms)).toEqual(['<http://e/s>', '<http://e/p>', '"a b . c"@en', '<http://e/g>']);
    expect(splitTerms(rows[2].terms)[2]).toBe('"1"^^<http://www.w3.org/2001/XMLSchema#integer>');
  });

  it('reads headers', () => {
    expect(patchHeader(patch, 'id')).toBe('urn:uuid:1');
    expect(patchHeader(patch, 'prev')).toBe('urn:uuid:0');
    expect(patchHeader('A <a> <b> <c> .', 'id')).toBeNull();
  });

  it('adds the log headers a patch lacks, and only those', () => {
    const body = 'A <a> <b> <c> .';
    expect(withLogHeaders(body, 'urn:uuid:last', () => 'urn:uuid:new')).toBe(
      'H id <urn:uuid:new> .\nH prev <urn:uuid:last> .\nA <a> <b> <c> .',
    );
    expect(withLogHeaders(body, null, () => 'urn:uuid:new')).toBe('H id <urn:uuid:new> .\nA <a> <b> <c> .');
    expect(withLogHeaders(patch, 'urn:uuid:other', () => 'x')).toBe(patch);
  });
});

describe('gcPreview', () => {
  const v = (version: string, status: string, created_at: string) => ({ version, status, created_at });
  const versions = [
    v('1.0.0', 'published', '2026-01-01T00:00:00Z'),
    v('1.1.0', 'draft', '2026-02-01T00:00:00Z'),
    v('1.2.0', 'staged', '2026-03-01T00:00:00Z'),
    v('1.3.0', 'deprecated', '2026-04-01T00:00:00Z'),
  ];
  it('keeps the newest unpublished versions and never a published one', () => {
    expect(gcPreview(versions, 2).map((x) => x.version)).toEqual(['1.1.0']);
    expect(gcPreview(versions, 0).map((x) => x.version)).toEqual(['1.3.0', '1.2.0', '1.1.0']);
    expect(gcPreview(versions, 10)).toEqual([]);
  });
  it('breaks a timestamp tie on the label, newest first', () => {
    const tied = [v('a', 'draft', '2026-01-01T00:00:00Z'), v('b', 'draft', '2026-01-01T00:00:00Z')];
    expect(gcPreview(tied, 1).map((x) => x.version)).toEqual(['a']);
  });
});

describe('shortIri', () => {
  it('keeps the local part', () => {
    expect(shortIri('<https://example.org/onto#Asset>')).toBe('Asset');
    expect(shortIri('urn:x')).toBe('urn:x');
  });
});
