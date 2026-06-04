import { describe, it, expect } from 'vitest';
import { Parser, Store } from 'n3';
import { validateSemantics } from '../ontology/semanticValidator.ts';

const PFX = `@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix ex: <http://example.org/> .
`;

/** Parse Turtle into an n3 Store the same way the loader does. */
const storeOf = (ttl: string): Store => {
  const store = new Store();
  store.addQuads(new Parser().parse(PFX + ttl));
  return store;
};

const codes = (ttl: string) => validateSemantics(storeOf(ttl)).map(i => i.code);

describe('validateSemantics', () => {
  it('does not throw on an empty store', () => {
    expect(() => validateSemantics(new Store())).not.toThrow();
    expect(validateSemantics(new Store())).toEqual([]);
  });

  it('flags an rdfs:subClassOf cycle as an error', () => {
    const issues = validateSemantics(storeOf(`
      ex:A a owl:Class ; rdfs:subClassOf ex:B .
      ex:B a owl:Class ; rdfs:subClassOf ex:A .
    `));
    const cycle = issues.find(i => i.code === 'subclass-cycle');
    expect(cycle).toBeTruthy();
    expect(cycle!.severity).toBe('error');
  });

  it('flags a property typed as both Object and Datatype property', () => {
    expect(codes(`ex:p a owl:ObjectProperty , owl:DatatypeProperty .`))
      .toContain('property-kind-conflict');
  });

  it('flags sh:minCount greater than sh:maxCount', () => {
    const issues = validateSemantics(storeOf(`
      ex:S a sh:NodeShape ; sh:targetClass ex:Thing ;
        sh:property [ sh:path ex:n ; sh:minCount 5 ; sh:maxCount 2 ] .
      ex:Thing a owl:Class . ex:n a owl:DatatypeProperty .
    `));
    const minmax = issues.find(i => i.code === 'shacl-min-gt-max');
    expect(minmax).toBeTruthy();
    expect(minmax!.severity).toBe('error');
  });

  it('flags a property shape with both sh:datatype and sh:class', () => {
    expect(codes(`
      ex:S a sh:NodeShape ; sh:targetClass ex:Thing ;
        sh:property [ sh:path ex:n ; sh:datatype xsd:string ; sh:class ex:Other ] .
      ex:Thing a owl:Class .
    `)).toContain('shacl-datatype-and-class');
  });

  it('flags rdfs:domain pointing at an undeclared class (warning), but not xsd ranges', () => {
    const cs = codes(`
      ex:p a owl:ObjectProperty ; rdfs:domain ex:Ghost .
      ex:age a owl:DatatypeProperty ; rdfs:range xsd:integer .
    `);
    expect(cs).toContain('domain-unknown-class');
    // xsd:integer range must NOT be flagged as an unknown class.
    expect(cs).not.toContain('range-unknown-class');
  });

  it('flags a literal where an IRI is expected', () => {
    const issues = validateSemantics(storeOf(`ex:A a owl:Class ; rdfs:subClassOf "not-an-iri" .`));
    const lit = issues.find(i => i.code === 'literal-where-iri-expected');
    expect(lit).toBeTruthy();
    expect(lit!.severity).toBe('error');
  });

  it('sorts errors ahead of warnings and info', () => {
    const issues = validateSemantics(storeOf(`
      ex:A a owl:Class ; rdfs:subClassOf ex:B .
      ex:B a owl:Class ; rdfs:subClassOf ex:A .
      ex:p a owl:ObjectProperty ; rdfs:domain ex:Ghost .
    `));
    const firstWarnIdx = issues.findIndex(i => i.severity === 'warning');
    const lastErrIdx = issues.map(i => i.severity).lastIndexOf('error');
    expect(lastErrIdx).toBeLessThan(firstWarnIdx === -1 ? Infinity : firstWarnIdx);
  });
});
