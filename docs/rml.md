# RML Mapping Guide

The triplestore supports the [RDF Mapping Language (RML)](https://rml.io/specs/rml/) for converting tabular and semi-structured data (CSV, JSON, XML) into RDF triples.

This page covers mappings over uploaded **files**. Relational databases (PostgreSQL, MySQL / MariaDB, SQL Server) and SPARQL endpoints are mapped as registered datasources instead — see [Sources](sources.md) — and that path also resolves joins between triples maps.

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
| `rr:template` | Full — `{column}` expansion; IRI-safe encoding (R2RML §7.3) when the term is an IRI, the value as it is otherwise |
| `rml:reference` / `rr:column` | Full — direct column lookup |
| `rr:constant` | Full — the term is the constant itself: an IRI stays an IRI, a literal keeps its datatype and language tag (R2RML §7.4) |
| `rr:class` | Full — adds `rdf:type` to every generated subject, in the subject's graphs |
| `rr:termType` | IRI, BlankNode, Literal, with R2RML's defaults — see [Term types](#term-types) |
| `rr:datatype` | Full |
| `rr:language` | Full |
| `rr:graphMap` / `rr:graph` | On the subject map and on predicate-object maps; a triple goes to the union of both, `rr:defaultGraph` names the default graph — see [Named Graphs](#named-graphs) |
| Blank nodes | One per value and graph (R2RML §11.2, §9.1) |
| Base IRI | `?base=` on the execute endpoint, or `rml:baseIRI` on a triples map (RML-Core) |
| `rr:subjectMap` shortcut (`rr:subject`) | Supported |
| `rr:predicateMap` shortcut (`rr:predicate`) | Supported |
| `rr:objectMap` shortcut (`rr:object`) | Supported |
| `rr:parentTriplesMap` (referencing object maps, joins) | Not on file sources: a mapping that uses one is refused with `400` and names the triples map. Registered datasources resolve them ([Sources](sources.md)). |

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
| `base=<iri>` | none | Base IRI that relative IRIs resolve against; a triples map's `rml:baseIRI` wins |

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

In `rr:template` strings, `{column}` placeholders are replaced with the column value. When the term is an IRI the value is made **IRI-safe** (R2RML §7.3): every character outside RFC 3987 `iunreserved` — letters, digits, `-`, `.`, `_`, `~` and non-ASCII letters — is UTF-8 percent-encoded. A literal or blank-node template takes the value as it is. A column not found in the row skips the term, and so the triple.

```turtle
# Template: "http://example.org/product/{sku}/{variant}"
# Row: {sku: "ABC 123", variant: "~red-2.0"}
# Result: <http://example.org/product/ABC%20123/~red-2.0>
```

A value that is not an absolute IRI — from a template, a column with `rr:termType rr:IRI`, or a relative constant — is appended to the **base IRI** (R2RML §11.2): the triples map's `rml:baseIRI`, else the `?base=` of the run. Without one it generates no term.

---

## Term types

An explicit `rr:termType` wins. Without one, R2RML §7.4 decides:

| Term map | Generates |
|---|---|
| Subject, predicate or graph map | An IRI |
| Object map reading a column (`rml:reference` / `rr:column`) | A literal |
| Object map with `rr:language` or `rr:datatype` | A literal |
| Any other object map — a template | An IRI |
| A constant (`rr:constant`, `rr:object`, …) | The constant itself; `rr:termType` has no effect |

So `rr:objectMap [ rr:template "Hello {name}" ]` is an IRI; write `rr:termType rr:Literal` for text.

A **blank node** is one per value within a graph: two rows that generate the same value share a node, and the same value in another graph is another node. Labels are derived from the run, the graph and the value, so they never collide with another run's.

---

## Named Graphs

Graph maps sit on the **subject map** (every triple of the subject, `rr:class` included) and on **predicate-object maps**, as `rr:graphMap` or the constant shortcut `rr:graph`. A triple goes to the union of its subject map's and its predicate-object map's graphs (R2RML §11.1); a graph map that generates `rr:defaultGraph` names the default graph, and with no graph map anywhere a triple goes to the default graph.

```turtle
<#PersonMap>
  rr:subjectMap [ rr:template "http://example.org/person/{id}" ;
                  rr:graph <http://example.org/people-graph> ] ;
  rr:predicateObjectMap [ rr:predicate ex:email ;
                          rr:objectMap [ rml:reference "email" ] ;
                          rr:graph <http://example.org/contact-graph> ] ;
  ...
```

Here `ex:email` triples land in both graphs. The default graph is the execute endpoint's output graph — `urn:dataset:<id>:rml-output`, or `?graph=<iri>`. A graph map on the triples map itself is read as a subject graph map. Every destination graph is checked against the dataset's boundary before anything is written.

### Mappings written before these rules

The term rules above are R2RML's. Before this engine followed them, a template object map defaulted to a literal, every template value had all but its letters and digits percent-encoded (`-` became `%2D`), and blank nodes were minted per row. A mapping stored here is re-read on every run, so it now runs under R2RML's rules. A registered datasource mapping keeps the rules of the version it was frozen with — see [Sources](sources.md#term-generation-rules).

---

## Storage

Mappings are stored in the named graph `urn:dataset:<id>:rml-mappings` and are automatically registered in the dataset's graph list. Generated output goes to `urn:dataset:<id>:rml-output` unless overridden.

Both graphs appear in the dataset graph list and participate in dataset-scoped SPARQL queries.

---

## Limitations

- **Joins between TriplesMap entries**: a file row stands alone, so `rr:parentTriplesMap` (with or without `rr:joinCondition`) cannot be resolved here and the whole mapping is refused rather than run without its links. Put the parent's key in the child's rows and build the object with `rr:template`, or load the data as a registered datasource, where joins run ([Sources](sources.md)).
- **SQL / SPARQL sources**: this upload path reads files only. SQL logical tables (`rr:tableName`, `rml:query`) and SPARQL endpoints are mapped through registered datasources ([Sources](sources.md)); a mapping that names one is refused here with a pointer to that path.
- **Empty values**: an empty CSV cell, JSON string or XML text generates no term, as a NULL does. RML-IO treats only `rml:null` values as NULL; that is not implemented yet.
- **One predicate and one object per map**: a predicate-object map uses its first `rr:predicateMap` / `rr:predicate` and its first `rr:objectMap` / `rr:object`; write one predicate-object map per pair.
- **Large files**: Source files are read entirely into memory. For very large files (> 100 MB), consider splitting them before upload.
- **Nested JSON/XML**: Deep nesting (e.g. accessing `$.orders[].items[].price`) requires the iterator to point to the innermost array. Nested sibling references are flattened at a single object level.
