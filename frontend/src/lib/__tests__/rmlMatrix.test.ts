import { describe, it, expect } from 'vitest';
import { parseRmlMatrix } from '../rmlMatrix.ts';

const MAPPING = `
@prefix rr:   <http://www.w3.org/ns/r2rml#> .
@prefix rml:  <http://semweb.mmlab.be/ns/rml#> .
@prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .
@prefix fnml: <http://semweb.mmlab.be/ns/fnml#> .
@prefix fno:  <https://w3id.org/function/ontology#> .
@prefix otsfn: <https://w3id.org/open-triplestore/fn#> .
@prefix ex:   <http://example.org/> .

ex:ProductsMap a rr:TriplesMap ;
  rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName "products" ] ;
  rr:subjectMap [ rr:template "http://example.org/p{product_id}" ; rr:class ex:Product ] ;
  rr:predicateObjectMap [ rr:predicate ex:name ; rr:objectMap [ rr:column "name" ] ] ;
  rr:predicateObjectMap [ rr:predicate ex:hasPrice ; rr:objectMap [ rr:column "price" ; rr:datatype xsd:decimal ] ] ;
  rr:predicateObjectMap [ rr:predicate ex:kind ; rr:object ex:Thing ] ;
  rr:predicateObjectMap [ rr:predicate ex:suppliedBy ; rr:objectMap [
      rr:parentTriplesMap ex:SuppliersMap ; rr:joinCondition [ rr:child "supplier_id" ; rr:parent "supplier_id" ] ] ] ;
  rr:predicateObjectMap [ rr:predicate ex:hasStatus ; rr:objectMap [ fnml:functionValue [
      rr:predicateObjectMap [ rr:predicate fno:executes ; rr:object otsfn:mapValue ] ;
      rr:predicateObjectMap [ rr:predicate otsfn:value ; rr:objectMap [ rr:column "status" ] ] ] ] ] ;
  rr:predicateObjectMap [ rr:predicate ex:inCategory ; rr:objectMap [ fnml:functionValue [
      rr:predicateObjectMap [ rr:predicate fno:executes ; rr:object otsfn:mintIri ] ;
      rr:predicateObjectMap [ rr:predicate otsfn:template ; rr:object "http://example.org/c/{category_slug}" ] ] ] ] .

ex:SuppliersMap a rr:TriplesMap ;
  rml:logicalSource [ rml:source <urn:source:s> ; rml:query "SELECT * FROM suppliers" ] ;
  rr:subjectMap [ fnml:functionValue [
      rr:predicateObjectMap [ rr:predicate fno:executes ; rr:object otsfn:mintIri ] ;
      rr:predicateObjectMap [ rr:predicate otsfn:template ; rr:object "http://example.org/s/{name_slug}" ] ] ;
    rr:class ex:Supplier ] .
`;

describe('parseRmlMatrix', () => {
  const maps = parseRmlMatrix(MAPPING);

  it('finds every triples map, sorted, with what it reads and mints', () => {
    expect(maps.map((m) => m.name)).toEqual(['ProductsMap', 'SuppliersMap']);
    const [products, suppliers] = maps;
    expect(products.table).toBe('products');
    expect(products.query).toBeNull();
    expect(products.subjectKind).toBe('template');
    expect(products.subject).toBe('http://example.org/p{product_id}');
    expect(products.classes).toEqual(['http://example.org/Product']);
    expect(suppliers.query).toBe('SELECT * FROM suppliers');
    expect(suppliers.subjectKind).toBe('function');
    expect(suppliers.subject).toBe('otsfn:mintIri(http://example.org/s/{name_slug})');
  });

  it('describes each object in a word', () => {
    const rows = Object.fromEntries(maps[0].rows.map((r) => [r.predicate.replace('http://example.org/', ''), r]));
    expect(rows.name).toMatchObject({ kind: 'column', detail: 'name', datatype: null });
    expect(rows.hasPrice).toMatchObject({ kind: 'column', detail: 'price', datatype: 'decimal' });
    expect(rows.kind).toMatchObject({ kind: 'constant', detail: 'http://example.org/Thing' });
    expect(rows.suppliedBy).toMatchObject({
      kind: 'parent',
      detail: 'SuppliersMap',
      parentMap: 'http://example.org/SuppliersMap',
      joins: [{ child: 'supplier_id', parent: 'supplier_id' }],
    });
    expect(rows.hasStatus).toMatchObject({ kind: 'function', detail: 'otsfn:mapValue(status)' });
    expect(rows.inCategory).toMatchObject({ kind: 'function', detail: 'otsfn:mintIri(http://example.org/c/{category_slug})' });
  });

  it('is empty for input that is not Turtle', () => {
    expect(parseRmlMatrix('this is not turtle')).toEqual([]);
    expect(parseRmlMatrix('')).toEqual([]);
  });
});
