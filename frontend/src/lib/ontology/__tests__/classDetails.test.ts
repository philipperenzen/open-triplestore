// The class detail pane lists a class's attributes, relations, shape
// constraints and instances. These tests pin how extractSchema's lookup tables
// and classDetails() combine rdfs:domain/range declarations, SHACL shapes and
// the class hierarchy.
import { describe, it, expect } from 'vitest';
import { Parser, Store } from 'n3';
import { extractSchema } from '../schema-model';
import { classDetails, classInstances } from '../class-details';

const EX = 'http://example.org/ns#';
const XSD = 'http://www.w3.org/2001/XMLSchema#';

function model(ttl: string) {
  const store = new Store(new Parser().parse(`
    @prefix ex:   <${EX}> .
    @prefix rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
    @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
    @prefix owl:  <http://www.w3.org/2002/07/owl#> .
    @prefix sh:   <http://www.w3.org/ns/shacl#> .
    @prefix xsd:  <${XSD}> .
    ${ttl}`));
  return extractSchema(store);
}

const HIERARCHY = `
  ex:Structure a owl:Class ; rdfs:label "Structure" .
  ex:Bridge a owl:Class ; rdfs:label "Bridge" ; rdfs:subClassOf ex:Structure .
  ex:SteelBridge a owl:Class ; rdfs:subClassOf ex:Bridge .
  ex:River a owl:Class ; rdfs:label "River" .
  ex:Road a owl:Class .

  ex:name a owl:DatatypeProperty, owl:FunctionalProperty ; rdfs:label "name" ;
    rdfs:domain ex:Structure ; rdfs:range xsd:string .
  ex:span a owl:DatatypeProperty ; rdfs:domain ex:Bridge ; rdfs:range xsd:decimal .
  ex:crosses a owl:ObjectProperty ; rdfs:label "crosses" ;
    rdfs:domain ex:Bridge ; rdfs:range ex:River .
  ex:partOf a owl:ObjectProperty ; rdfs:domain ex:Structure ; rdfs:range ex:Road .
  ex:carries a owl:ObjectProperty ; rdfs:domain ex:Road ; rdfs:range ex:Bridge .
`;

const iris = (rows: { iri: string }[]) => rows.map(r => r.iri);

describe('classDetails: declared properties', () => {
  it('lists own attributes and relations', () => {
    const d = classDetails(model(HIERARCHY), EX + 'Bridge');
    const own = d.attributes.filter(r => r.origin === EX + 'Bridge');
    expect(iris(own)).toEqual([EX + 'span']);
    expect(own[0].targets).toEqual([XSD + 'decimal']);
    expect(own[0].declared).toBe(true);

    const crosses = d.outgoing.find(r => r.iri === EX + 'crosses')!;
    expect(crosses.origin).toBe(EX + 'Bridge');
    expect(crosses.kind).toBe('relation');
    expect(crosses.targets).toEqual([EX + 'River']);
  });

  it('lists inherited properties after own ones, marked with their ancestor', () => {
    const d = classDetails(model(HIERARCHY), EX + 'SteelBridge');
    expect(d.ancestors).toEqual([EX + 'Bridge', EX + 'Structure']);
    expect(d.attributes.map(r => [r.iri, r.origin])).toEqual([
      [EX + 'span', EX + 'Bridge'],
      [EX + 'name', EX + 'Structure'],
    ]);
    expect(d.attributes.find(r => r.iri === EX + 'name')!.functional).toBe(true);
    expect(d.outgoing.map(r => [r.iri, r.origin])).toEqual([
      [EX + 'crosses', EX + 'Bridge'],
      [EX + 'partOf', EX + 'Structure'],
    ]);
  });

  it('lists incoming relations, own and inherited', () => {
    const river = classDetails(model(HIERARCHY), EX + 'River');
    expect(river.incoming.map(r => [r.iri, r.sources, r.origin])).toEqual([
      [EX + 'crosses', [EX + 'Bridge'], EX + 'River'],
    ]);
    const steel = classDetails(model(HIERARCHY), EX + 'SteelBridge');
    expect(steel.incoming.map(r => [r.iri, r.sources, r.origin])).toEqual([
      [EX + 'carries', [EX + 'Road'], EX + 'Bridge'],
    ]);
  });

  it('picks up members of an owl:unionOf domain', () => {
    const m = model(`
      ex:Bridge a owl:Class . ex:Tunnel a owl:Class .
      ex:length a owl:DatatypeProperty ;
        rdfs:domain [ a owl:Class ; owl:unionOf ( ex:Bridge ex:Tunnel ) ] .
    `);
    for (const c of ['Bridge', 'Tunnel']) {
      const row = classDetails(m, EX + c).attributes[0];
      expect(row.iri).toBe(EX + 'length');
      expect(row.viaUnion).toBe(true);
    }
    // The stated domain itself is not rewritten.
    expect(m.properties.get(EX + 'length')!.domain).toEqual([]);
    expect(m.properties.get(EX + 'length')!.domainUnion).toEqual([EX + 'Bridge', EX + 'Tunnel']);
  });

  it('survives a subclass cycle', () => {
    const m = model(`
      ex:A a owl:Class ; rdfs:subClassOf ex:B .
      ex:B a owl:Class ; rdfs:subClassOf ex:A .
      ex:p a owl:DatatypeProperty ; rdfs:domain ex:B .
    `);
    const d = classDetails(m, EX + 'A');
    expect(d.ancestors).toEqual([EX + 'B']);
    expect(iris(d.attributes)).toEqual([EX + 'p']);
  });
});

describe('classDetails: shapes', () => {
  const SHAPES = `
    ex:Bridge a owl:Class . ex:River a owl:Class .
    ex:name a owl:DatatypeProperty ; rdfs:domain ex:Bridge .
    ex:BridgeShape a sh:NodeShape ;
      sh:targetClass ex:Bridge ;
      sh:property [ sh:path ex:name ; sh:minCount 1 ; sh:maxCount 1 ; sh:datatype xsd:string ] ;
      sh:property [ sh:path ex:code ; sh:name "code" ; sh:pattern "^[A-Z]+$" ;
                    sh:in ( "A" "B" "C" ) ] ;
      sh:property [ sh:path ex:over ; sh:class ex:River ; sh:minCount 1 ] ;
      sh:property [ sh:path ( ex:over ex:name ) ; sh:maxCount 1 ] .
  `;

  it('merges a shape into the declared row instead of listing it twice', () => {
    const d = classDetails(model(SHAPES), EX + 'Bridge');
    const name = d.attributes.filter(r => r.iri === EX + 'name');
    expect(name).toHaveLength(1);
    expect(name[0].declared).toBe(true);
    expect(name[0].shapes).toEqual([EX + 'BridgeShape']);
    expect([name[0].minCount, name[0].maxCount]).toEqual(['1', '1']);
    expect(name[0].targets).toEqual([XSD + 'string']);
  });

  it('shows shape-only properties: sh:class makes a relation, otherwise an attribute', () => {
    const d = classDetails(model(SHAPES), EX + 'Bridge');
    const code = d.attributes.find(r => r.iri === EX + 'code')!;
    expect(code.declared).toBe(false);
    expect(code.propKind).toBeNull();
    expect(code.name).toBe('code');
    expect(code.inCount).toBe(3);
    expect(code.pattern).toBe('^[A-Z]+$');

    const over = d.outgoing.find(r => r.iri === EX + 'over')!;
    expect(over.kind).toBe('relation');
    expect(over.targets).toEqual([EX + 'River']);
    expect(d.attributes.some(r => r.iri === EX + 'over')).toBe(false);
  });

  it('keeps complex paths apart and reports the applied shape', () => {
    const d = classDetails(model(SHAPES), EX + 'Bridge');
    expect(d.shapes.map(s => [s.iri, s.implicit, s.propertyCount])).toEqual([[EX + 'BridgeShape', false, 4]]);
    expect(d.pathConstraints).toHaveLength(1);
    expect(d.pathConstraints[0].prop.pathExpr).toEqual({
      kind: 'sequence',
      paths: [{ kind: 'predicate', iri: EX + 'over' }, { kind: 'predicate', iri: EX + 'name' }],
    });
  });

  it('turns a shape sh:class into an incoming relation on the target class', () => {
    const d = classDetails(model(SHAPES), EX + 'River');
    expect(d.incoming.map(r => [r.iri, r.sources, r.shapes])).toEqual([
      [EX + 'over', [EX + 'Bridge'], [EX + 'BridgeShape']],
    ]);
  });

  it('applies a shape that is itself the class (implicit class target)', () => {
    const m = model(`
      ex:Bridge a owl:Class, sh:NodeShape ;
        sh:property [ sh:path ex:span ; sh:datatype xsd:decimal ; sh:maxCount 1 ] .
      ex:Pier a owl:Class ; rdfs:subClassOf ex:Bridge .
    `);
    expect(m.classes.get(EX + 'Bridge')!.hasShape).toBe(true);
    const own = classDetails(m, EX + 'Bridge');
    expect(own.shapes.map(s => [s.iri, s.implicit])).toEqual([[EX + 'Bridge', true]]);
    expect(iris(own.attributes)).toEqual([EX + 'span']);
    // Shapes on a superclass constrain its subclasses' instances too.
    const sub = classDetails(m, EX + 'Pier');
    expect(sub.attributes.map(r => [r.iri, r.origin])).toEqual([[EX + 'span', EX + 'Bridge']]);
  });

  it('does not treat a shape as an implicit target when the IRI is only a superclass', () => {
    const m = model(`
      ex:Pier a owl:Class ; rdfs:subClassOf ex:BridgeShape .
      ex:BridgeShape a sh:NodeShape ; sh:property [ sh:path ex:span ] .
    `);
    expect(m.shapes.get(EX + 'BridgeShape')!.implicitTarget).toBe(false);
  });
});

describe('classInstances', () => {
  const DATA = `
    ex:Bridge a owl:Class . ex:SteelBridge a owl:Class ; rdfs:subClassOf ex:Bridge .
    ex:b1 a ex:Bridge . ex:b2 a ex:Bridge .
    ex:s1 a ex:SteelBridge . ex:b2 a ex:SteelBridge .
  `;

  it('lists direct instances, and with subclasses without duplicates', () => {
    const m = model(DATA);
    const r = classInstances(m, EX + 'Bridge');
    expect(r.direct.sort()).toEqual([EX + 'b1', EX + 'b2']);
    expect(r.withSubclasses.sort()).toEqual([EX + 'b1', EX + 'b2', EX + 's1']);
  });

  it('returns the direct list for a class without subclasses', () => {
    const r = classInstances(model(DATA), EX + 'SteelBridge');
    expect(r.direct.sort()).toEqual([EX + 'b2', EX + 's1']);
    expect(r.withSubclasses).toBe(r.direct);
  });

  it('is empty for an unknown class', () => {
    expect(classInstances(model(DATA), EX + 'Nope')).toEqual({ direct: [], withSubclasses: [] });
  });
});
