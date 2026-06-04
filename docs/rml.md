# RML Mapping Guide

The triplestore supports the [RDF Mapping Language (RML)](https://rml.io/specs/rml/) for converting tabular and semi-structured data (CSV, JSON, XML) into RDF triples.

---

## Overview

RML extends W3C R2RML to non-relational data sources. A mapping document is a Turtle file that describes:

- **`rml:LogicalSource`** — where data comes from (file name, reference formulation)
- **`rr:TriplesMap`** — how each row maps to a set of RDF triples
- **`rr:SubjectMap`** — how to construct the subject IRI or blank node
- **`rr:PredicateObjectMap`** — how to construct predicate-object pairs

---

## Supported Features

| Feature | Support |
|---|---|
| `ql:CSV` (CSV source) | Full — header-based column references |
| `ql:JSONPath` (JSON source) | Iterator path + flat object key references |
| `ql:XPath` (XML source) | Simple element path + child text content |
| `rr:template` | Full — `{column}` expansion with percent-encoding |
| `rml:reference` / `rr:column` | Full — direct column lookup |
| `rr:constant` | Full |
| `rr:class` | Full — adds `rdf:type` to every generated subject |
| `rr:termType` | IRI, BlankNode, Literal |
| `rr:datatype` | Full |
| `rr:language` | Full |
| `rr:graphMap` | Supported on TriplesMap and PredicateObjectMap |
| `rr:subjectMap` shortcut (`rr:subject`) | Supported |
| `rr:predicateMap` shortcut (`rr:predicate`) | Supported |
| `rr:objectMap` shortcut (`rr:object`) | Supported |

---

## API Endpoints

### Store a mapping

```bash
curl -X PUT http://localhost:7878/api/datasets/<dataset_id>/mappings \
     -H 'Authorization: Bearer <token>' \
     -H 'Content-Type: text/turtle' \
     --data-binary @mapping.ttl
# → 204 No Content
```

The mapping is validated on upload. A `400 Bad Request` is returned if the mapping is not valid RML.

### Retrieve the stored mapping

```bash
curl http://localhost:7878/api/datasets/<dataset_id>/mappings \
     -H 'Authorization: Bearer <token>'
# → text/turtle
```

### Execute the mapping

Source files are supplied as multipart form parts. The part name must match the `rml:source` value in the mapping.

```bash
curl -X POST http://localhost:7878/api/datasets/<dataset_id>/mappings/execute \
     -H 'Authorization: Bearer <token>' \
     -F 'people.csv=@people.csv' \
     -F 'orders.json=@orders.json'
# → {"triples_inserted": 420, "target_graph": "urn:dataset:<id>:rml-output"}
```

**Query parameters:**

| Parameter | Default | Description |
|---|---|---|
| `preview=true` | `false` | Return generated triples without persisting |
| `graph=<iri>` | `urn:dataset:<id>:rml-output` | Override the target named graph |

### Preview without persisting

```bash
curl -X POST 'http://localhost:7878/api/datasets/<dataset_id>/mappings/execute?preview=true' \
     -H 'Authorization: Bearer <token>' \
     -F 'people.csv=@people.csv'
# → {"preview": true, "triples_count": 50, "turtle": "@prefix ..."}
```

### Standalone preview (no dataset required)

```bash
curl -X POST http://localhost:7878/api/rml/preview \
     -F 'mapping=@mapping.ttl' \
     -F 'people.csv=@people.csv'
# → {"triples_count": 50, "turtle": "..."}
```

The `mapping` part name is reserved for the mapping document. All other parts are source files.

---

## CSV Source

Reference formulation: `ql:CSV`

Column references use the header name exactly as it appears in the CSV file.

### Example mapping

```turtle
@prefix rr:  <http://www.w3.org/ns/r2rml#> .
@prefix rml: <http://semweb.mmlab.be/ns/rml#> .
@prefix ql:  <http://semweb.mmlab.be/ns/ql#> .
@prefix ex:  <http://example.org/> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

<#PersonMap>
  a rr:TriplesMap ;
  rml:logicalSource [
    rml:source "people.csv" ;
    rml:referenceFormulation ql:CSV
  ] ;
  rr:subjectMap [
    rr:template "http://example.org/person/{id}" ;
    rr:class ex:Person
  ] ;
  rr:predicateObjectMap [
    rr:predicate ex:name ;
    rr:objectMap [ rml:reference "name" ]
  ] ;
  rr:predicateObjectMap [
    rr:predicate ex:age ;
    rr:objectMap [
      rml:reference "age" ;
      rr:datatype xsd:integer
    ]
  ] ;
  rr:predicateObjectMap [
    rr:predicate ex:email ;
    rr:objectMap [ rml:reference "email" ]
  ] .
```

### Example CSV (`people.csv`)

```
id,name,age,email
1,Alice,30,alice@example.org
2,Bob,25,bob@example.org
```

### Generated triples

```turtle
<http://example.org/person/1> a ex:Person ;
    ex:name "Alice" ;
    ex:age "30"^^xsd:integer ;
    ex:email "alice@example.org" .

<http://example.org/person/2> a ex:Person ;
    ex:name "Bob" ;
    ex:age "25"^^xsd:integer ;
    ex:email "bob@example.org" .
```

---

## JSON Source

Reference formulation: `ql:JSONPath`

Use `rml:iterator` to select the array to iterate over (supports simple `$.key` or `$.key[*]` paths). References access keys of the current object.

### Example mapping

```turtle
<#OrderMap>
  a rr:TriplesMap ;
  rml:logicalSource [
    rml:source "orders.json" ;
    rml:referenceFormulation ql:JSONPath ;
    rml:iterator "$.orders"
  ] ;
  rr:subjectMap [
    rr:template "http://example.org/order/{orderId}"
  ] ;
  rr:predicateObjectMap [
    rr:predicate ex:amount ;
    rr:objectMap [
      rml:reference "amount" ;
      rr:datatype xsd:decimal
    ]
  ] ;
  rr:predicateObjectMap [
    rr:predicate ex:customer ;
    rr:objectMap [
      rr:template "http://example.org/person/{customerId}" ;
      rr:termType rr:IRI
    ]
  ] .
```

### Example JSON (`orders.json`)

```json
{
  "orders": [
    {"orderId": "O1", "amount": 99.50, "customerId": "1"},
    {"orderId": "O2", "amount": 14.00, "customerId": "2"}
  ]
}
```

---

## XML Source

Reference formulation: `ql:XPath`

Use `rml:iterator` as a simple element path (e.g. `/people/person`). Each matching element is a row; child element names are column references.

### Example mapping

```turtle
<#PersonXmlMap>
  a rr:TriplesMap ;
  rml:logicalSource [
    rml:source "people.xml" ;
    rml:referenceFormulation ql:XPath ;
    rml:iterator "/people/person"
  ] ;
  rr:subjectMap [
    rr:template "http://example.org/person/{id}"
  ] ;
  rr:predicateObjectMap [
    rr:predicate ex:name ;
    rr:objectMap [ rml:reference "name" ]
  ] .
```

### Example XML (`people.xml`)

```xml
<people>
  <person>
    <id>1</id>
    <name>Alice</name>
  </person>
  <person>
    <id>2</id>
    <name>Bob</name>
  </person>
</people>
```

---

## Template Expansion

In `rr:template` strings, `{column}` placeholders are replaced with the column value, percent-encoded for safe IRI inclusion. Columns not found in the row cause the triple to be silently skipped.

```turtle
# Template: "http://example.org/product/{sku}/{variant}"
# Row: {sku: "ABC 123", variant: "red"}
# Result: <http://example.org/product/ABC%20123/red>
```

---

## Named Graphs

To write generated triples into a specific named graph, use `rr:graphMap` on the TriplesMap or PredicateObjectMap:

```turtle
<#PersonMap>
  rr:subjectMap [ rr:template "http://example.org/person/{id}" ] ;
  rr:graphMap [ rr:constant <http://example.org/people-graph> ] ;
  ...
```

The `?graph=<iri>` query parameter on the execute endpoint overrides the default output graph (`urn:dataset:<id>:rml-output`) globally, but per-TriplesMap `rr:graphMap` takes precedence for individual triples.

---

## Storage

Mappings are stored in the named graph `urn:dataset:<id>:rml-mappings` and are automatically registered in the dataset's graph list. Generated output goes to `urn:dataset:<id>:rml-output` unless overridden.

Both graphs appear in the dataset graph list and participate in dataset-scoped SPARQL queries.

---

## Limitations

- **Joins between TriplesMap entries**: RML join conditions (`rr:joinCondition`) are not yet supported. Denormalize source data before mapping or use separate SPARQL UPDATE statements to add cross-reference links.
- **SQL / SPARQL sources**: Only file-based sources (CSV, JSON, XML) are supported. R2RML SQL source and SPARQL-based sources are not implemented.
- **Large files**: Source files are read entirely into memory. For very large files (> 100 MB), consider splitting them before upload.
- **Nested JSON/XML**: Deep nesting (e.g. accessing `$.orders[].items[].price`) requires the iterator to point to the innermost array. Nested sibling references are flattened at a single object level.
