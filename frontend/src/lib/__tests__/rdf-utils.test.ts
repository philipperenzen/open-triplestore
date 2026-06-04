import { describe, it, expect } from 'vitest';
import { detectGraphRolesFromContent, normalizeGraphRole, graphRoleLabel, contentKindToRole, graphResultsToElements, shortenIRI } from '../rdf-utils.js';

describe('normalizeGraphRole', () => {
  it('folds legacy/singular spellings onto canonical tokens', () => {
    expect(normalizeGraphRole('instance')).toBe('instances');
    expect(normalizeGraphRole('instances')).toBe('instances');
    expect(normalizeGraphRole('abox')).toBe('instances');
    expect(normalizeGraphRole('tbox')).toBe('model');
    expect(normalizeGraphRole('Model')).toBe('model');
  });
  it('returns null for empty/unknown input', () => {
    expect(normalizeGraphRole('')).toBeNull();
    expect(normalizeGraphRole(null)).toBeNull();
    expect(normalizeGraphRole('mixed')).toBeNull();
  });
});

describe('graphRoleLabel', () => {
  it('labels both canonical and legacy spellings', () => {
    expect(graphRoleLabel('instance')).toBe('Instances');
    expect(graphRoleLabel('vocabulary')).toBe('Vocabulary');
    expect(graphRoleLabel('unknown')).toBeNull();
  });
});

describe('contentKindToRole', () => {
  it('maps typed verdicts to a role and rejects non-role verdicts', () => {
    expect(contentKindToRole('shapes')).toBe('shapes');
    expect(contentKindToRole('entailment')).toBe('entailment');
    expect(contentKindToRole('mixed')).toBeNull();
    expect(contentKindToRole('empty')).toBeNull();
  });
});

describe('detectGraphRolesFromContent', () => {
  it('classifies each TriG graph block independently', () => {
    const trig = `
      @prefix owl: <http://www.w3.org/2002/07/owl#> .
      @prefix skos: <http://www.w3.org/2004/02/skos/core#> .
      @prefix ex: <http://example.org/> .

      <http://example.org/g/model> {
        ex:Person a owl:Class .
      }

      <http://example.org/g/vocab> {
        ex:Scheme a skos:ConceptScheme .
        ex:Term a skos:Concept .
      }
    `;
    const roles = detectGraphRolesFromContent('data.trig', trig);
    expect(roles['http://example.org/g/model']).toBe('model');
    expect(roles['http://example.org/g/vocab']).toBe('vocabulary');
  });

  it('resolves prefixed TriG graph names via @prefix declarations', () => {
    const trig = `
      @prefix g: <http://example.org/g/> .
      @prefix owl: <http://www.w3.org/2002/07/owl#> .
      @prefix ex: <http://example.org/> .
      g:model { ex:Thing a owl:Class . }
    `;
    const roles = detectGraphRolesFromContent('data.trig', trig);
    expect(roles['http://example.org/g/model']).toBe('model');
  });

  it('groups N-Quads lines by graph and classifies each', () => {
    const nq = [
      '<http://example.org/Person> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#Class> <http://example.org/g/model> .',
      '<http://example.org/s> <http://example.org/p> <http://example.org/o> <http://example.org/g/data> .',
    ].join('\n');
    const roles = detectGraphRolesFromContent('data.nq', nq);
    expect(roles['http://example.org/g/model']).toBe('model');
    // A plain triple with no schema signal is not 'mixed'.
    expect(roles['http://example.org/g/data']).not.toBe('mixed');
  });

  it('returns an empty map for triple formats', () => {
    expect(detectGraphRolesFromContent('data.ttl', 'whatever')).toEqual({});
  });
});

describe('graphResultsToElements', () => {
  const uri = (value) => ({ type: 'uri', value });
  const lit = (value, extra = {}) => ({ type: 'literal', value, ...extra });
  const XSD_INT = 'http://www.w3.org/2001/XMLSchema#integer';
  const RDFS_LABEL = 'http://www.w3.org/2000/01/rdf-schema#label';

  it('gives a literal a stable id across calls so re-expansion does not duplicate it', () => {
    const triple = [{ s: uri('http://ex/Amsterdam'), p: uri('http://ex/population'), o: lit('921402', { datatype: XSD_INT }) }];
    const first = graphResultsToElements(triple);
    const second = graphResultsToElements(triple);

    const litId1 = first.nodes.find(n => n.data.nodeType === 'literal').data.id;
    const litId2 = second.nodes.find(n => n.data.nodeType === 'literal').data.id;
    expect(litId2).toBe(litId1);
    expect(second.edges[0].data.id).toBe(first.edges[0].data.id);

    // Simulate the incremental-expansion dedupe (applyElements/updateGraph):
    // the second batch contributes nothing new because ids match.
    const existing = new Set(first.nodes.map(n => n.data.id));
    expect(second.nodes.filter(n => !existing.has(n.data.id))).toHaveLength(0);
  });

  it('keeps the same literal value under different subjects as separate nodes', () => {
    const { nodes } = graphResultsToElements([
      { s: uri('http://ex/a'), p: uri('http://ex/p'), o: lit('5', { datatype: XSD_INT }) },
      { s: uri('http://ex/b'), p: uri('http://ex/p'), o: lit('5', { datatype: XSD_INT }) },
    ]);
    const literals = nodes.filter(n => n.data.nodeType === 'literal');
    expect(literals).toHaveLength(2);
    expect(literals[0].data.id).not.toBe(literals[1].data.id);
  });

  it('keeps language-tagged variants of the same value as separate nodes', () => {
    const { nodes } = graphResultsToElements([
      { s: uri('http://ex/Amsterdam'), p: uri(RDFS_LABEL), o: lit('Amsterdam', { language: 'en' }) },
      { s: uri('http://ex/Amsterdam'), p: uri(RDFS_LABEL), o: lit('Amsterdam', { language: 'nl' }) },
    ]);
    expect(nodes.filter(n => n.data.nodeType === 'literal')).toHaveLength(2);
  });

  it('threads datatype and language onto literal node data (drives the badge + hover)', () => {
    const { nodes } = graphResultsToElements([
      { s: uri('http://ex/Ada'), p: uri('http://ex/age'), o: lit('36', { datatype: XSD_INT }) },
      { s: uri('http://ex/Ada'), p: uri(RDFS_LABEL), o: lit('Ada', { language: 'en' }) },
    ]);
    const intNode = nodes.find(n => n.data.literalValue === '36');
    const langNode = nodes.find(n => n.data.literalValue === 'Ada');
    expect(intNode.data.datatype).toBe(XSD_INT);
    expect(intNode.data.language).toBe(null);
    expect(langNode.data.language).toBe('en');
    expect(langNode.data.datatype).toBe(null);
    // URI nodes carry no literal datatype/language.
    expect(nodes.find(n => n.data.nodeType === 'uri').data.datatype ?? null).toBe(null);
  });

  it('dedupes identical triples within a single call (no duplicate-id edges)', () => {
    const row = { s: uri('http://ex/a'), p: uri('http://ex/p'), o: lit('x') };
    const { nodes, edges } = graphResultsToElements([row, { ...row }]);
    expect(nodes.filter(n => n.data.nodeType === 'literal')).toHaveLength(1);
    expect(edges).toHaveLength(1);
  });

  it('skips RDF-star quoted-triple bindings instead of throwing on a non-string value', () => {
    // SPARQL-JSON shape for an embedded (RDF-star) triple: value is an object,
    // not a string. A quoted triple can't be a simple graph node, so it must be
    // skipped rather than reaching shortenIRI (which used to throw on `.startsWith`).
    const star = (s, p, o) => ({ type: 'triple', value: { subject: s, predicate: p, object: o } });
    const rows = [
      { s: uri('http://ex/Ada'), p: uri('http://ex/age'), o: lit('36', { datatype: XSD_INT }) },
      {
        s: star(uri('http://ex/Ada'), uri('http://ex/knows'), uri('http://ex/Charles')),
        p: uri('http://ex/confidence'),
        o: lit('0.9'),
      },
    ];
    let result;
    expect(() => { result = graphResultsToElements(rows); }).not.toThrow();
    // Only the plain triple contributes to the graph; the quoted triple is dropped.
    expect(result.edges).toHaveLength(1);
    expect(result.nodes.some(n => n.data.id === 'http://ex/Ada')).toBe(true);
  });
});

describe('shortenIRI', () => {
  it('shortens known and unknown IRIs', () => {
    expect(shortenIRI('http://www.w3.org/1999/02/22-rdf-syntax-ns#type')).toBe('rdf:type');
    expect(shortenIRI('http://example.org/ns/BridgeDataset')).toBe('ns:BridgeDataset');
  });

  it('never throws on non-string input (e.g. an RDF-star object value)', () => {
    expect(() => shortenIRI(undefined as unknown as string)).not.toThrow();
    expect(shortenIRI(null as unknown as string)).toBe('');
    expect(typeof shortenIRI({ subject: {} } as unknown as string)).toBe('string');
  });
});
