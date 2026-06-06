// Standards-facing frontend rendering tests.
//
// The frontend re-implements several W3C concrete syntaxes and result formats in
// pure TypeScript (no backend round-trip): the RDF 1.1 serializers
// (Turtle / N-Triples / N-Quads / TriG), an N-Triples reader, the SPARQL 1.1
// Query-Results CSV writer, a SPARQL 1.1 Update INSERT/DELETE-DATA previewer, and
// RDF 1.1 IRI ↔ prefixed-name handling. These suites exercise the spec-relevant
// edge cases (datatype/language literals, blank nodes, escaping, named graphs,
// CSV injection, prefix round-trips) so a regression in client-side rendering is
// caught without a server. The store-/protocol-level conformance lives in the
// Rust suites under `tests/`.

import { describe, it, expect } from 'vitest';
import {
  shortenIRI,
  expandPrefix,
  isValidIri,
  detectRdfFormat,
  termLabel,
  toNTriples,
  toNQuads,
  toTurtle,
  toTrig,
  parseNTriplesToBindings,
  parseSparqlUpdatePreview,
  resultsToCsv,
} from '../rdf-utils.js';

// Term constructors mirroring the SPARQL-JSON / internal term shape.
const uri = (value: string) => ({ type: 'uri', value });
const bnode = (value: string) => ({ type: 'bnode', value });
const lit = (value: string, extra: Record<string, string> = {}) => ({ type: 'literal', value, ...extra });

const RDF_TYPE = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#type';
const XSD_INT = 'http://www.w3.org/2001/XMLSchema#integer';
const XSD_STRING = 'http://www.w3.org/2001/XMLSchema#string';
const OWL_CLASS = 'http://www.w3.org/2002/07/owl#Class';
const FOAF_PERSON = 'http://xmlns.com/foaf/0.1/Person';

// ── RDF 1.1 IRIs & prefixed names ──────────────────────────────────────────────

describe('IRI ↔ prefixed-name (RDF 1.1)', () => {
  it('shortens well-known namespaces to their conventional prefix', () => {
    expect(shortenIRI(RDF_TYPE)).toBe('rdf:type');
    expect(shortenIRI('http://www.w3.org/ns/shacl#NodeShape')).toBe('sh:NodeShape');
    expect(shortenIRI('http://www.w3.org/ns/dcat#Catalog')).toBe('dcat:Catalog');
    expect(shortenIRI('http://www.opengis.net/ont/geosparql#asWKT')).toBe('geo:asWKT');
  });

  it('round-trips shorten ∘ expand for every common prefix', () => {
    const samples = [OWL_CLASS, RDF_TYPE, 'http://www.w3.org/ns/shacl#minCount', FOAF_PERSON];
    for (const iri of samples) {
      const short = shortenIRI(iri);
      expect(short).toMatch(/^[a-z]+:[A-Za-z]/);
      expect(expandPrefix(short)).toBe(iri);
    }
  });

  it('does not abbreviate when the local name contains a slash or hash', () => {
    // local part "ns/Thing" can't be a turtle pname → keep a derived label, not sh:…
    expect(shortenIRI('http://www.w3.org/ns/shacl#ns/Thing')).not.toContain('sh:');
  });

  it('expandPrefix returns null for unknown prefixes and non-prefixed input', () => {
    expect(expandPrefix('nope:Thing')).toBeNull();
    // `http` is not a registered prefix label, so a full IRI does not expand.
    expect(expandPrefix('http://example.org/x')).toBeNull();
    expect(expandPrefix('plain')).toBeNull();
  });
});

describe('isValidIri (RDF 1.1 absolute-IRI gate)', () => {
  it('accepts absolute http/urn/mailto IRIs', () => {
    expect(isValidIri('http://example.org/x')).toBe(true);
    expect(isValidIri('https://example.org/x#frag')).toBe(true);
    expect(isValidIri('urn:isbn:0451450523')).toBe(true);
    expect(isValidIri('mailto:alice@example.org')).toBe(true);
  });

  it('rejects relative references, blanks, and non-strings', () => {
    expect(isValidIri('not an iri')).toBe(false);
    expect(isValidIri('/relative/path')).toBe(false);
    expect(isValidIri('')).toBe(false);
    expect(isValidIri(null)).toBe(false);
    expect(isValidIri(42)).toBe(false);
  });
});

// ── RDF format detection (content negotiation hints) ────────────────────────────

describe('detectRdfFormat', () => {
  it('maps each RDF 1.1 serialization extension to its media type', () => {
    expect(detectRdfFormat('data.ttl').contentType).toBe('text/turtle');
    expect(detectRdfFormat('data.nt').contentType).toBe('application/n-triples');
    expect(detectRdfFormat('data.nq').contentType).toBe('application/n-quads');
    expect(detectRdfFormat('data.trig').contentType).toBe('application/trig');
    expect(detectRdfFormat('schema.rdf').contentType).toBe('application/rdf+xml');
    expect(detectRdfFormat('onto.owl').contentType).toBe('application/rdf+xml');
    expect(detectRdfFormat('data.jsonld').contentType).toBe('application/ld+json');
  });

  it('is case-insensitive and falls back to Turtle', () => {
    expect(detectRdfFormat('DATA.TTL').contentType).toBe('text/turtle');
    expect(detectRdfFormat('mystery.bin').contentType).toBe('text/turtle');
  });
});

// ── N-Triples / N-Quads / Turtle / TriG serializers (RDF 1.1) ───────────────────

describe('toNTriples (RDF 1.1 N-Triples)', () => {
  it('serializes IRIs, typed/lang literals, and blank nodes', () => {
    const nt = toNTriples([
      { subject: uri('http://ex/s'), predicate: uri('http://ex/p'), object: lit('plain') },
      { subject: uri('http://ex/s'), predicate: uri('http://ex/age'), object: lit('42', { datatype: XSD_INT }) },
      { subject: uri('http://ex/s'), predicate: uri('http://ex/label'), object: lit('hi', { language: 'en' }) },
      { subject: bnode('b0'), predicate: uri(RDF_TYPE), object: uri(OWL_CLASS) },
    ]);
    const lines = nt.split('\n');
    expect(lines[0]).toBe('<http://ex/s> <http://ex/p> "plain" .');
    expect(lines[1]).toBe(`<http://ex/s> <http://ex/age> "42"^^<${XSD_INT}> .`);
    expect(lines[2]).toBe('<http://ex/s> <http://ex/label> "hi"@en .');
    expect(lines[3]).toBe(`_:b0 <${RDF_TYPE}> <${OWL_CLASS}> .`);
  });

  it('omits the xsd:string datatype (RDF 1.1 simple literal) and escapes specials', () => {
    const nt = toNTriples([
      { subject: uri('http://ex/s'), predicate: uri('http://ex/p'), object: lit('str', { datatype: XSD_STRING }) },
      { subject: uri('http://ex/s'), predicate: uri('http://ex/p'), object: lit('a"b\nc\\d') },
    ]);
    const lines = nt.split('\n');
    expect(lines[0]).toBe('<http://ex/s> <http://ex/p> "str" .');
    expect(lines[1]).toBe('<http://ex/s> <http://ex/p> "a\\"b\\nc\\\\d" .');
  });
});

describe('toNQuads (RDF 1.1 N-Quads)', () => {
  it('appends the graph IRI as the fourth term and omits it for the default graph', () => {
    const nq = toNQuads([
      { subject: uri('http://ex/s'), predicate: uri('http://ex/p'), object: uri('http://ex/o'), graph: uri('http://ex/g') },
      { subject: uri('http://ex/s'), predicate: uri('http://ex/p'), object: uri('http://ex/o') },
    ]);
    const lines = nq.split('\n');
    expect(lines[0]).toBe('<http://ex/s> <http://ex/p> <http://ex/o> <http://ex/g> .');
    expect(lines[1]).toBe('<http://ex/s> <http://ex/p> <http://ex/o> .');
  });
});

describe('toTurtle (RDF 1.1 Turtle)', () => {
  it('emits @prefix directives and abbreviates well-known terms', () => {
    const ttl = toTurtle([
      { subject: uri(FOAF_PERSON), predicate: uri(RDF_TYPE), object: uri(OWL_CLASS) },
    ]);
    expect(ttl).toContain('@prefix foaf: <http://xmlns.com/foaf/0.1/> .');
    expect(ttl).toContain('@prefix owl: <http://www.w3.org/2002/07/owl#> .');
    expect(ttl).toContain('foaf:Person rdf:type owl:Class .');
  });

  it('escapes quotes and newlines inside a Turtle literal', () => {
    const ttl = toTurtle([
      { subject: uri(FOAF_PERSON), predicate: uri('http://www.w3.org/2000/01/rdf-schema#label'), object: lit('a"b\nc') },
    ]);
    expect(ttl).toContain('rdfs:label');
    expect(ttl).toContain('"a\\"b\\nc"');
  });
});

describe('toTrig (RDF 1.1 TriG)', () => {
  it('groups triples under their named-graph block', () => {
    const trig = toTrig([
      { subject: uri(FOAF_PERSON), predicate: uri(RDF_TYPE), object: uri(OWL_CLASS), graph: uri('http://ex/g1') },
      { subject: uri('http://ex/s'), predicate: uri('http://ex/p'), object: lit('v'), graph: uri('http://ex/g2') },
    ]);
    expect(trig).toContain('<http://ex/g1> {');
    expect(trig).toContain('<http://ex/g2> {');
    expect(trig).toContain('foaf:Person rdf:type owl:Class .');
  });
});

// ── N-Triples reader (RDF 1.1) ──────────────────────────────────────────────────

describe('parseNTriplesToBindings (RDF 1.1 N-Triples reader)', () => {
  it('parses IRIs, datatyped/lang literals, and blank nodes into SPARQL-JSON terms', () => {
    const doc = [
      '# a comment line is ignored',
      '<http://ex/s> <http://ex/p> <http://ex/o> .',
      `<http://ex/s> <http://ex/age> "42"^^<${XSD_INT}> .`,
      '<http://ex/s> <http://ex/label> "hi"@en .',
      '_:b0 <http://ex/p> "plain" .',
      '',
    ].join('\n');
    const { head, results } = parseNTriplesToBindings(doc);
    expect(head.vars).toEqual(['s', 'p', 'o']);
    const b = results.bindings;
    expect(b).toHaveLength(4);
    expect(b[0].o).toEqual({ type: 'uri', value: 'http://ex/o' });
    expect(b[1].o).toEqual({ type: 'literal', value: '42', datatype: XSD_INT });
    expect(b[2].o).toEqual({ type: 'literal', value: 'hi', language: 'en' });
    expect(b[3].s).toEqual({ type: 'bnode', value: 'b0' });
  });

  it('unescapes quotes and backslashes in literals', () => {
    const { results } = parseNTriplesToBindings('<http://ex/s> <http://ex/p> "a\\"b\\\\c" .');
    expect(results.bindings[0].o.value).toBe('a"b\\c');
  });

  it('round-trips toNTriples → parseNTriplesToBindings', () => {
    const triples = [
      { subject: uri('http://ex/Ada'), predicate: uri('http://ex/age'), object: lit('36', { datatype: XSD_INT }) },
      { subject: uri('http://ex/Ada'), predicate: uri('http://ex/label'), object: lit('Ada', { language: 'en' }) },
      { subject: uri('http://ex/Ada'), predicate: uri('http://ex/knows'), object: uri('http://ex/Charles') },
    ];
    const reparsed = parseNTriplesToBindings(toNTriples(triples)).results.bindings;
    expect(reparsed).toHaveLength(3);
    expect(reparsed[0].o).toEqual({ type: 'literal', value: '36', datatype: XSD_INT });
    expect(reparsed[1].o).toEqual({ type: 'literal', value: 'Ada', language: 'en' });
    expect(reparsed[2].o).toEqual({ type: 'uri', value: 'http://ex/Charles' });
  });
});

// ── SPARQL 1.1 Update preview ───────────────────────────────────────────────────

describe('parseSparqlUpdatePreview (SPARQL 1.1 Update)', () => {
  it('extracts INSERT DATA and DELETE DATA triples and flags pattern updates', () => {
    const upd = parseSparqlUpdatePreview(
      'INSERT DATA { <http://ex/s> <http://ex/p> <http://ex/o> } ;\n' +
        'DELETE DATA { <http://ex/s> <http://ex/old> <http://ex/x> }'
    );
    expect(upd.isPatternBased).toBe(false);
    expect(upd.inserts).toHaveLength(1);
    expect(upd.deletes).toHaveLength(1);
    expect(upd.inserts[0].o).toEqual({ type: 'uri', value: 'http://ex/o' });
  });

  it('expands PREFIX declarations inside the data block', () => {
    const upd = parseSparqlUpdatePreview(
      'PREFIX ex: <http://example.org/>\nINSERT DATA { ex:s ex:p ex:o }'
    );
    expect(upd.inserts).toHaveLength(1);
    expect(upd.inserts[0].s).toEqual({ type: 'uri', value: 'http://example.org/s' });
    expect(upd.inserts[0].o).toEqual({ type: 'uri', value: 'http://example.org/o' });
  });

  it('reads triples nested inside a GRAPH block', () => {
    const upd = parseSparqlUpdatePreview(
      'INSERT DATA { GRAPH <http://ex/g> { <http://ex/s> <http://ex/p> "v" } }'
    );
    expect(upd.inserts).toHaveLength(1);
    expect(upd.inserts[0].o).toEqual({ type: 'literal', value: 'v' });
  });

  it('flags WHERE-based (pattern) updates without inventing data triples', () => {
    const upd = parseSparqlUpdatePreview('DELETE { ?s ?p ?o } WHERE { ?s ?p ?o }');
    expect(upd.isPatternBased).toBe(true);
    expect(upd.inserts).toHaveLength(0);
    expect(upd.deletes).toHaveLength(0);
  });
});

// ── SPARQL 1.1 Query Results CSV ────────────────────────────────────────────────

describe('resultsToCsv (SPARQL 1.1 Query Results CSV)', () => {
  const results = (vars: string[], rows: Record<string, unknown>[]) => ({
    head: { vars },
    results: { bindings: rows },
  });

  it('writes a header row and quotes each value', () => {
    const csv = resultsToCsv(results(['s', 'o'], [{ s: uri('http://ex/a'), o: lit('Alice') }]));
    expect(csv).toBe('s,o\n"http://ex/a","Alice"');
  });

  it('escapes embedded double-quotes by doubling them', () => {
    const csv = resultsToCsv(results(['v'], [{ v: lit('say "hi"') }]));
    expect(csv).toBe('v\n"say ""hi"""');
  });

  it('leaves an unbound variable as an empty field', () => {
    const csv = resultsToCsv(results(['a', 'b'], [{ a: lit('x') }]));
    expect(csv).toBe('a,b\n"x",');
  });

  it('neutralizes CSV/formula injection by prefixing a quote', () => {
    const csv = resultsToCsv(
      results(['v'], [{ v: lit('=SUM(1+1)') }, { v: lit('+cmd') }, { v: lit('@ref') }])
    );
    expect(csv).toContain('"\'=SUM(1+1)"');
    expect(csv).toContain('"\'+cmd"');
    expect(csv).toContain('"\'@ref"');
  });

  it('returns an empty string for malformed/empty results', () => {
    expect(resultsToCsv(null)).toBe('');
    expect(resultsToCsv(undefined)).toBe('');
    expect(resultsToCsv({ head: { vars: [] }, results: { bindings: [] } } as never)).toBe('');
  });
});

// ── termLabel (display rendering of RDF terms) ──────────────────────────────────

describe('termLabel', () => {
  it('renders IRIs shortened, literals quoted with lang, and blanks with _: ', () => {
    expect(termLabel(uri(FOAF_PERSON))).toBe('foaf:Person');
    expect(termLabel(lit('hi', { language: 'en' }))).toBe('"hi"@en');
    expect(termLabel(lit('plain'))).toBe('"plain"');
    expect(termLabel(bnode('b1'))).toBe('_:b1');
  });
});
