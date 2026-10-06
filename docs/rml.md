# RML Mapping Guide

The triplestore supports the [RDF Mapping Language (RML)](https://rml.io/specs/rml/) for converting tabular and semi-structured data (CSV, JSON, XML) into RDF triples.

This page covers mappings over uploaded **files**. Relational databases (PostgreSQL, MySQL / MariaDB, SQL Server) and SPARQL endpoints are mapped as registered datasources instead — see [Sources](sources.md). Joins between triples maps run on both paths, by the same rules.

---

## Overview

RML extends W3C R2RML to non-relational data sources. A mapping document is a Turtle file that describes:

- **`rml:LogicalSource`** — where data comes from (file name, reference formulation)
- **`rr:TriplesMap`** — how each row maps to a set of RDF triples
- **`rr:SubjectMap`** — how to construct the subject IRI or blank node
- **`rr:PredicateObjectMap`** — how to construct predicate-object pairs

### Vocabularies

A mapping may be written in any of three vocabularies, and may mix them:

| Vocabulary | Namespace | What it covers |
|---|---|---|
| R2RML | `http://www.w3.org/ns/r2rml#` (`rr:`) | triples maps, term maps, joins, graphs |
| Legacy RML | `http://semweb.mmlab.be/ns/rml#` (`rml:`), formulations in `http://semweb.mmlab.be/ns/ql#` (`ql:`) | logical sources, references, iterators |
| RML-Core / RML-IO | `http://w3id.org/rml/` (`rml:`) | all of the above in one namespace, plus language and datatype maps, join expression maps, `rml:URI` / `rml:UnsafeIRI` / `rml:UnsafeURI`, source descriptions, encodings, compression |

Two things depend on the vocabulary. A JSON value read by an RML-Core
mapping carries its **natural datatype** (a number without a fraction is an
`xsd:integer`, one with a fraction an `xsd:double`, `true` / `false` an
`xsd:boolean`, per the RML-IO registry), where the legacy vocabulary reads
every JSON value as a plain string. And a JSONPath reference that selects a
whole array or object is an error under RML-Core (select its members with
`[*]`), where the legacy vocabulary reads an array's elements and an object's
JSON text.

The RML modules beyond RML-Core and RML-IO's sources are **not implemented**:
RML-FNML (functions), RML-CC (collections and containers), RML-LV (logical
views), RML-star and RML-IO targets. A mapping that uses one of their terms is
refused with the module named, rather than run as if the terms were absent.
The legacy `fnml:functionValue` with this store's own functions
(`otsfn:mapValue`, `otsfn:mintIri`) still works.

---

## Supported Features

| Feature | Support |
|---|---|
| CSV (`ql:CSV`, `rml:CSV`) | Full — one iteration per row, header-named columns; a `csvw:Table` source may set the CSVW dialect — see [Sources](#sources) |
| JSON (`ql:JSONPath`, `rml:JSONPath`) | Full — the iterator and references are RFC 9535 JSONPath; a reference may select several values; JSON Lines files — see [JSON Source](#json-source) |
| XML (`ql:XPath`, `rml:XPath`) | XPath 1.0 iterator and references with attributes, axes and declared namespaces — see [XML Source](#xml-source) |
| `rr:template` | Full — `{reference}` expansion, the cartesian product of the references' values; IRI-safe encoding (R2RML §7.3) when the term is an IRI, URI-safe for `rml:URI`, the value as it is otherwise |
| `rml:reference` / `rr:column` | Full — one term per value the reference selects |
| `rr:constant` | Full — the term is the constant itself: an IRI stays an IRI, a literal keeps its datatype and language tag (R2RML §7.4) |
| `rr:class` | Full — adds `rdf:type` to every generated subject, in the subject's graphs |
| `rr:termType` | IRI, BlankNode, Literal, with R2RML's defaults, and RML-Core's `rml:URI`, `rml:UnsafeIRI`, `rml:UnsafeURI`; a blank-node term map with no expression generates a fresh blank node per iteration — see [Term types](#term-types) |
| `rr:datatype` / `rml:datatypeMap` | Full — a constant datatype, or one generated per iteration |
| `rr:language` / `rml:languageMap` | Full — a constant tag, or tags generated per iteration (each must be BCP 47) |
| `rr:graphMap` / `rr:graph` | On the subject map and on predicate-object maps; a triple goes to the union of both, `rr:defaultGraph` names the default graph — see [Named Graphs](#named-graphs) |
| Blank nodes | One per value and graph (R2RML §11.2, §9.1) |
| Base IRI | `?base=` on the execute endpoint, or `rml:baseIRI` on a triples map (RML-Core) |
| `rr:subjectMap` shortcut (`rr:subject`) | Supported |
| `rr:predicateMap` shortcut (`rr:predicate`) | Supported |
| `rr:objectMap` shortcut (`rr:object`) | Supported |
| Several predicate and object maps per predicate-object map | Every predicate map × every object map, shortcuts included (R2RML §6.3, §11.1): `rr:predicate ex:a, ex:b ; rr:object ex:X ; rr:objectMap [ … ]` generates four triples per row |
| `rr:parentTriplesMap` (referencing object maps, joins) | Full — on CSV, JSON and XML sources, and between them; RML-Core's `rml:childMap` / `rml:parentMap` may be a reference, a template or a constant — see [Joins](#joins) |
| Empty values and `rml:null` | An empty CSV cell, an empty JSON string and an empty XML element are values — an empty literal, or an IRI built from the empty string (RML-IO: nothing is NULL unless the source says so). A JSON `null` and a missing key or element are no value. `rml:null "…"` on the logical source or its source description (and `csvw:null` on a CSVW table) lists values that count as NULL — `rml:null ""` restores "an empty cell generates nothing" |
| Encodings and compression | `rml:encoding` (`rml:UTF-8`, `rml:UTF-16`, or a `csvw:encoding` name), `rml:compression` (`rml:gzip`, `rml:zip`, `rml:targz`, `rml:tarxz`) — see [Sources](#sources) |
| Mapping validation | A non-conforming mapping is refused at upload with the construct named — see [Errors](#errors) |
| Data errors | A value that cannot become its term aborts the run and names the rows, or is skipped and reported with `on_data_error=skip` — see [Errors](#errors) |

---

## Sources

A logical source names its file in one of three ways, and the run looks it up
among the files it was given (the multipart parts of the execute and preview
endpoints) — as written, then without a leading `./`, then by its last path
segment:

```turtle
rml:source "people.csv"                                            # legacy RML
rml:source [ a rml:RelativePathSource ; rml:path "people.csv" ]    # RML-IO
rml:source [ a csvw:Table ; csvw:url "people.csv" ;                # CSVW
             csvw:dialect [ csvw:delimiter ";" ; csvw:encoding "utf-16" ] ]
```

A source is never fetched over the network: a logical source that names a
remote URL is refused unless the run was given a part of that name. A D2RQ
database description or a SPARQL endpoint description is refused too — those
are registered datasources ([Sources](sources.md)).

An RML-IO source description may also say how its bytes are read:

| Property | Values | Effect |
|---|---|---|
| `rml:encoding` | `rml:UTF-8` (default), `rml:UTF-16`, or any WHATWG encoding label through `csvw:encoding` | how the bytes are decoded; a byte-order mark always wins |
| `rml:compression` | `rml:none` (default), `rml:gzip`, `rml:zip`, `rml:targz`, `rml:tarxz` | the file is decompressed first; an archive must hold one file, or one named like the source (`Friends.json.zip` → `Friends.json`) |
| `rml:null` | strings | values that count as NULL, besides the format's own |

A decompressed source larger than `OTS_RML_MAX_SOURCE_BYTES` (default 256 MiB)
is refused, so a compression bomb cannot exhaust memory. Zip support is part
of the `asset-archive` build feature, which `full` includes.

A `csvw:Table` source may carry a **CSVW dialect**: `csvw:delimiter` (`;`,
`\t`, …), `csvw:quoteChar`, `csvw:doubleQuote`, `csvw:header` /
`csvw:headerRowCount`, `csvw:skipRows`, `csvw:commentPrefix`, `csvw:trim` and
`csvw:encoding`; `csvw:null` on the table names NULL values. A table without a
header names its columns `_col.1`, `_col.2`, … as CSVW does.

## Joins

A referencing object map makes the object the subject another triples map
generates (R2RML §8):

```turtle
ex:StudentMap a rr:TriplesMap ;
  rml:logicalSource [ rml:source "students.json" ; rml:referenceFormulation ql:JSONPath ;
                      rml:iterator "$.students[*]" ] ;
  rr:subjectMap [ rr:template "http://example.org/student/{ID}" ] ;
  rr:predicateObjectMap [ rr:predicate ex:practises ; rr:objectMap [
      rr:parentTriplesMap ex:SportMap ;
      rr:joinCondition [ rr:child "Sport" ; rr:parent "ID" ] ] ] .
ex:SportMap a rr:TriplesMap ;
  rml:logicalSource [ rml:source "sports.csv" ; rml:referenceFormulation ql:CSV ] ;
  rr:subjectMap [ rr:template "http://example.org/sport/{ID}" ] .
```

- **With join conditions**, the parent's rows are read once and indexed by the
  parent side of the join; each child row links to the subject of every parent
  row whose values equal its own on every condition. The two maps may read
  different files and different formats. A key with a NULL in it (a missing
  JSON key or XML element, a JSON `null`, a value the logical source lists
  under `rml:null`) matches nothing; an empty CSV cell is the value `""`. The
  index is bounded by `OTS_SOURCES_JOIN_MAX_ROWS` (default 1 000 000 distinct
  keys); a mapping that would exceed it is refused by name before anything is
  written.
- **Without a join condition**, both maps must read the same logical source
  (the same file, iterator and reference formulation; a mapping where they do
  not is refused at upload), and each row joins to *itself*: the object is the
  parent's subject for the child's own row. It is not a cross join.

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
| `on_data_error=skip` | `abort` | Leave out terms whose values cannot become them, and report the rows as `data_errors`, instead of failing the run — see [Errors](#errors). `POST /api/rml/preview` takes it too |

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

Reference formulation: `ql:JSONPath` or `rml:JSONPath`

The iterator is an [RFC 9535](https://www.rfc-editor.org/rfc/rfc9535) JSONPath
query (default `$`): every node it selects is one iteration. A reference is a
JSONPath query on that node — `$.name`, `$.address.city`, `$.tags[*]` — and
may select several values, each generating a term. A reference written as a
bare name (`name`, `Country Code`), as legacy mappings write them, is the
member of that name; `a.b` is `$.a.b`. Under the legacy vocabulary an
iterator that selects one array iterates its elements, as this engine always
did (`$.orders` and `$.orders[*]` agree there).

A file named `*.jsonl` or `*.ndjson` (optionally `.gz`) is **JSON Lines**: each
line is a document the iterator runs over.

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

Reference formulation: `ql:XPath` or `rml:XPath`

The iterator and references are **XPath 1.0** expressions: every node the
iterator selects is one iteration and the context node of its references, so
`name`, `name/text()`, `@id`, `../@id` and `count(item)` all work. A node-set
reference has one value per node — its string value — in document order.
Namespaces are declared on the reference formulation:

```turtle
rml:referenceFormulation [ a rml:XPathReferenceFormulation ;
    rml:namespace [ rml:namespacePrefix "ex" ; rml:namespaceURL "http://example.org/" ] ] ;
rml:iterator "/Friends/ex:Character" .
```

The RML-IO registry asks for XPath 3.1; the 2.0 and 3.1 additions (sequences,
`if`, `for`, typed comparisons) are refused as invalid expressions. A relative
iterator in a legacy mapping (`person`) matches the element anywhere
(`//person`), as this engine's older XML reader did.

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

In `rr:template` strings, `{reference}` placeholders are replaced with the reference's value. When the term is an IRI the value is made **IRI-safe** (R2RML §7.3): every character outside RFC 3987 `iunreserved` — letters, digits, `-`, `.`, `_`, `~` and non-ASCII letters — is UTF-8 percent-encoded. For `rml:URI` it is made **URI-safe** (non-ASCII letters encoded too); `rml:UnsafeIRI`, `rml:UnsafeURI`, a literal and a blank-node template take the value as it is. A reference with no value skips the term, and so the triple; a reference with several values makes one term per element of the cartesian product of all the template's references (`http://x/{$.a[*]}/{$.b[*]}` over two and three values makes six). A backslash escapes a brace or a backslash, inside a placeholder too (RML-Core): `{$['\\{Name\\}']}`.

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
| Object map with `rr:language` / `rml:languageMap` or `rr:datatype` / `rml:datatypeMap` | A literal |
| Any other object map — a template | An IRI |
| A constant (`rr:constant`, `rr:object`, …) | The constant itself; `rr:termType` has no effect |

So `rr:objectMap [ rr:template "Hello {name}" ]` is an IRI; write `rr:termType rr:Literal` for text.

RML-Core adds three IRI term types — `rml:URI` (an RFC 3986 URI: template values URI-safe, non-ASCII refused), `rml:UnsafeIRI` and `rml:UnsafeURI` (template values written in as they are, so the result must already be a valid IRI) — and a blank-node term map with no expression (`rml:subjectMap [ rml:termType rml:BlankNode ]`), which generates a fresh blank node for every iteration.

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

## Errors

Three kinds, each refused rather than run into silence:

**A non-conforming mapping** is refused when it is parsed — on upload, and on
every run of a stored mapping — with `400` and a message that names the
triples map and the construct. R2RML's rules: a triples map has exactly one
logical source (`rml:logicalSource` / `rr:logicalTable`) and exactly one
subject map (`rr:subjectMap` / `rr:subject`); a term map is exactly one of a
constant, a column (`rr:column` / `rml:reference`) or a template, each given
once; `rr:termType` is `rr:IRI`, `rr:BlankNode` or `rr:Literal`, and only
what the position allows (a subject: IRI or blank node; a predicate or a
graph: IRI); `rr:language` and `rr:datatype` exclude each other, belong to a
literal only, and a language tag must be valid BCP 47; `rr:class` values are
IRIs; a logical table names `rr:tableName` or a query, not both; a
referencing object map has no term map of its own, and each join condition
one `rr:child` and one `rr:parent`. `rr:sqlVersion` and `rr:inverseExpression`
are accepted (and not needed).

**A column the source does not have** is refused before the first row: a CSV
header that lacks a column the mapping names, or — for a registered
datasource — a column the logical table's query does not return. The message
names the column, where the mapping uses it, and the columns the source has.
JSON and XML records carry no fixed column set; a missing key there is a NULL.

**A data error** (R2RML §4.3) is a row whose value cannot become the term the
mapping asks for: an IRI term map whose value is not a valid IRI (nor one
relative to the base IRI), or a literal whose `rr:datatype` it does not fit
(`"forty-two"` as `xsd:integer`; XSD's numeric, boolean, date, time,
duration, `g*` and `hexBinary` types are checked). By default the run aborts,
writes nothing, and names the first ten offending rows with their values. With
`on_data_error=skip` it leaves those terms out — as this engine once did
silently — and reports every row it skipped from:

```json
{ "triples_inserted": 418, "target_graph": "urn:dataset:<id>:rml-output",
  "data_errors": { "rows": 1,
    "first": ["<http://example.org/LinkMap> row 2: column \"target\" generates \"not a valid iri\", which is not a valid IRI"] } }
```

---

## Storage

Mappings are stored in the named graph `urn:dataset:<id>:rml-mappings` and are automatically registered in the dataset's graph list. Generated output goes to `urn:dataset:<id>:rml-output` unless overridden.

Both graphs appear in the dataset graph list and participate in dataset-scoped SPARQL queries.

---

## Limitations

- **SQL / SPARQL sources**: this upload path reads files only. SQL logical tables (`rr:tableName`, `rml:query`) and SPARQL endpoints are mapped through registered datasources ([Sources](sources.md)); a mapping that names one is refused here with a pointer to that path.
- **Large files**: Source files are read entirely into memory (an XML file as a DOM). For very large files (> 100 MB), consider splitting them before upload.
- **Remote sources** are not fetched: give the file to the run.
- **RML modules**: RML-FNML, RML-CC, RML-LV, RML-star and RML-IO targets are not implemented; a mapping that uses them is refused.
