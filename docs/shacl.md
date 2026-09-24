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

The parser is **strict**: input it does not recognise — a W3C SHACL-C form this
parser does not implement, an unknown constraint keyword, plain garbage — is a
`400` naming the line and column, and the dataset's shapes graph is left as it
was. (It used to be lenient: unrecognised input was dropped, so a document that
used unsupported forms could parse to an *empty* shapes graph, and the upload
replaced the dataset's shapes with nothing while answering 200.) Pass
`?lenient=true` for the old behaviour — whatever parses is kept, the rest is
ignored:

```bash
curl -X PUT 'http://localhost:7878/api/datasets/<dataset_id>/shapes?lenient=true' \
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

## What a validation run reads

A dataset validation run is given **every registered graph of the dataset**
except its persisted-report graph — instances, shapes, linkset, provenance,
catalogue, domain values — plus **the graphs of the data model the dataset
declares `dct:conformsTo`**. Derived graphs are not included: entailment
output, version snapshots and report graphs are never registered as dataset
graphs.

A run reads **one instant**: the query accelerator's in-memory copy when one
is published, otherwise a single RocksDB snapshot taken when the run starts.
`sh:sparql` constraints, custom-component validators and SHACL-AF SPARQL
targets read that same source, so a write landing mid-run cannot be visible to
one half of a shapes graph and invisible to the other. Those queries therefore
do not use the result cache or the accelerator's shard routing — the same trade
every other probe in the run makes.

The model graphs are in scope because SHACL reads the class hierarchy out of
the data graph it is handed: *"all the `rdfs:subClassOf` declarations needed to
walk the class hierarchy need to exist in the data graph"* (SHACL §2.1.3.2).
Without them, `sh:targetClass` on a superclass would target nothing and
`sh:class` against a model term would fail, silently. Only model graphs the
caller may read are added.

### Multi-graph reach — a known inconsistency

When a run spans more than one data graph, the SHACL constructs do not all read
the same set of graphs, and **the same logical rule can give opposite answers
depending on how it is written**:

| Construct | Reads |
|---|---|
| `sh:path` (property paths), for an IRI focus node | each data graph separately, results unioned — a path that must cross graphs finds nothing |
| `sh:path`, for a blank-node or literal focus node | all data graphs merged |
| `sh:sparql`, `sh:class` | all data graphs merged |
| `sh:closed`, `sh:targetSubjectsOf`, `sh:targetObjectsOf` | all data graphs at once — but these are single-hop lookups, so this is the same answer as reading each graph in turn |
| `sh:targetClass` | type triples per graph; the `rdfs:subClassOf*` chain across all graphs |

Only paths with an **intermediate node** can diverge — a sequence, a
`zeroOrMorePath` or a `oneOrMorePath`. A single hop matches quads that each
live in exactly one graph, so reading the graphs one at a time and reading them
merged give the same answer; `sh:closed` and the `subjectsOf`/`objectsOf`
targets are therefore never affected.

So a rule expressed as `sh:path ( ex:hasDeck ex:width )` can report a violation
that the identical rule written as a `sh:sparql` constraint does not, and the
same path answers differently for an IRI focus node and a blank-node one. The
specification defines validation against **one** data graph (§3.4), so the
merged reading is the faithful one and the per-graph path evaluation is the
deviation.

**This has not been changed**, because flipping it would alter which SHACL-AF
rules fire, and inference materialises into your data on an unattended
schedule. Single-graph runs — which includes every write gate — are unaffected
either way, since the two readings coincide when there is one graph.

To find out whether it affects your data, set `OTS_SHACL_REACH_PROBE=1`. Each
run then logs, at warning level, how many value-node lookups found nothing per
graph but would have found values over the merge:

```
graph-reach probe: 14 value-node lookups found nothing per data graph but would
have found 21 value nodes over the merge of them
```

The probe changes no answer — it measures and discards. It costs one extra path
evaluation per lookup that found nothing, so leave it off outside an
investigation. If it reports nothing on your datasets, the inconsistency does
not reach your data.

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

The gate fails **closed**: a gate that cannot be evaluated refuses the write with the same 422 and a report naming the cause, never a 204. That covers a shapes graph that cannot be read or copied, a validation-engine error, and an ill-formed shapes graph — in particular a `sh:sparql` constraint whose `sh:select` does not parse (or errors at evaluation) is a violation of the focus node, not a constraint that silently never fires. Loading such a shapes graph for on-demand validation fails with an error for the same reason.

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

A shape graph *is* a named graph of SHACL, so the **Shapes catalog** sweeps every non-system graph the caller may read — including shapes embedded in data graphs ("joined with instance data") — for `sh:NodeShape` / `sh:PropertyShape` subjects. It is graph-first: without `?graph=` it lists the graphs holding shapes, with counts; `?graph=<iri>` returns one graph's shapes:

```bash
curl http://localhost:7878/api/shacl/shapes -H 'Authorization: Bearer <token>'
# → { graphs: [{ graph, node_count, property_count, total,
#                registered, shape_graph_id?, shape_graph_name? }, …] }
curl "http://localhost:7878/api/shacl/shapes?graph=<url-encoded iri>" -H 'Authorization: Bearer <token>'
# → { graph, shapes: [{ graph, shape, kind:"node"|"property", label?, target_classes, path?,
#                       registered, shape_graph_id?, shape_graph_name? }, …] }
```

A graph registered in the Library is listed to whoever the Library shows its entry to. Any other graph is listed only to a caller who may read it by the rule `/sparql` applies — dataset visibility (a private graph only for its dataset's writers) plus graph-ACL read grants; admins read every graph — and `?graph=` on any other graph answers 403.

**Compose** — copy picked shapes (each with its full blank-node closure) into a shape graph (create an empty one first, or pick an existing one):

```bash
curl -X POST http://localhost:7878/api/shacl/shape-graphs/<shape_graph_id>/import-shapes \
     -H 'Authorization: Bearer <token>' -H 'Content-Type: application/json' \
     -d '{"shapes":[{"source_graph":"urn:shapes:other","shape":"http://ex/PersonShape"}]}'
```

**Register in place** — adopt a pre-existing shapes-bearing graph as a Library shape graph without copying (idempotent; returns the existing record, to a caller who may see it, if already known). The caller becomes the entry's owner, and owners edit the graph in place. So registering needs the right to change the graph, not only to read it: an admin, a graph-ACL write grant, write access to a dataset that holds the graph (in its namespace or registered to it), or write access to the registry entry holding it. Graphs named `urn:shapes:…` are registered only by admins:

```bash
curl -X POST http://localhost:7878/api/shacl/register-shape-graph \
     -H 'Authorization: Bearer <token>' -H 'Content-Type: application/json' \
     -d '{"graph_iri":"http://example.org/graph/my-shapes","name":"My shapes"}'
```

Every Studio write checks that right again: save, restore, import shapes, and a visibility change of an adopted graph. Managing a Library entry is enough on its own only for a graph the Studio minted for it (`urn:shapes:…`). So a dataset's shapes graph is edited in the Studio by the members who may write the dataset, not by an org viewer, as with `PUT /api/datasets/{id}/shapes`.

Impact — *what data a shape graph is applied to* — is the reverse binding lookup: `GET /api/shacl/bindings?shape_graph_id=<shape_graph_id>` → `{ shape_graph_id, targets: [ …IRIs ] }`.

### Pipelines & targets

A pipeline is a saved, runnable validation. Its scope is a set of **targets** — any mix of datasets, graphs, and shape graphs — plus composed shape graphs, a severity threshold, and triggers (manual, on-write, cron). When `gate_writes` is set, writes covered by the pipeline are gated. See `POST /api/shacl/pipelines`; the request body's `targets` is an array of `{ "kind": "dataset"|"graph"|"shapegraph", "id": "…" }`.

A run's report carries the data it validated (focus nodes and values), so a pipeline's whole scope — every dataset, every data graph it resolves to and every shape graph it composes — must be readable by whoever creates or updates it, runs or test-runs it, or opens a stored run's report (`GET /api/shacl/pipelines/{id}/runs/{run_id}`); anything else answers 403. Reading follows the `/sparql` rule above, and a Library shape graph is readable by whoever the Library shows it to. The check is made each time, so a revoked grant takes effect at the next run. A scheduled run is checked against the pipeline's creator and skipped when they may no longer read its scope. Run summaries (`…/runs`, counts only) are listed to everyone who can see the pipeline.

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

### Reading and writing a shape graph's content

The content of a shape graph is served and replaced as one document:

```bash
# Turtle, with an @prefix header built from the prefix registry for the
# namespaces the graph actually uses — sh:, xsd:, the deployment's own.
curl http://localhost:7878/api/shacl/shape-graphs/<shape_graph_id>/turtle -H 'Authorization: Bearer <token>'

# SHACL Compact Syntax instead: ?format=shaclc, or Accept: text/shaclc
curl 'http://localhost:7878/api/shacl/shape-graphs/<shape_graph_id>/turtle?format=shaclc' -H 'Authorization: Bearer <token>'

# Replace the content. The revision note is what the history shows for it.
curl -X PUT 'http://localhost:7878/api/shacl/shape-graphs/<shape_graph_id>/turtle?message=Require%20a%20name' \
     -H 'Authorization: Bearer <token>' -H 'Content-Type: text/turtle' --data-binary @shapes.ttl
# → {"version": 4}
```

`message` is optional (the default note is `Edited`), trimmed, bounded to 200 characters and stripped of control characters before it reaches the commit log. A body sent as `Content-Type: text/shaclc` is parsed as SHACL-C first and stored as Turtle. Reading needs read access to the shape graph; writing needs manage access.

---

## SHACL-AF Inference

Run SHACL Advanced Features rules to materialize inferred triples:

```bash
curl -X POST http://localhost:7878/api/datasets/<dataset_id>/infer \
     -H 'Authorization: Bearer <token>'
# → {"inferred_triples": 42}
```

Supports `sh:SPARQLRule` (`sh:construct`) and `sh:TripleRule` (`sh:subject` /
`sh:predicate` / `sh:object`, with `sh:this` standing for the focus node; a
literal object keeps its datatype). Inferred triples are written back into the
data graph, and the rules run to a fixed point. The SHACL-AF rule modifiers are
honoured:

| Modifier | Effect |
|---|---|
| `sh:order` | Rules run in ascending order (default `0`), so a later rule sees what an earlier one produced within the same pass. |
| `sh:condition` | A shape the focus node must conform to for the rule to fire — below, only adults get `ex:mayVote`. |
| `sh:deactivated true` | On the rule or on its shape: the rule does not run. |

```turtle
ex:VoterShape a sh:NodeShape ;
  sh:targetClass ex:Person ;
  sh:rule [ a sh:TripleRule ; sh:order 1 ; sh:condition ex:Adult ;
            sh:subject sh:this ; sh:predicate ex:mayVote ; sh:object true ] .
ex:Adult a sh:NodeShape ;
  sh:property [ sh:path ex:age ; sh:minInclusive 18 ] .
```

---

## SPARQL-based constraints and constraint components

### `sh:sparql` and pre-binding

A `sh:SPARQLConstraint` (`sh:select`) is evaluated once per focus node with
`$this` **pre-bound** as SHACL §5.3 defines it: the focus node reaches every
scope of the query — a `FILTER` in a nested group or a `UNION` branch, a
sub-select that projects `$this`, the projection and `GROUP BY` of an aggregate
— and `bound($this)` is true. On a property shape, `$PATH` is replaced by the
shape's path. Every solution is a violation; `?value` and `?path` in a solution
become `sh:value` and `sh:resultPath`.

The features the specification forbids under pre-binding (§5.3.2) — `MINUS`,
`VALUES`, `SERVICE`, a nested `SELECT` that does not project `$this`
explicitly (`SELECT *` included), and assigning to a pre-bound variable
(`… AS $this`) — make the shapes graph **fail to load**, so a constraint that
uses them fails loudly instead of silently never firing. `$shapesGraph` and
`$currentShape` are not supported and fail the shapes graph the same way. The
`sh:prefixes` prologue includes the `sh:declare` declarations of the named
ontology and of everything it `owl:imports` within the shapes graph.

### Custom constraint components (SHACL-AF §6)

A shapes graph can declare its own reusable constraint components. A shape that
carries a component's parameter predicates instantiates it:

```turtle
ex:MaxWordsComponent a sh:ConstraintComponent ;
  sh:parameter [ sh:path ex:maxWords ] ;
  sh:propertyValidator [ a sh:SPARQLAskValidator ;
    sh:message "Too many words (max {$maxWords})" ;
    sh:ask """ASK { FILTER (STRLEN(REPLACE(STR($value), "[^ ]", "")) < $maxWords) }""" ] .

ex:TitleShape a sh:NodeShape ; sh:targetClass ex:Doc ;
  sh:property [ sh:path ex:title ; ex:maxWords 3 ] .
```

* **`sh:parameter`** — one per parameter. Its `sh:path` is the predicate the
  shape uses, and the path's local name is the SPARQL variable the validator
  sees (`ex:maxWords` → `$maxWords`). `sh:optional true` makes a parameter
  optional; a component only applies when every mandatory parameter is present.
* **Validators** — `sh:nodeValidator` (node shapes), `sh:propertyValidator`
  (property shapes) or `sh:validator` (either). An `sh:ask` validator runs once
  per value node with `$this`, `$value` and the parameters pre-bound; `false`
  is a violation. An `sh:select` validator runs once per focus node; every row
  is a violation (`?value`, `?path` as for `sh:sparql`). `$PATH` is available
  in property validators.
* **`sh:message`** on the validator is the result message, with `{$param}`,
  `{?param}`, `{$this}` and `{$value}` rendered; `sourceConstraint` names the
  component.

A component a shape uses without a validator for the shape's kind, or a
validator that does not parse, fails the shapes graph.

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
# SHACLC text → Turtle (strict: unrecognised input is a 400 naming its position)
curl -X POST http://localhost:7878/api/shaclc/parse \
     -H 'Content-Type: text/shaclc' \
     --data-binary @shapes.shaclc

# The same, ignoring unrecognised input instead of failing on it
curl -X POST 'http://localhost:7878/api/shaclc/parse?lenient=true' \
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

## Exporting to IDS

The inverse of the importer, at `POST /api/shacl/export/ids` with a shapes
graph in Turtle as the body. `GET /api/shacl/exporters` lists the formats.

```bash
curl -X POST http://localhost:7878/api/shacl/export/ids \
     -H 'Authorization: Bearer <token>' \
     -H 'Content-Type: text/turtle' \
     --data-binary @shapes.ttl
# → {"format":"ids","document":"<?xml …","specification_count":2,"losses":[…]}

# the bare document instead of the report
curl -X POST 'http://localhost:7878/api/shacl/export/ids?raw=true' …
```

**The report is the default representation, and `losses` is the reason.** IDS's
whole expressive surface for a requirement is a facet kind, a cardinality of
`required` / `prohibited` / `optional`, and one value restriction. Most of SHACL
has no IDS form at all, so an exporter that silently wrote a thinner document
than the shapes it was given would be actively misleading for a delivery
contract. Every constraint that cannot be carried is listed, and a shape graph
from which nothing at all can be expressed is a `422`, not an empty document.

### What survives

| SHACL | IDS |
|---|---|
| `sh:targetClass` | `<ids:applicability><ids:entity>` (several classes become an `xs:enumeration`) |
| `sh:hasValue` | `<ids:value><ids:simpleValue>` |
| `sh:in` | `<xs:restriction>` with `<xs:enumeration>` |
| `sh:minInclusive` / `sh:maxInclusive` / `sh:minExclusive` / `sh:maxExclusive` | the matching `xs:` facet |
| `sh:minLength` / `sh:maxLength` | `<xs:minLength>` / `<xs:maxLength>` |
| `sh:pattern` without `sh:flags` | `<xs:pattern>` — see the caveat below |
| `sh:minCount 1` | `cardinality="required"` |
| `sh:maxCount 0` | `cardinality="prohibited"` |
| `sh:class` on an inverse `bot:` path | `<ids:partOf>` with its nested entity |
| the `sh:or ( [ sh:not …-applies ] …-requires )` idiom | the applicability / requirements split |

### What does not

`sh:nodeKind`, `sh:languageIn`, `sh:uniqueLang`, `sh:equals`, `sh:disjoint`,
`sh:lessThan`, `sh:lessThanOrEquals`, `sh:xone`, `sh:node`, nested
`sh:property`, `sh:qualifiedValueShape`, `sh:closed`, `sh:sparql`, custom
constraint components and `sh:expression` have no IDS counterpart. Neither does
`sh:minCount n` for n > 1 or `sh:maxCount n` for n > 0 — IDS carries no
multiplicity. A shape targeted by `sh:targetNode`, `sh:targetSubjectsOf`,
`sh:targetObjectsOf` or a SPARQL target cannot become a specification at all,
because IDS applicability is class-based.

Three further caveats, each reported in `losses` when it applies:

- **A classification facet is dropped.** `ids:classificationType/system` is
  mandatory and the importer keeps it only as free text, so it cannot be
  recovered; synthesising one would emit a document that lies.
- **`xs:pattern` is implicitly anchored and has no flags**, while `sh:pattern`
  is an XPath/SPARQL regex. A flagless pattern is exported with a warning that
  the match semantics differ; a flagged one is dropped.
- **This is not a general SHACL-to-IDS translator.** It exports shapes written
  over *this store's* IFC RDF vocabulary — the `props:` / `bot:` convention the
  IFC lift emits and the IDS importer targets. Shapes produced by other tools
  will mostly land in the loss list.

The tests pin an import → export → import fixpoint over that shared subset; they
do not prove the output is schema-valid, because validating against the IDS XSD
would need a network fetch and an XSD validator, and neither is available here.

---

## Importing constraint specifications (IDS)

Domain exchange requirements often arrive in their own format. The
specification importers turn such a document into a SHACL shape graph that
lives in SHACL Studio like any other. The interface is generic (a format id,
bytes in, Turtle and a report out); buildingSMART **IDS 1.0** is the first
implementation.

```bash
curl http://localhost:7878/api/shacl/importers                       # registered formats
curl -X POST 'http://localhost:7878/api/shacl/import/ids?create=true' \
  -H "Authorization: Bearer <token>" -H 'Content-Type: application/xml' \
  --data-binary @requirements.ids
```

Without `create=true` the response carries the Turtle and the report only.
Each `ids:specification` becomes a node shape targeting the entity's ifcOWL
class over the RDF the built-in IFC importer emits (`props:<Pset>_<Name>`
properties, `props:ifcName`/`props:ifcGuid` attributes, BOT containment for
`partOf`). Applicability facets beyond the entity become an "applies" shape
combined with the "requires" shape as `sh:or ( [ sh:not applies ] requires )`
— SHACL Core throughout. Value restrictions map to `sh:hasValue`, `sh:in`,
`sh:pattern`, bounds and lengths; cardinality to `sh:minCount 1` /
`sh:maxCount 0`. Whatever cannot be expressed per node (a specification's
"at least one such entity must exist"), or relies on a value the IFC lift
does not populate (predefined types, attributes other than Name/GlobalId), is
listed under `warnings`. Classification and material facets target
`props:ifcClassification` / `props:ifcMaterial`, which the lift emits from
`IfcRelAssociatesClassification` (the reference's identification) and
`IfcRelAssociatesMaterial` (the material's name); a model lifted before it
did carries neither, and the warning says so.

