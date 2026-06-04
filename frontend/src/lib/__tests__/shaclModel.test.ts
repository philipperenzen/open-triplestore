import { describe, it, expect } from 'vitest';
import { parseShapesGraph, serializeShapesGraph } from '../shaclModel.ts';

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

  it.each([
    ['logical or', `ex:S a sh:NodeShape ; sh:or ( [ sh:path ex:a ] [ sh:path ex:b ] ) .`],
    ['sparql constraint', `ex:S a sh:NodeShape ; sh:sparql [ sh:select "SELECT $this {}" ] .`],
    ['path expression', `ex:S a sh:NodeShape ; sh:property [ sh:path ( ex:a ex:b ) ] .`],
    [
      'named property shape',
      `ex:S a sh:NodeShape ; sh:property ex:NameProp . ex:NameProp a sh:PropertyShape ; sh:path ex:name .`,
    ],
    [
      'qualified shape',
      `ex:S a sh:NodeShape ; sh:property [ sh:path ex:k ; sh:qualifiedValueShape [ sh:class ex:P ] ; sh:qualifiedMinCount 1 ] .`,
    ],
  ])('flags advanced SHACL (%s) as read-only (canRoundTrip=false)', (_label, body) => {
    expect(parseShapesGraph(PFX + body).canRoundTrip).toBe(false);
  });

  it('flags a typed (date) range bound as read-only', () => {
    const g = parseShapesGraph(
      PFX + `ex:S a sh:NodeShape ; sh:targetClass ex:T ; sh:property [ sh:path ex:when ; sh:minInclusive "2020-01-01"^^xsd:date ] .`,
    );
    expect(g.canRoundTrip).toBe(false);
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
