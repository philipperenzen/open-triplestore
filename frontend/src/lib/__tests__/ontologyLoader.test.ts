import { describe, it, expect } from 'vitest';
import { Parser, Store } from 'n3';
import { extractOntologyModel } from '../ontology/loader.ts';

type ShapeEntry = {
  iri: string;
  targetClass?: string[];
  properties: { path: string; name?: string; datatype?: string }[];
};
const shapesOf = (model: unknown): ShapeEntry[] => (model as { shapes: ShapeEntry[] }).shapes;

const PFX = `@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix ex: <http://example.org/> .
`;
const EX = 'http://example.org/';

const storeOf = (ttl: string): Store => {
  const store = new Store();
  store.addQuads(new Parser().parse(PFX + ttl));
  return store;
};

describe('extractOntologyModel — SHACL shapes', () => {
  it('keeps node shapes with attached property shapes (named and blank)', () => {
    const model = extractOntologyModel(storeOf(`
      ex:S a sh:NodeShape ; sh:targetClass ex:T ;
        sh:property [ sh:path ex:p1 ; sh:minCount 1 ] ;
        sh:property ex:Attached .
      ex:Attached a sh:PropertyShape ; sh:path ex:p2 .
    `));
    const s = shapesOf(model).find((x) => x.iri === EX + 'S');
    expect(s).toBeTruthy();
    expect(s!.targetClass).toEqual([EX + 'T']);
    expect(s!.properties.map((p) => p.path).sort()).toEqual([EX + 'p1', EX + 'p2']);
    // attached named property shapes don't become standalone shape entries
    expect(shapesOf(model).some((x) => x.iri === EX + 'Attached')).toBe(false);
  });

  it('indexes orphan property shapes as standalone entries', () => {
    const model = extractOntologyModel(storeOf(`
      ex:S a sh:NodeShape ; sh:targetClass ex:T ; sh:property [ sh:path ex:p1 ] .
      ex:Orphan a sh:PropertyShape ; sh:path ex:p2 ; sh:name "orphan" ; sh:datatype xsd:string .
    `));
    const orphan = shapesOf(model).find((x) => x.iri === EX + 'Orphan');
    expect(orphan).toBeTruthy();
    expect(orphan!.properties).toHaveLength(1);
    expect(orphan!.properties[0].path).toBe(EX + 'p2');
    expect(orphan!.properties[0].name).toBe('orphan');
    expect(orphan!.properties[0].datatype).toBe('http://www.w3.org/2001/XMLSchema#string');
  });

  it('does not duplicate property entries for attached shapes', () => {
    const model = extractOntologyModel(storeOf(`
      ex:S a sh:NodeShape ; sh:property ex:P .
      ex:P a sh:PropertyShape ; sh:path ex:p .
    `));
    const s = shapesOf(model).find((x) => x.iri === EX + 'S');
    expect(s!.properties).toHaveLength(1);
    expect(shapesOf(model).filter((x) => x.iri === EX + 'P')).toHaveLength(0);
  });
});
