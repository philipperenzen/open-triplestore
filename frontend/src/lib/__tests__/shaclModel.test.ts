import { describe, it, expect, vi } from 'vitest';
import { Parser } from 'n3';
import {
  parseShapesGraph,
  serializeShapesGraph,
  renderPath,
  makeCurie,
  SEVERITY_WARNING,
} from '../shaclModel.ts';
import { loadPrefixCcPrefixes } from '../rdf-utils.js';

// makeCurie must not depend on the resolver offering each label for at most one
// namespace — that invariant lives in another module. `wkStub.override` injects
// a resolver that breaks it; left null, every test below sees the real one.
type WellKnown = { prefix: string; namespace: string; local: string } | null;
const wkStub = vi.hoisted(() => ({ override: null as ((iri: string) => WellKnown) | null }));
vi.mock('../rdf-utils.js', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../rdf-utils.js')>();
  return {
    ...actual,
    wellKnownCurie: (iri: string) => (wkStub.override ? wkStub.override(iri) : actual.wellKnownCurie(iri)),
  };
});

const PFX = `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix ex: <http://example.org/> .
`;
const XSD = 'http://www.w3.org/2001/XMLSchema#';
const EX = 'http://example.org/';

/** parse → serialize, the operation the builder performs on every edit. */
const reser = (ttl: string) => serializeShapesGraph(parseShapesGraph(ttl));

// ─── Canonical quad comparison (blank-node tolerant) ─────────────────────────
// Quad-set equality up to blank-node relabelling: bnode labels are replaced by
// Weisfeiler-Lehman-style neighbourhood signatures, then the quad strings are
// compared as sorted multisets.

function djb2(s: string): string {
  let h = 5381;
  for (let i = 0; i < s.length; i++) h = ((h * 33) ^ s.charCodeAt(i)) >>> 0;
  return h.toString(36);
}

function canonQuads(ttl: string): string[] {
  const quads = new Parser().parse(ttl) as unknown as {
    subject: { termType: string; value: string };
    predicate: { value: string };
    object: { termType: string; value: string; language?: string; datatype?: { value: string } };
  }[];
  const bnodes = new Set<string>();
  for (const q of quads) {
    if (q.subject.termType === 'BlankNode') bnodes.add(q.subject.value);
    if (q.object.termType === 'BlankNode') bnodes.add(q.object.value);
  }
  let sig = new Map<string, string>();
  for (const b of bnodes) sig.set(b, 'b');
  const tstr = (t: { termType: string; value: string; language?: string; datatype?: { value: string } }): string =>
    t.termType === 'BlankNode'
      ? '_:' + sig.get(t.value)
      : t.termType === 'Literal'
        ? `"${t.value}"${t.language ? '@' + t.language : ''}^^${t.datatype?.value || ''}`
        : `<${t.value}>`;
  for (let round = 0; round < 8; round++) {
    const next = new Map<string, string>();
    for (const b of bnodes) {
      const parts: string[] = [sig.get(b)!];
      for (const q of quads) {
        if (q.subject.termType === 'BlankNode' && q.subject.value === b)
          parts.push(`S|${q.predicate.value}|${tstr(q.object)}`);
        if (q.object.termType === 'BlankNode' && q.object.value === b)
          parts.push(`O|${q.predicate.value}|${tstr(q.subject as never)}`);
      }
      parts.sort();
      next.set(b, djb2(parts.join('')));
    }
    sig = next;
  }
  return quads.map((q) => `${tstr(q.subject as never)} <${q.predicate.value}> ${tstr(q.object)}`).sort();
}

/** Assert parse→serialize is quad-set identical to the source, return output. */
function expectRoundTrip(ttl: string): string {
  const out = serializeShapesGraph(parseShapesGraph(ttl));
  expect(canonQuads(out)).toEqual(canonQuads(ttl));
  return out;
}

const curie = makeCurie({ ex: EX, sh: 'http://www.w3.org/ns/shacl#' });

describe('parseShapesGraph', () => {
  it('parses a node shape with target + typed property constraints', () => {
    const g = parseShapesGraph(
      PFX +
        `ex:PersonShape a sh:NodeShape ;
          sh:targetClass ex:Person ;
          sh:property [ sh:path ex:name ; sh:minCount 1 ; sh:maxCount 1 ; sh:datatype xsd:string ] ;
          sh:property [ sh:path ex:age ; sh:datatype xsd:integer ; sh:minInclusive 0 ; sh:maxInclusive 120 ] .`,
    );
    expect(g.canRoundTrip).toBe(true);
    expect(g.hasUnsupported).toBeFalsy();
    expect(g.shapes).toHaveLength(1);
    const s = g.shapes[0];
    expect(s.iri).toBe(EX + 'PersonShape');
    expect(s.targets).toEqual([{ kind: 'class', value: EX + 'Person' }]);
    expect(s.properties).toHaveLength(2);

    const name = s.properties[0];
    expect(name.path).toBe(EX + 'name');
    expect(name.c.minCount).toBe(1);
    expect(name.c.maxCount).toBe(1);
    expect(name.c.datatype).toBe(XSD + 'string');

    const age = s.properties[1];
    expect(age.c.datatype).toBe(XSD + 'integer');
    expect(age.c.minInclusive).toBe('0');
    expect(age.c.maxInclusive).toBe('120');
  });

  it('recognises every target kind', () => {
    const g = parseShapesGraph(
      PFX +
        `ex:S a sh:NodeShape ;
          sh:targetClass ex:C ; sh:targetNode ex:n ;
          sh:targetSubjectsOf ex:p ; sh:targetObjectsOf ex:q .`,
    );
    expect(g.canRoundTrip).toBe(true);
    expect(g.shapes[0].targets).toEqual([
      { kind: 'class', value: EX + 'C' },
      { kind: 'node', value: EX + 'n' },
      { kind: 'subjectsOf', value: EX + 'p' },
      { kind: 'objectsOf', value: EX + 'q' },
    ]);
  });

  it('is a serialisation fixpoint over the full supported subset', () => {
    const src =
      PFX +
      `ex:Shape a sh:NodeShape ;
        sh:targetClass ex:Thing ;
        sh:closed true ;
        sh:ignoredProperties ( rdf:type ) ;
        sh:property [
          sh:path ex:code ; sh:name "Code" ; sh:minCount 1 ; sh:maxCount 1 ;
          sh:datatype xsd:string ; sh:pattern "^[A-Z]{2}$" ; sh:flags "i" ;
          sh:minLength 2 ; sh:maxLength 2 ; sh:message "two upper letters" ; sh:severity sh:Warning
        ] ;
        sh:property [ sh:path ex:status ; sh:in ( ex:Open ex:Closed ) ] ;
        sh:property [ sh:path rdfs:label ; sh:languageIn ( "en" "nl" ) ; sh:uniqueLang true ] ;
        sh:property [ sh:path ex:home ; sh:nodeKind sh:IRI ] ;
        sh:property [ sh:path ex:addr ; sh:node ex:AddrShape ] .`;
    const s1 = reser(src);
    const s2 = reser(s1);
    expect(s2).toBe(s1); // serialiser output is stable under re-parse
    expect(parseShapesGraph(s1).canRoundTrip).toBe(true);
    expectRoundTrip(src);
  });

  it('preserves enum (mixed iri/literal) + languageIn through a round-trip', () => {
    const g = parseShapesGraph(
      reser(
        PFX +
          `ex:S a sh:NodeShape ; sh:targetClass ex:T ;
            sh:property [ sh:path ex:status ; sh:in ( ex:Open "mid" ) ] ;
            sh:property [ sh:path rdfs:label ; sh:languageIn ( "en" "nl" ) ] .`,
      ),
    );
    const status = g.shapes[0].properties.find((p) => p.path.endsWith('status'))!;
    expect(status.c.in).toEqual([
      { type: 'iri', value: EX + 'Open' },
      { type: 'literal', value: 'mid' },
    ]);
    const label = g.shapes[0].properties.find((p) => p.path.endsWith('label'))!;
    expect(label.c.languageIn).toEqual(['en', 'nl']);
  });

  it('keeps sh:ignoredProperties even without sh:closed', () => {
    const out = reser(
      PFX + `ex:S a sh:NodeShape ; sh:targetClass ex:T ; sh:ignoredProperties ( ex:foo ex:bar ) .`,
    );
    expect(out).toContain('sh:ignoredProperties ( ex:foo ex:bar )');
  });

  it('makes implicit node shapes explicit on serialise', () => {
    const out = reser(PFX + `ex:S sh:targetClass ex:T ; sh:property [ sh:path ex:n ; sh:minCount 1 ] .`);
    expect(out).toContain('ex:S a sh:NodeShape');
  });

  it('lets model edits survive serialize → parse', () => {
    const g = parseShapesGraph(PFX + `ex:S a sh:NodeShape ; sh:targetClass ex:T ; sh:property [ sh:path ex:n ] .`);
    g.shapes[0].properties[0].c.minCount = 1;
    g.shapes[0].properties[0].c.datatype = XSD + 'string';
    const g2 = parseShapesGraph(serializeShapesGraph(g));
    expect(g2.shapes[0].properties[0].c.minCount).toBe(1);
    expect(g2.shapes[0].properties[0].c.datatype).toBe(XSD + 'string');
  });

  it('handles empty and invalid input', () => {
    const empty = parseShapesGraph('   ');
    expect(empty.shapes).toEqual([]);
    expect(empty.canRoundTrip).toBe(true);

    const bad = parseShapesGraph('this is not <turtle {{{');
    expect(bad.parseError).toBeTruthy();
    expect(bad.canRoundTrip).toBe(false);
  });
});

describe('lossless retention (extraQuads)', () => {
  it('keeps a SPARQL constraint verbatim and flags it', () => {
    const src =
      PFX +
      `ex:S a sh:NodeShape ; sh:targetClass ex:T ;
        sh:sparql [ sh:message "bad" ; sh:select "SELECT $this WHERE { $this ex:bad true }" ] .`;
    const g = parseShapesGraph(src);
    expect(g.canRoundTrip).toBe(true);
    expect(g.hasUnsupported).toBe(true);
    expect(g.shapes[0].hasUnsupported).toBe(true);
    expectRoundTrip(src);
  });

  it('keeps SHACL-AF rules verbatim', () => {
    const src =
      PFX +
      `ex:S a sh:NodeShape ; sh:targetClass ex:T ;
        sh:rule [ a sh:SPARQLRule ; sh:construct "CONSTRUCT { $this ex:x 1 } WHERE { }" ] .`;
    expectRoundTrip(src);
  });

  it('keeps non-SHACL statements mixed into the document', () => {
    const src =
      PFX +
      `ex:S a sh:NodeShape ; sh:targetClass ex:T ;
        sh:property [ sh:path ex:n ; sh:minCount 1 ] .
       ex:T a rdfs:Class ; rdfs:label "Thing" .
       ex:n a rdf:Property .`;
    const g = parseShapesGraph(src);
    expect(g.extraQuads?.length).toBe(3); // ex:T type+label, ex:n type
    expectRoundTrip(src);
  });

  it('keeps a typed (date) range bound verbatim with its datatype', () => {
    const src =
      PFX +
      `ex:S a sh:NodeShape ; sh:targetClass ex:T ;
        sh:property [ sh:path ex:when ; sh:minInclusive "2020-01-01"^^xsd:date ] .`;
    const g = parseShapesGraph(src);
    expect(g.canRoundTrip).toBe(true);
    expect(g.shapes[0].properties[0].hasUnsupported).toBe(true);
    const out = expectRoundTrip(src);
    expect(out).toContain('xsd:date');
  });

  it('keeps unknown annotations on shapes and properties editable-around', () => {
    const src =
      PFX +
      `ex:S a sh:NodeShape ; sh:targetClass ex:T ; ex:approvedBy ex:Alice ;
        sh:property [ sh:path ex:n ; sh:minCount 1 ; ex:note "internal" ] .`;
    const g = parseShapesGraph(src);
    expect(g.shapes[0].hasUnsupported).toBe(true);
    // edit a modelled constraint while extras survive
    g.shapes[0].properties[0].c.minCount = 3;
    const out = serializeShapesGraph(g);
    const g2 = parseShapesGraph(out);
    expect(g2.shapes[0].properties[0].c.minCount).toBe(3);
    expect(out).toContain('ex:approvedBy ex:Alice');
    expect(out).toContain('ex:note "internal"');
  });

  it('retains a second sh:or constraint (only the first is modelled)', () => {
    const src =
      PFX +
      `ex:S a sh:NodeShape ;
        sh:or ( [ sh:path ex:a ; sh:minCount 1 ] ) ;
        sh:or ( [ sh:path ex:b ; sh:minCount 1 ] ) .`;
    const g = parseShapesGraph(src);
    expect(g.shapes[0].logic?.or).toHaveLength(1);
    expect(g.shapes[0].hasUnsupported).toBe(true);
    expectRoundTrip(src);
  });
});

describe('logical operators', () => {
  it('models sh:or with shape references on a node shape', () => {
    const src = PFX + `ex:S a sh:NodeShape ; sh:or ( ex:A ex:B ) . ex:A a sh:NodeShape . ex:B a sh:NodeShape .`;
    const g = parseShapesGraph(src);
    const s = g.shapes.find((x) => x.iri === EX + 'S')!;
    expect(s.logic?.or).toEqual([{ ref: EX + 'A' }, { ref: EX + 'B' }]);
    expect(s.hasUnsupported).toBeFalsy();
    expectRoundTrip(src);
  });

  it('models inline anonymous shapes in sh:or recursively', () => {
    const src =
      PFX +
      `ex:S a sh:NodeShape ; sh:or (
        [ sh:path ex:email ; sh:minCount 1 ]
        [ sh:property [ sh:path ex:phone ; sh:minCount 1 ] ]
      ) .`;
    const g = parseShapesGraph(src);
    const or = g.shapes[0].logic!.or!;
    expect(or).toHaveLength(2);
    expect('inline' in or[0] && or[0].inline.path).toBe(EX + 'email');
    expect('inline' in or[1] && or[1].inline.properties![0].path).toBe(EX + 'phone');
    expect(g.hasUnsupported).toBeFalsy();
    expectRoundTrip(src);
  });

  it('models sh:and / sh:xone / sh:not on property shapes', () => {
    const src =
      PFX +
      `ex:S a sh:NodeShape ; sh:targetClass ex:T ;
        sh:property [
          sh:path ex:v ;
          sh:and ( [ sh:minLength 1 ] [ sh:maxLength 10 ] ) ;
          sh:xone ( [ sh:datatype xsd:string ] [ sh:datatype xsd:integer ] ) ;
          sh:not [ sh:hasValue "forbidden" ]
        ] .`;
    const g = parseShapesGraph(src);
    const p = g.shapes[0].properties[0];
    expect(p.logic?.and).toHaveLength(2);
    expect(p.logic?.xone).toHaveLength(2);
    expect(p.logic?.not).toHaveLength(1);
    expect(g.hasUnsupported).toBeFalsy();
    expectRoundTrip(src);
  });

  it('models multiple sh:not constraints', () => {
    const src = PFX + `ex:S a sh:NodeShape ; sh:not ex:A ; sh:not [ sh:path ex:b ; sh:minCount 1 ] .`;
    const g = parseShapesGraph(src);
    expect(g.shapes[0].logic?.not).toHaveLength(2);
    expectRoundTrip(src);
  });

  it('models nested logic inside inline shapes', () => {
    const src =
      PFX +
      `ex:S a sh:NodeShape ; sh:or (
        [ sh:and ( [ sh:path ex:a ; sh:minCount 1 ] [ sh:path ex:b ; sh:minCount 1 ] ) ]
        ex:C
      ) .`;
    const g = parseShapesGraph(src);
    const first = g.shapes[0].logic!.or![0];
    expect('inline' in first && first.inline.logic?.and).toHaveLength(2);
    expectRoundTrip(src);
  });
});

describe('qualified value shapes', () => {
  it('models the full qualified constraint set with an inline shape', () => {
    const src =
      PFX +
      `ex:S a sh:NodeShape ; sh:targetClass ex:T ;
        sh:property [
          sh:path ex:member ;
          sh:qualifiedValueShape [ sh:class ex:Manager ; sh:nodeKind sh:IRI ] ;
          sh:qualifiedMinCount 1 ;
          sh:qualifiedMaxCount 2 ;
          sh:qualifiedValueShapesDisjoint true
        ] .`;
    const g = parseShapesGraph(src);
    const q = g.shapes[0].properties[0].qualified!;
    expect('inline' in q.shape && q.shape.inline.c.class).toBe(EX + 'Manager');
    expect(q.minCount).toBe(1);
    expect(q.maxCount).toBe(2);
    expect(q.disjoint).toBe(true);
    expect(g.hasUnsupported).toBeFalsy();
    expectRoundTrip(src);
  });

  it('models a qualified shape reference', () => {
    const src =
      PFX +
      `ex:S a sh:NodeShape ; sh:targetClass ex:T ;
        sh:property [ sh:path ex:k ; sh:qualifiedValueShape ex:Q ; sh:qualifiedMinCount 1 ] .`;
    const g = parseShapesGraph(src);
    expect(g.shapes[0].properties[0].qualified).toEqual({ shape: { ref: EX + 'Q' }, minCount: 1 });
    expectRoundTrip(src);
  });
});

describe('property paths', () => {
  const pathOf = (body: string) => {
    const g = parseShapesGraph(PFX + `ex:S a sh:NodeShape ; sh:targetClass ex:T ; sh:property [ sh:path ${body} ; sh:minCount 1 ] .`);
    expect(g.hasUnsupported).toBeFalsy();
    return g.shapes[0].properties[0];
  };

  it('models inverse paths', () => {
    const p = pathOf('[ sh:inversePath ex:worksFor ]');
    expect(p.pathExpr).toEqual({ kind: 'inverse', path: { kind: 'predicate', iri: EX + 'worksFor' } });
    expect(renderPath(p.pathExpr, curie)).toBe('^ex:worksFor');
  });

  it('models sequence paths', () => {
    const p = pathOf('( ex:a ex:b )');
    expect(p.pathExpr).toEqual({
      kind: 'sequence',
      paths: [
        { kind: 'predicate', iri: EX + 'a' },
        { kind: 'predicate', iri: EX + 'b' },
      ],
    });
    expect(renderPath(p.pathExpr, curie)).toBe('ex:a/ex:b');
  });

  it('models alternative paths', () => {
    const p = pathOf('[ sh:alternativePath ( ex:a ex:b ) ]');
    expect(p.pathExpr?.kind).toBe('alternative');
    expect(renderPath(p.pathExpr, curie)).toBe('ex:a|ex:b');
  });

  it('models zeroOrMore / oneOrMore / zeroOrOne paths', () => {
    expect(renderPath(pathOf('[ sh:zeroOrMorePath ex:p ]').pathExpr, curie)).toBe('ex:p*');
    expect(renderPath(pathOf('[ sh:oneOrMorePath ex:p ]').pathExpr, curie)).toBe('ex:p+');
    expect(renderPath(pathOf('[ sh:zeroOrOnePath ex:p ]').pathExpr, curie)).toBe('ex:p?');
  });

  it('models nested path expressions', () => {
    const p = pathOf('( ex:a [ sh:zeroOrMorePath [ sh:inversePath ex:b ] ] )');
    expect(renderPath(p.pathExpr, curie)).toBe('ex:a/((^ex:b)*)');
  });

  it('round-trips every path kind quad-identically', () => {
    for (const body of [
      '[ sh:inversePath ex:worksFor ]',
      '( ex:a ex:b )',
      '( ex:a ex:b ex:c )',
      '[ sh:alternativePath ( ex:a ex:b ) ]',
      '[ sh:zeroOrMorePath ex:p ]',
      '[ sh:oneOrMorePath ex:p ]',
      '[ sh:zeroOrOnePath ex:p ]',
      '( ex:a [ sh:zeroOrMorePath ex:b ] )',
    ]) {
      expectRoundTrip(PFX + `ex:S a sh:NodeShape ; sh:targetClass ex:T ; sh:property [ sh:path ${body} ; sh:minCount 1 ] .`);
    }
  });

  it('retains unparseable paths verbatim', () => {
    const src = PFX + `ex:S a sh:NodeShape ; sh:targetClass ex:T ; sh:property [ sh:path ( ex:only ) ; sh:minCount 1 ] .`;
    const g = parseShapesGraph(src); // 1-member sequence is invalid SHACL
    expect(g.shapes[0].properties[0].hasUnsupported).toBe(true);
    expectRoundTrip(src);
  });
});

describe('named property shapes', () => {
  it('preserves identity and stays editable', () => {
    const src =
      PFX +
      `ex:S a sh:NodeShape ; sh:targetClass ex:T ; sh:property ex:NameProp .
       ex:NameProp a sh:PropertyShape ; sh:path ex:name ; sh:minCount 1 .`;
    const g = parseShapesGraph(src);
    expect(g.canRoundTrip).toBe(true);
    expect(g.hasUnsupported).toBeFalsy();
    const p = g.shapes[0].properties[0];
    expect(p.iri).toBe(EX + 'NameProp');
    expect(p.declared).toBe(true);
    expect(p.path).toBe(EX + 'name');
    expectRoundTrip(src);
    // edits keep the name
    p.c.maxCount = 1;
    const out = serializeShapesGraph(g);
    expect(out).toContain('sh:property ex:NameProp');
    expect(out).toContain('ex:NameProp a sh:PropertyShape');
    expect(parseShapesGraph(out).shapes[0].properties[0].c.maxCount).toBe(1);
  });

  it('shares one instance between node shapes and emits one block', () => {
    const src =
      PFX +
      `ex:A a sh:NodeShape ; sh:targetClass ex:T ; sh:property ex:P .
       ex:B a sh:NodeShape ; sh:targetClass ex:U ; sh:property ex:P .
       ex:P a sh:PropertyShape ; sh:path ex:name .`;
    const g = parseShapesGraph(src);
    const a = g.shapes.find((s) => s.iri === EX + 'A')!;
    const b = g.shapes.find((s) => s.iri === EX + 'B')!;
    expect(a.properties[0]).toBe(b.properties[0]); // same object → edits stay in sync
    const out = expectRoundTrip(src);
    expect(out.match(/ex:P a sh:PropertyShape/g)).toHaveLength(1);
  });

  it('keeps a dangling named property reference', () => {
    const src = PFX + `ex:S a sh:NodeShape ; sh:targetClass ex:T ; sh:property ex:Elsewhere .`;
    expectRoundTrip(src);
  });
});

describe('severities', () => {
  it('models custom severity IRIs', () => {
    const src =
      PFX +
      `ex:S a sh:NodeShape ; sh:targetClass ex:T ; sh:severity ex:Critical ;
        sh:property [ sh:path ex:n ; sh:severity ex:Blocker ] .`;
    const g = parseShapesGraph(src);
    expect(g.shapes[0].severity).toBe(EX + 'Critical');
    expect(g.shapes[0].properties[0].severity).toBe(EX + 'Blocker');
    expect(g.hasUnsupported).toBeFalsy();
    expectRoundTrip(src);
  });

  it('keeps the convenience constants working', () => {
    const g = parseShapesGraph(
      PFX + `ex:S a sh:NodeShape ; sh:targetClass ex:T ; sh:property [ sh:path ex:n ; sh:severity sh:Warning ] .`,
    );
    expect(g.shapes[0].properties[0].severity).toBe(SEVERITY_WARNING);
  });
});

describe('kitchen sink + W3C person example', () => {
  const KITCHEN =
    PFX +
    `ex:KitchenSink a sh:NodeShape ;
      sh:targetClass ex:Thing ;
      sh:severity ex:Critical ;
      sh:closed true ;
      sh:ignoredProperties ( rdf:type ) ;
      sh:or ( ex:AShape [ sh:path ex:b ; sh:minCount 1 ] ) ;
      sh:not [ sh:property [ sh:path ex:deprecated ; sh:minCount 1 ] ] ;
      sh:property ex:NamedProp ;
      sh:property [
        sh:path ( ex:a [ sh:zeroOrMorePath ex:b ] ) ;
        sh:qualifiedValueShape [ sh:class ex:Q ] ;
        sh:qualifiedMinCount 1 ;
        sh:qualifiedMaxCount 3 ;
        sh:qualifiedValueShapesDisjoint true
      ] ;
      sh:property [ sh:path ex:when ; sh:minInclusive "2020-01-01"^^xsd:date ] ;
      sh:sparql [ sh:message "custom" ; sh:select "SELECT $this WHERE { $this ex:bad true }" ] .

    ex:NamedProp a sh:PropertyShape ; sh:path ex:name ; sh:minCount 1 ;
      sh:xone ( [ sh:datatype xsd:string ] [ sh:datatype rdf:langString ] ) .

    ex:Thing a rdfs:Class ; rdfs:label "Thing" .`;

  it('round-trips a kitchen-sink document quad-identically', () => {
    const g = parseShapesGraph(KITCHEN);
    expect(g.canRoundTrip).toBe(true);
    expect(g.hasUnsupported).toBe(true); // sparql + date bound + ontology triples
    const shape = g.shapes.find((s) => s.iri === EX + 'KitchenSink')!;
    expect(shape.logic?.or).toHaveLength(2);
    expect(shape.properties.some((p) => p.iri === EX + 'NamedProp')).toBe(true);
    const out = expectRoundTrip(KITCHEN);
    // and the serialised form is a fixpoint
    expect(reser(out)).toBe(out);
  });

  it('round-trips the W3C person example shape', () => {
    const src =
      PFX +
      `ex:PersonShape a sh:NodeShape ;
        sh:targetClass ex:Person ;
        sh:property [
          sh:path ex:ssn ;
          sh:maxCount 1 ;
          sh:datatype xsd:string ;
          sh:pattern "^\\\\d{3}-\\\\d{2}-\\\\d{4}$"
        ] ;
        sh:property [
          sh:path ex:worksFor ;
          sh:class ex:Company ;
          sh:nodeKind sh:IRI
        ] ;
        sh:property [
          sh:path [ sh:inversePath ex:worksFor ] ;
          sh:name "employees"
        ] ;
        sh:closed true ;
        sh:ignoredProperties ( rdf:type ) .`;
    const g = parseShapesGraph(src);
    expect(g.canRoundTrip).toBe(true);
    expect(g.hasUnsupported).toBeFalsy();
    expectRoundTrip(src);
  });
});

// ─── CURIE fallback (the builder used to render bare <http://…> IRIs) ─────────

describe('CURIE resolution', () => {
  const FOAF = 'http://xmlns.com/foaf/0.1/';
  // A shapes graph that declares only sh: — everything else must be shortened
  // by the well-known fallback or not at all.
  const SPARSE = `@prefix sh: <http://www.w3.org/ns/shacl#> .
<http://example.org/PersonShape> a sh:NodeShape ;
  sh:targetClass <${FOAF}Person> ;
  sh:property [ sh:path <${FOAF}name> ; sh:minCount 1 ] .
`;

  it('shortens a well-known IRI the document never declared', () => {
    // This is exactly ShapeBuilder's disp(): a curie built from the document's
    // own prefix map, which for foaf: holds nothing.
    expect(makeCurie({})(FOAF + 'Person')).toBe('foaf:Person');
    expect(makeCurie({})('http://www.w3.org/ns/shacl#NodeShape')).toBe('sh:NodeShape');
    expect(makeCurie({})('http://purl.org/dc/terms/title')).toBe('dct:title');
  });

  it('still refuses IRIs nothing knows, rather than inventing a prefix', () => {
    expect(makeCurie({})('http://example.org/private/Thing')).toBe('<http://example.org/private/Thing>');
    expect(makeCurie({})('urn:example:shapes')).toBe('<urn:example:shapes>');
  });

  it('lets a document-declared prefix win over the well-known one', () => {
    expect(makeCurie({ f: FOAF })(FOAF + 'Person')).toBe('f:Person');
    const out = serializeShapesGraph(parseShapesGraph(`@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix f: <${FOAF}> .
<http://example.org/S> a sh:NodeShape ; sh:targetClass f:Person .
`));
    expect(out).toContain('sh:targetClass f:Person');
    expect(out).not.toContain('foaf:Person');
  });

  it('never reuses a label the document bound to a different namespace', () => {
    // 'foaf:Person' here would denote http://example.org/foaf/Person — a
    // different term. The full IRI is the only correct rendering.
    const prefixes = { foaf: 'http://example.org/foaf/' };
    expect(makeCurie(prefixes)(FOAF + 'Person')).toBe(`<${FOAF}Person>`);
  });

  it('declares every fallback prefix it uses, so the Turtle still parses', () => {
    const out = expectRoundTrip(SPARSE); // parses the output → undeclared prefix would throw
    expect(out).toContain(`@prefix foaf: <${FOAF}> .`);
    expect(out).toContain('sh:targetClass foaf:Person');
    expect(out).toContain('sh:path foaf:name');
    // and the shortened form is a fixpoint under re-parse
    expect(reser(out)).toBe(out);
  });

  it('refuses a local name that would terminate the Turtle statement', () => {
    // `foaf:Person.` re-parses as `foaf:Person` followed by the '.' that ends
    // the statement, so the emitted document stops parsing at that point.
    expect(makeCurie({})(FOAF + 'Person.')).toBe(`<${FOAF}Person.>`);
    const src = `@prefix sh: <http://www.w3.org/ns/shacl#> .
<http://example.org/S> a sh:NodeShape ;
  sh:targetClass <${FOAF}Person.> ;
  sh:property [ sh:path <${FOAF}name> ; sh:minCount 1 ] .
`;
    const out = expectRoundTrip(src); // re-parses the output → an early '.' would throw
    expect(out).toContain(`sh:targetClass <${FOAF}Person.>`);
    expect(out).toContain('sh:path foaf:name');
    // A trailing '-' is PN_CHARS and parses; it must keep shortening.
    expect(makeCurie({})(FOAF + 'knows-')).toBe('foaf:knows-');
  });

  it('keeps the round trip lossless when a label is shadowed', () => {
    const src = `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix foaf: <http://example.org/foaf/> .
<http://example.org/S> a sh:NodeShape ;
  sh:targetClass <${FOAF}Person> ;
  sh:property [ sh:path foaf:name ; sh:minCount 1 ] .
`;
    const out = expectRoundTrip(src);
    expect(out).toContain(`sh:targetClass <${FOAF}Person>`);
    expect(out).toContain('sh:path foaf:name');
  });
});

// Kept last: warming the prefix store is module-global state.
describe('CURIE resolution via the prefix service', () => {
  const QUDT = 'http://qudt.org/schema/qudt/';

  it('falls back to a service-loaded prefix and declares it', async () => {
    const src = `@prefix sh: <http://www.w3.org/ns/shacl#> .
<http://example.org/S> a sh:NodeShape ; sh:targetClass <${QUDT}Unit> .
`;
    // Cold store: the service namespace is unknown, so the IRI stays whole.
    expect(serializeShapesGraph(parseShapesGraph(src))).toContain(`<${QUDT}Unit>`);

    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true, json: async () => ({ qudt: QUDT }) }));
    await loadPrefixCcPrefixes();
    vi.unstubAllGlobals();

    const out = expectRoundTrip(src);
    expect(out).toContain('sh:targetClass qudt:Unit');
    expect(out).toContain(`@prefix qudt: <${QUDT}> .`);
  });
});

// ─── One label, two namespaces ───────────────────────────────────────────────
// `void` is the real-world collision: COMMON_PREFIXES binds it to
// http://www.w3.org/ns/void#, the shipped prefix snapshot to
// http://rdfs.org/ns/void#. Only the last fallback reaches the @prefix header,
// so emitting both as `void:…` rewrites one IRI into the other's namespace —
// and the output is still valid Turtle, so nothing downstream notices.

describe('colliding well-known labels', () => {
  const VOID_W3 = 'http://www.w3.org/ns/void#';
  const VOID_RDFS = 'http://rdfs.org/ns/void#';

  const SRC = `@prefix sh: <http://www.w3.org/ns/shacl#> .
<http://example.org/S> a sh:NodeShape ;
  sh:targetClass <${VOID_W3}Dataset> ;
  sh:property [ sh:path <http://example.org/p> ; sh:class <${VOID_RDFS}Linkset> ] .
`;

  it('keeps both namespaces intact through a serialise', () => {
    const out = expectRoundTrip(SRC); // quad-identical: neither IRI may move
    expect(out.match(/^@prefix void: /gm) || []).toHaveLength(1);
  });

  it('hands a label to one namespace only, whatever the resolver offers', () => {
    // Injected rather than taken from the prefix store: makeCurie is the last
    // place that can keep the emitted @prefix header honest, so it must hold
    // even when the resolver does offer one label for two namespaces.
    wkStub.override = (iri) => {
      for (const ns of [VOID_W3, VOID_RDFS])
        if (iri.startsWith(ns)) return { prefix: 'void', namespace: ns, local: iri.slice(ns.length) };
      return null;
    };
    try {
      const seen: [string, string][] = [];
      const curie = makeCurie({}, (label, ns) => seen.push([label, ns]));
      expect(curie(VOID_W3 + 'Dataset')).toBe('void:Dataset');
      // Same label, second namespace: `void:Linkset` would denote the first.
      expect(curie(VOID_RDFS + 'Linkset')).toBe(`<${VOID_RDFS}Linkset>`);
      // One report per label, so a caller collecting them into a flat
      // label → namespace map cannot lose the winner to the loser.
      expect(seen).toEqual([['void', VOID_W3]]);

      // End to end: the saved document must still mean what it meant.
      const out = expectRoundTrip(SRC);
      expect(out).toContain(`@prefix void: <${VOID_W3}> .`);
      expect(out).toContain(`<${VOID_RDFS}Linkset>`);
    } finally {
      wkStub.override = null;
    }
  });
});
