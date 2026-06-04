# SHACL Guide

This document covers SHACL validation, SHACL-AF inference, automatic validation on write, and SHACL Compact Syntax (SHACLC) in the Open Triplestore.

---

## Overview

The triplestore has built-in support for:

- **On-demand validation** — validate a dataset's named graphs against a shapes graph via `POST /api/datasets/:id/validate`
- **Validation on write** — when `shacl_on_write` is enabled, every Graph Store `PUT` or `POST` is validated before the data is committed
- **SHACL Studio** — reusable shape graphs, an RDF **validation layer** (graph-attached shapes that inherit into datasets), pipelines, write-gating, and **meta-validation** (SHACL-SHACL). See [SHACL Studio](#shacl-studio--shape-graphs-the-validation-layer--meta-validation) below
- **SHACL-AF inference** — materialize inferred triples by executing `sh:SPARQLRule` and `sh:TripleRule` rules
- **SHACLC** — upload and download shapes in [SHACL Compact Syntax](https://w3c.github.io/shacl/shacl-compact-syntax/) as well as Turtle

---

## Shapes Graph Storage

Each dataset can have one shapes graph. Its IRI is stored in the dataset record (`shapes_graph_iri`) and defaults to `urn:dataset:<id>:shapes` when first uploaded.

The shapes graph is a regular named graph in the triplestore and can be queried via SPARQL:

```sparql
SELECT * WHERE {
  GRAPH <urn:dataset:my-dataset:shapes> { ?s ?p ?o }
}
```

---

## Uploading Shapes

### Turtle

```bash
curl -X PUT http://localhost:7878/api/datasets/<dataset_id>/shapes \
     -H 'Authorization: Bearer <token>' \
     -H 'Content-Type: text/turtle' \
     --data-binary @shapes.ttl
```

### SHACL Compact Syntax (SHACLC)

Shapes can be uploaded in compact syntax — they are parsed to Turtle before storage. The stored form is always Turtle.

```bash
curl -X PUT http://localhost:7878/api/datasets/<dataset_id>/shapes \
     -H 'Authorization: Bearer <token>' \
     -H 'Content-Type: text/shaclc' \
     --data-binary @shapes.shaclc
```

---

## Retrieving Shapes

```bash
# Turtle (default)
curl http://localhost:7878/api/datasets/<dataset_id>/shapes \
     -H 'Authorization: Bearer <token>'

# SHACLC via Accept header
curl http://localhost:7878/api/datasets/<dataset_id>/shapes \
     -H 'Authorization: Bearer <token>' \
     -H 'Accept: text/shaclc'

# SHACLC via query param
curl 'http://localhost:7878/api/datasets/<dataset_id>/shapes?format=shaclc' \
     -H 'Authorization: Bearer <token>'
```

---

## On-Demand Validation

```bash
curl -X POST http://localhost:7878/api/datasets/<dataset_id>/validate \
     -H 'Authorization: Bearer <token>'
```

Response:

```json
{
  "conforms": false,
  "results_count": 2,
  "results": [
    {
      "severity": "Violation",
      "focusNode": "http://example.org/alice",
      "path": "http://schema.org/name",
      "value": null,
      "message": "Less than 1 values on schema:name",
      "sourceShape": "urn:dataset:my-dataset:shapes#PersonShape",
      "sourceConstraint": "http://www.w3.org/ns/shacl#MinCountConstraintComponent"
    }
  ]
}
```

---

## Validation on Write

When `shacl_on_write` is `true` on a dataset and a `shapes_graph_iri` is configured, every `PUT` or `POST` to `/store?graph=<graph-iri>` that targets a graph belonging to the dataset is validated before the write is committed.

If validation fails, the write is rejected with **422 Unprocessable Entity** and the JSON report is returned. The store is not modified.

### Enable via API

```bash
curl -X PUT http://localhost:7878/api/datasets/<dataset_id> \
     -H 'Authorization: Bearer <token>' \
     -H 'Content-Type: application/json' \
     -d '{"shacl_on_write": true}'
```

### Example: valid write succeeds

```bash
curl -X PUT 'http://localhost:7878/store?graph=http://example.org/people' \
     -H 'Authorization: Bearer <token>' \
     -H 'Content-Type: text/turtle' \
     -d '@prefix schema: <http://schema.org/> .
         <http://example.org/alice> a schema:Person ;
             schema:name "Alice" .'
# → 204 No Content
```

### Example: invalid write is rejected

```bash
curl -X PUT 'http://localhost:7878/store?graph=http://example.org/people' \
     -H 'Authorization: Bearer <token>' \
     -H 'Content-Type: text/turtle' \
     -d '@prefix schema: <http://schema.org/> .
         <http://example.org/alice> a schema:Person .'
         # missing required schema:name

# → 422 Unprocessable Entity
# {"error":"SHACL validation failed","conforms":false,"results":[...]}
```

### Limitations

- Validation is applied to `PUT` and `POST` on the Graph Store Protocol (`/store`).
- SPARQL `UPDATE` statements are not validated automatically (target graphs cannot be reliably determined without executing the update).
- Only named graphs registered to the dataset trigger validation; writes to unregistered graphs pass through unchecked.

---

## SHACL Studio — shape graphs, the validation layer & meta-validation

The Studio (under `/shacl` in the UI) generalises the per-dataset gate above into reusable **shape graphs**, an RDF **validation layer**, **pipelines**, and **meta-validation**. The concepts live in styleguide §5.4–5.6; this section is the API surface.

### The validation layer (bindings)

A *binding* links a target to a shape graph, stored as RDF in the system graph `urn:system:validation-layer` (`<target> ots:validatedBy <shape-graph graph>`, mirrored as `dct:conformsTo`). A target is a **dataset** (`{base}/datasets/{id}`), a **named graph** (its own IRI), or a **shape graph** (its `graph_iri`, for meta-validation). `kind` is one of `dataset` | `graph` | `shapegraph`.

```bash
# List bindings for a target (or reverse — for a shape graph — with ?shape_graph_id=…)
curl 'http://localhost:7878/api/shacl/bindings?target_kind=graph&target_id=http://example.org/graph/cities' \
     -H 'Authorization: Bearer <token>'

# Create a binding — attach a shape graph to a graph (idempotent)
curl -X POST http://localhost:7878/api/shacl/bindings \
     -H 'Authorization: Bearer <token>' -H 'Content-Type: application/json' \
     -d '{"target":{"kind":"graph","id":"http://example.org/graph/cities"},"shape_graph_id":"<shape_graph_id>"}'

# Remove a binding (same body)
curl -X DELETE http://localhost:7878/api/shacl/bindings \
     -H 'Authorization: Bearer <token>' -H 'Content-Type: application/json' \
     -d '{"target":{"kind":"graph","id":"http://example.org/graph/cities"},"shape_graph_id":"<shape_graph_id>"}'
```

Writing a binding requires write access to the target (a graph binding uses the same ACL as a Graph Store write); the change is recorded in the shape graph's commit history.

### Shapes inherited from graphs

Shapes attached to a **named graph travel with it**: any dataset that mounts the graph is validated against them, with no per-dataset wiring. A dataset's *effective* shapes are its own bindings ∪ the bindings of every graph it contains — and that effective set is what gates writes, runs in pipelines, and appears in the form-manifest. Inspect it:

```bash
curl http://localhost:7878/api/datasets/<dataset_id>/effective-shapes \
     -H 'Authorization: Bearer <token>'
```

A binding **gates on its own**: a write to a graph (or to any graph of a dataset) that carries a binding is validated and rejected with **422** exactly like the legacy `shacl_on_write` gate, even when no pipeline references it.

### Discovering & composing shapes

A shape graph *is* a named graph of SHACL, so the **Shapes catalog** sweeps every non-system graph — including shapes embedded in data graphs ("joined with instance data") — for `sh:NodeShape` / `sh:PropertyShape` subjects:

```bash
curl http://localhost:7878/api/shacl/shapes -H 'Authorization: Bearer <token>'
# → [{ graph, shape, kind:"node"|"property", label?, target_classes, path?,
#      registered, shape_graph_id?, shape_graph_name? }, …]
```

**Compose** — copy picked shapes (each with its full blank-node closure) into a shape graph (create an empty one first, or pick an existing one):

```bash
curl -X POST http://localhost:7878/api/shacl/shape-graphs/<shape_graph_id>/import-shapes \
     -H 'Authorization: Bearer <token>' -H 'Content-Type: application/json' \
     -d '{"shapes":[{"source_graph":"urn:shapes:other","shape":"http://ex/PersonShape"}]}'
```

**Register in place** — adopt a pre-existing shapes-bearing graph as a Library shape graph without copying (idempotent; returns the existing record if already known):

```bash
curl -X POST http://localhost:7878/api/shacl/register-shape-graph \
     -H 'Authorization: Bearer <token>' -H 'Content-Type: application/json' \
     -d '{"graph_iri":"http://example.org/graph/my-shapes","name":"My shapes"}'
```

Impact — *what data a shape graph is applied to* — is the reverse binding lookup: `GET /api/shacl/bindings?shape_graph_id=<shape_graph_id>` → `{ shape_graph_id, targets: [ …IRIs ] }`.

### Pipelines & targets

A pipeline is a saved, runnable validation. Its scope is a set of **targets** — any mix of datasets, graphs, and shape graphs — plus composed shape graphs, a severity threshold, and triggers (manual, on-write, cron). When `gate_writes` is set, writes covered by the pipeline are gated. See `POST /api/shacl/pipelines`; the request body's `targets` is an array of `{ "kind": "dataset"|"graph"|"shapegraph", "id": "…" }`.

### Meta-validation (SHACL-SHACL)

Validate a shape graph *as data* against the built-in SHACL-SHACL shape graph (seeded at `urn:system:shapes:shacl-shacl`):

```bash
curl -X POST http://localhost:7878/api/shacl/shape-graphs/<shape_graph_id>/validate \
     -H 'Authorization: Bearer <token>'
# → a ValidationReport, identical in shape to on-demand validation
```

To enforce meta-validity continuously, give a pipeline a `shapegraph` target (it loads the shape graph as data and SHACL-SHACL as the shapes), or persist an `ots:validatedBy` binding whose subject is the shape graph.

### Shape-graph lifecycle & history

Shape graphs move through `draft → staged → published → deprecated` and keep a commit history:

```bash
curl -X POST http://localhost:7878/api/shacl/shape-graphs/<shape_graph_id>/publish -H 'Authorization: Bearer <token>'
# also: /stage and /deprecate
curl http://localhost:7878/api/shacl/shape-graphs/<shape_graph_id>/commits -H 'Authorization: Bearer <token>'
```

When a dataset version is snapshotted, the dataset's effective bindings are captured into a version-scoped `{base}/dataset/{id}/version/{ver}/validation` graph and re-applied on restore — so the validation layer versions and branches together with the data it governs.

---

## SHACL-AF Inference

Run SHACL Advanced Features rules to materialize inferred triples:

```bash
curl -X POST http://localhost:7878/api/datasets/<dataset_id>/infer \
     -H 'Authorization: Bearer <token>'
# → {"inferred_triples": 42}
```

Supports `sh:SPARQLRule` and `sh:TripleRule` from SHACL-AF. Inferred triples are written back into the data graph.

---

## SHACL Compact Syntax (SHACLC)

SHACLC is a compact, human-friendly syntax for SHACL shapes. It is fully compatible with Turtle SHACL — shapes stored as Turtle can be serialized as SHACLC and vice versa.

### SHACLC Syntax Overview

```shaclc
PREFIX schema: <http://schema.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

shape schema:PersonShape -> schema:Person {
    schema:name xsd:string [1..1] // "Name is required" ;
    schema:email xsd:string [0..*] ;
    schema:age xsd:integer [0..1] ;
    schema:knows IRI [0..*] ;
}

shape schema:OrganizationShape -> schema:Organization closed {
    schema:name xsd:string [1..1] ;
    schema:url IRI [0..1] ;
}
```

Key SHACLC constructs:

| Construct | SHACLC | Turtle equivalent |
|---|---|---|
| Node shape | `shape IRI -> TargetClass { ... }` | `sh:NodeShape ; sh:targetClass` |
| Property cardinality | `[min..max]` or `[1..*]` | `sh:minCount / sh:maxCount` |
| Datatype | `xsd:string` after path | `sh:datatype xsd:string` |
| Node kind | `IRI` / `BlankNode` / `Literal` | `sh:nodeKind sh:IRI` |
| Shape reference | `schema:OtherShape` (non-datatype IRI) | `sh:node schema:OtherShape` |
| Closed shape | `closed` keyword | `sh:closed true` |
| Message | `// "message text"` | `sh:message "message text"` |
| Pattern | `pattern "regex"` | `sh:pattern "regex"` |

### Standalone conversion

```bash
# SHACLC text → Turtle
curl -X POST http://localhost:7878/api/shaclc/parse \
     -H 'Content-Type: text/shaclc' \
     --data-binary @shapes.shaclc

# Shapes graph from store → SHACLC
curl -X POST http://localhost:7878/api/shaclc/serialize \
     -H 'Content-Type: application/json' \
     -d '{"shapesGraphIri": "urn:dataset:my-dataset:shapes"}'

# Plain IRI body also accepted
curl -X POST http://localhost:7878/api/shaclc/serialize \
     -d 'urn:dataset:my-dataset:shapes'
```

### Graceful degradation

Shapes using SPARQL-based constraints or complex property paths that cannot be expressed in SHACLC are serialized as Turtle comments in the SHACLC output.

---

## Example Shapes (Turtle)

```turtle
@prefix sh:     <http://www.w3.org/ns/shacl#> .
@prefix schema: <http://schema.org/> .
@prefix xsd:    <http://www.w3.org/2001/XMLSchema#> .

schema:PersonShape
    a sh:NodeShape ;
    sh:targetClass schema:Person ;
    sh:property [
        sh:path schema:name ;
        sh:datatype xsd:string ;
        sh:minCount 1 ;
        sh:maxCount 1 ;
        sh:message "Every Person must have exactly one schema:name"
    ] ;
    sh:property [
        sh:path schema:email ;
        sh:datatype xsd:string ;
        sh:pattern "^[^@]+@[^@]+$" ;
        sh:message "schema:email must be a valid email address"
    ] .
```
