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
- **Repair proposals** — turn what the shapes determine into an explained RDF Patch, computed in a throwaway copy and applied only after review (`POST /api/datasets/:id/repair`). See [Repair proposals](repair.md)

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

Write the boolean flags `sh:uniqueLang`, `sh:closed`, `sh:deactivated`,
`sh:qualifiedValueShapesDisjoint` and `sh:optional` as `true` or `false`. An
upload that writes one as another `xsd:boolean` form (`"1"^^xsd:boolean`,
`"0"^^xsd:boolean`) is refused with 422 naming the triples, here and in SHACL
Studio (create and `PUT …/turtle`); see
[Literal forms the engine cannot see](#literal-forms-the-engine-cannot-see).

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

### Several data graphs: one merged graph

SHACL validates against **one** data graph (§3.4). A run over several data
graphs validates their **merge**: every construct reads all of them at once.

| Construct | Reads |
|---|---|
| `sh:path` (property paths), for any focus node | all data graphs merged: each hop of a sequence, alternative or closure may continue in any of them |
| `sh:sparql`, custom-component validators, SPARQL targets | all data graphs merged (the default graph of the query; no named graphs) |
| `sh:class`, `sh:targetClass` | all data graphs merged, the `rdfs:subClassOf*` chain included |
| `sh:closed`, `sh:targetSubjectsOf`, `sh:targetObjectsOf` | all data graphs merged |

So `sh:path ( ex:hasDeck ex:width )` finds a deck's width when `ex:hasDeck`
lives in the instances graph and `ex:width` in a details graph, and it answers
exactly as the same rule written as a `sh:sparql` constraint does. SHACL-AF
inference reads its rules' conditions the same way.

**Changed 2026-10-02.** Until then a path from an **IRI** focus node was
evaluated inside each data graph in turn, results unioned, so a path whose hops
lived in different graphs found nothing, while the same path from a blank-node
focus node, or the same rule as a `sh:sparql` constraint, found the value.
Paths with an intermediate node (a sequence, `sh:zeroOrMorePath`,
`sh:oneOrMorePath`) can therefore now find values they used to miss, on
datasets with more than one graph: a `sh:minCount` that used to fail can pass,
and a `sh:maxCount`, `sh:uniqueLang` or `sh:qualifiedMaxCount` that used to pass
can fail. SHACL-AF rules whose `sh:condition` reads such a path can fire where
they did not, and scheduled inference materialises what they derive. A
single-hop path reads the same either way, and so does every single-graph run,
which includes every write gate. The `OTS_SHACL_REACH_PROBE` setting that
measured the difference is gone.

A write gate checks the one graph being written, so for a path that crosses
graphs it no longer predicts the dataset run: a write can pass its gate and the
dataset still report the path, or the other way round. Run the dataset's
validation after writes that a cross-graph path depends on.

### How a shapes graph is read

- **Every value of a parameter is a constraint.** A shape with two values of
  `sh:not`, `sh:hasValue`, `sh:pattern` (sharing the one `sh:flags`) or
  `sh:qualifiedValueShape`, or two lists for `sh:and`, `sh:or` or `sh:xone`,
  must satisfy each of them (SHACL §4). Until 2026-10 only the first value was
  read, so a gate let through data that a later value forbids.
- **`sh:deactivated true` works on every shape**: top-level node and property
  shapes, values of `sh:property` (named or blank), and inline shapes under
  `sh:node`, `sh:not`, `sh:and`/`sh:or`/`sh:xone`, `sh:qualifiedValueShape` and
  a rule's `sh:condition`. Every term conforms to a deactivated shape
  (SHACL §2.1.6), so it reports nothing — and `sh:not` of a deactivated shape
  fails for every value.
- **An ill-formed shapes graph fails the run** (a write gate turns that into
  422) rather than skipping what it cannot use. Besides an unparseable
  `sh:sparql` or validator, that covers a value of `sh:property`, or a shape
  typed `sh:PropertyShape`, without a `sh:path`; a shape with more than one
  `sh:path`; a path that is not a well-formed SHACL property path (a literal, a
  blank node that is no path, a sequence or alternative with a member that is
  no path); and a SPARQL target (`sh:target [ sh:select … ]`) that does not
  parse, does not project `?this`, or errors when it runs.

### Literal forms the engine cannot see

The store keeps `xsd:boolean`, the numeric types and the date/time types as
values, not as the text that was written. What comes back is the canonical
form of that value, and validation only ever sees what comes back:

| Written | Read back |
|---|---|
| `"5"^^xsd:nonNegativeInteger` (any of the 12 types derived from `xsd:integer`: `xsd:int`, `xsd:byte`, `xsd:positiveInteger`, …) | `"5"^^xsd:integer` |
| `"2026-10-01T12:00:00Z"^^xsd:dateTimeStamp` | `"2026-10-01T12:00:00Z"^^xsd:dateTime` |
| `"1"^^xsd:boolean`, `"0"^^xsd:boolean` | `true`, `false` |

Two consequences for SHACL:

- **`sh:datatype` with a derived integer type or `xsd:dateTimeStamp` reports
  every stored value as a violation**, valid ones included, and a write gate
  answers 422 on valid data. Until storage keeps the written datatype, use
  `sh:datatype xsd:integer` with `sh:minInclusive` / `sh:maxInclusive` for
  the range (or `xsd:dateTime`).
- **A boolean flag written as `"1"` acts as `true`.** SHACL activates
  `sh:uniqueLang`, `sh:closed`, `sh:deactivated` and the other flags only for
  the literal `true` (W3C test `core/property/uniqueLang-002`, the one known
  core failure), but once stored the two cannot be told apart. The dataset
  `PUT …/shapes` and SHACL Studio uploads refuse such flags instead of storing
  a meaning the author may not have intended; the other write paths (Graph
  Store Protocol, SPARQL Update, imports) store them as given.

Both are pinned by tests (`tests/shacl_conformance.rs`, `pinned_*`), which
will flip when storage keeps lexical forms.

## On-Demand Validation

```bash
curl -X POST http://localhost:7878/api/datasets/<dataset_id>/validate \
     -H 'Authorization: Bearer <token>'
```

Response (the keys are snake_case):

```json
{
  "report": {
    "conforms": false,
    "results": [
      {
        "severity": "violation",
        "focus_node": "http://example.org/alice",
        "path": "<http://schema.org/name>",
        "value": null,
        "source_shape": "urn:dataset:my-dataset:shapes#PersonShape",
        "source_constraint": "sh:minCount 1",
        "source_constraint_component": "http://www.w3.org/ns/shacl#MinCountConstraintComponent",
        "message": "Expected at least 1 values, found 0"
      }
    ],
    "results_count": 1,
    "metrics": { "path": "dataset", "duration_ms": 4, "quads": 120, "graphs": 1,
                 "source": "snapshot", "run_index": false }
  },
  "run_id": "<run id>",
  "ran_at": "<timestamp>"
}
```

The JSON fields are display strings: `focus_node` and `value` show an IRI or
a literal's lexical form (no datatype or language tag), `path` is a SPARQL
property path, and `source_constraint` is a short label such as
`sh:minCount 1` (the UI groups results by it).
`source_constraint_component` is the SHACL constraint component IRI.
`message` is the shape's `sh:message` when it has one, else a default text.
A test or partial run answers `"run_id": null, "ran_at": null` and adds
`"test": true` and `"partial"`. The 422 body of a write gate uses
camelCase keys instead (`focusNode`, `sourceShape`, `sourceConstraint`,
`sourceConstraintComponent`). A result of a SPARQL constraint or validator
that declares [result annotations](#result-annotations-shresultannotation-shacl-af-4)
also has an `annotations` list; no other result has the key.

The report RDF the run writes (below) is the W3C form: typed `sh:focusNode`
and `sh:value` terms (datatype and language kept), `sh:resultPath` as a SHACL
path structure (`[ sh:inversePath ex:p ]`, RDF lists for sequences),
`sh:sourceConstraintComponent` as the component IRI, `sh:sourceConstraint` for
`sh:sparql` constraints (the `sh:sparql` node) and expression constraints (the
node expression), the declared `sh:severity` IRI, custom ones included, and
any result annotations.

### What a run reads, and who sees its report

A report carries the focus nodes and values of the graphs it validated, so a
run reads only the dataset graphs the caller may read, by the rule `/sparql`
applies: a private graph only for the dataset's writers, plus graph-ACL read
grants. Admins read every graph. A run that could not read every graph of the
dataset is not official: it is answered as a test run (`"test": true,
"partial": true`), is not recorded, and leaves the dataset's validation status
as it was. An explicit `shapes_graph` in the body must be readable by the same
rule.

An official run is recorded with the graphs it validated, and its report is
written as RDF to `urn:system:reports:dataset:<id>`. That graph is private in
the dataset whenever the run validated a private graph (or a model graph not
everyone may read), and it stays private. `GET …/validation/latest` and
`GET …/validation/runs/<run_id>` return the full report to the dataset's
writers and to callers who may read every graph the run validated; anyone else
gets the run's summary (counts, `conforms`) with `"report": null` and
`"report_withheld": true`.

A report also carries its shapes' messages, paths and shape IRIs, so shapes
follow the same rule. A graph some dataset holds as private shapes only the
runs of who may read it: the dataset's private shapes-role graph, or another
dataset's private graph linked or bound as shapes here. A run that leaves one
out is a test run too. `GET /api/datasets/<id>/shapes` and the form manifest
serve such a graph only to who may read it (`GET …/shapes` answers 404 when
that leaves none). Linking one as a dataset's shapes graph
(`PUT /api/datasets/<id>/shacl`) needs that right too. Its readers may link
it into another dataset all the same: that dataset's runs then use it, and its
own writers need not be allowed to read it.

So a stored run also records the shapes graphs it used, and its full report
is withheld from anyone but an admin who may not read one of them that some
dataset holds as private when they ask, the dataset's writers included. An
official run shaped by another dataset's private graph writes no report
graph, and clears the last one. Making a graph private
(`PATCH /api/datasets/<id>/graphs` with `"private": true`) takes the report
graphs on it along: a dataset whose latest official run validated the graph,
or was shaped by it, has its report graph made private when it holds the
graph and cleared when it does not.

---

## Validation on Write

When `shacl_on_write` is `true` on a dataset and a `shapes_graph_iri` is configured, every `PUT` or `POST` to `/store?graph=<graph-iri>` that targets a graph belonging to the dataset is validated before the write is committed.

If validation fails, the write is rejected with **422 Unprocessable Entity** and the JSON report is returned. The store is not modified. A report names the gate's shapes, their paths and messages: when the shapes that refused the write include a graph some dataset holds as private that the writer may not read, the 422 says only that the write does not conform, and by how many results. The same holds for every write gate below, and for bulk import.

The gate fails **closed**: a gate that cannot be evaluated refuses the write with the same 422 and a report naming the cause, never a 204. That covers a shapes graph that cannot be read or copied, a validation-engine error, and an ill-formed shapes graph — in particular a `sh:sparql` constraint whose `sh:select` does not parse (or errors at evaluation) is a violation of the focus node, not a constraint that silently never fires, and a property shape without a usable `sh:path` or a SPARQL target that fails refuses the write rather than being skipped (see [How a shapes graph is read](#how-a-shapes-graph-is-read)). Loading such a shapes graph for on-demand validation fails with an error for the same reason.

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

These apply to every write gate: this per-dataset `shacl_on_write` gate, and the SHACL Studio gates below (validation-layer bindings and pipelines with `gate_writes`).

- Writes are validated on Graph Store `PUT` and `POST` (`/store`), bulk import (`/api/import/bulk`) and `POST /api/datasets/validate-and-commit`.
- SPARQL Update (`/sparql`, `/sparql/batch`) is still not validated, Studio gates included: an update can write data that a gate would refuse on `/store`. To keep a gated graph valid, write it through one of the paths above, or run the pipeline (or `POST /api/datasets/{id}/validate`) after the update.
- Only graphs a gate covers are validated: graphs registered to the dataset, graphs that carry a binding, and graphs in a gating pipeline's scope. Writes to other graphs pass through unchecked.

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

A dataset's shapes graph is adopted into the Library in place when the dataset is validated or imported into, or when its shapes graph or graph roles change. The entry takes the dataset's visibility, except for a graph the dataset holds as private, whose entry is `private`. An entry's visibility does not decide who reads a private graph, however: an entry of a graph some dataset holds as private is shown to, and worked on by, only those who may read that graph by the `/sparql` rule (its dataset's writers, graph-ACL read grants, admins). That covers the entry, its Turtle, revisions and clone, the Library list, bindings, effective shapes, the catalogue and pipelines. It holds for an entry made before the graph was marked private, too, and marking the graph public again gives the entry back.

Impact — *what data a shape graph is applied to* — is the reverse binding lookup: `GET /api/shacl/bindings?shape_graph_id=<shape_graph_id>` → `{ shape_graph_id, targets: [ …IRIs ] }`.

### Pipelines & targets

A pipeline is a saved, runnable validation. Its scope is a set of **targets** — any mix of datasets, graphs, and shape graphs — plus composed shape graphs, a severity threshold, and triggers (manual, on-write, cron). When `gate_writes` is set, writes covered by the pipeline are gated. See `POST /api/shacl/pipelines`; the request body's `targets` is an array of `{ "kind": "dataset"|"graph"|"shapegraph", "id": "…" }`.

A run's report carries the data it validated (focus nodes and values), so a pipeline's whole scope — every dataset, every data graph it resolves to and every shape graph it composes — must be readable by whoever creates or updates it, runs or test-runs it, or opens a stored run's report (`GET /api/shacl/pipelines/{id}/runs/{run_id}`); anything else answers 403. Reading follows the `/sparql` rule above, and a Library shape graph is readable by whoever the Library shows it to. The shapes bound to a dataset or graph in scope come with it, except a graph some dataset holds as private that the caller may not read: a pipeline with one in scope answers 403. The check is made each time, so a revoked grant takes effect at the next run. A scheduled run is checked against the pipeline's creator and skipped when they may no longer read its scope. Run summaries (`…/runs`, counts only) are listed to everyone who can see the pipeline. A report persisted as RDF (`results_target`) or inferred triples written to a new graph are attached to a dataset only when that dataset holds every graph the run validated, and are private there when any of them is private. The pipeline's own report graph collects every run, so a run over other data first detaches it, and it is attached again only while empty.

A pipeline with `gate_writes` refuses (422) every write its shapes reject to the graphs it covers, whoever makes it, the graphs' owners and editors included. So setting a gate (creating or updating a pipeline with `gate_writes`) needs what a validation-layer binding needs: write access to every dataset it covers (dataset targets, and `dataset_ids` while no `graph_iris` narrow the scope) and a graph-ACL write grant on every graph it names (graph targets, `graph_iris`). Admins pass. Anything else answers 403, and a dataset that does not exist 404. Read access is enough only for a pipeline that validates without gating. The gate acts with its creator's authority, checked at every write: once the creator may no longer write what it covers (a revoked grant, a deactivated account), the pipeline stops gating, and the server logs a warning at each write it would have gated. The gate covers the write paths listed under [Limitations](#limitations); SPARQL Update is not gated.

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
# → {"inferred_triples": 42, "partial": false}
```

It needs write access to the dataset and runs the rules of the same shapes
graphs validation uses (the configured shapes graph, SHACL Studio bindings and
`shapes`-role graphs), less those the caller may not read: a graph some dataset
holds as private (another dataset's, linked or bound here) runs only for that
dataset's writers, graph-ACL readers and admins. `partial: true` says a shapes
graph was left out; with none left the call answers 400.

Supports `sh:SPARQLRule` (`sh:construct`) and `sh:TripleRule` (`sh:subject` /
`sh:predicate` / `sh:object`). Inferred triples are written back into the data
graph, and the rules run to a fixed point. A triple rule's three terms are
[node expressions](#node-expressions-shacl-af-6): `sh:this` stands for the
focus node, an IRI or a literal for itself (a literal keeps its datatype), and
a blank node is evaluated per focus node — `sh:object [ sh:path ex:p ]` copies
the focus node's `ex:p` values. The rule derives one triple per combination of
the three result sets, skipping combinations that are no RDF triple (a literal
subject, a predicate that is not an IRI):

```turtle
ex:RectangleShape a sh:NodeShape ; sh:targetClass ex:Rectangle ;
  sh:rule [ a sh:TripleRule ;
    sh:subject sh:this ; sh:predicate ex:area ;
    sh:object [ ex:multiply ( [ sh:path ex:width ] [ sh:path ex:height ] ) ] ] .
```

A blank node that is none of the node-expression kinds, or an expression that
contains itself, fails the run at load, naming the shape. (Before 2026-10 such
a blank node was written into the data as a term, pointing at the shapes
graph's own node.) A rule shape whose target cannot be loaded fails the run
too. The SHACL-AF rule modifiers are honoured:

| Modifier | Effect |
|---|---|
| `sh:order` | Rules run in ascending order (default `0`), so a later rule sees what an earlier one produced within the same pass. |
| `sh:condition` | A shape the focus node must conform to for the rule to fire — below, only adults get `ex:mayVote`. |
| `sh:deactivated true` | On the rule or on its shape: the rule does not run. On a `sh:condition` shape: every node conforms to it, so it does not hold the rule back. |

```turtle
ex:VoterShape a sh:NodeShape ;
  sh:targetClass ex:Person ;
  sh:rule [ a sh:TripleRule ; sh:order 1 ; sh:condition ex:Adult ;
            sh:subject sh:this ; sh:predicate ex:mayVote ; sh:object true ] .
ex:Adult a sh:NodeShape ;
  sh:property [ sh:path ex:age ; sh:minInclusive 18 ] .
```

### What a rule may read and write

A shapes graph is data: anyone who may write a dataset may upload one and run
it. A rule therefore runs with the authority of the dataset it runs for, not
that of the store.

* **`sh:construct` must be a CONSTRUCT query** — `CONSTRUCT { … } WHERE { … }`,
  or the equivalent `INSERT { … } WHERE { … }` form this store also accepts.
  Anything else (a `DELETE`/`INSERT` update, `DROP`, `LOAD`, several
  operations) is refused when the shapes graph loads, naming the shape.
* **It reads the run's data graphs and nothing else.** Whatever `FROM` /
  `FROM NAMED` the query declares is replaced by them, and no named graph is
  available — so a `GRAPH <g>` block inside a rule matches nothing. The same
  holds for a `sh:sparql` constraint and a `sh:SPARQLTarget`.
* **The engine writes, not the rule.** A CONSTRUCT template cannot name a
  graph, and derived triples go to exactly one: the dataset's single data
  graph when it has one, otherwise its own `urn:dataset:{id}:inferred`
  (registered with the `entailment` role, so it is ACL'd, listed and deleted
  with the dataset).
* **`$this` is bound as a term**, never pasted into the query text, so a focus
  node with a hostile lexical form (`sh:targetNode "…"`) is just a term. In an
  expression it is the focus node, as SHACL pre-binding defines it:
  `BIND ($this AS ?x)` copies it, `BOUND ($this)` is true, and `?v = $this`
  compares values (a literal focus node `1` equals `1.0`).

### Validating with the rules' inferences: `sh:entailment`

A shapes graph can ask for its rules to run before every validation of it
(SHACL-AF §8.3) by declaring the `sh:Rules` entailment regime, on any node:

```turtle
<urn:example:shapes> sh:entailment sh:Rules .

ex:AgeRule a sh:NodeShape ; sh:targetSubjectsOf ex:age ;
  sh:rule [ a sh:TripleRule ; sh:subject sh:this ; sh:predicate rdf:type ; sh:object ex:Person ] .
ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ;
  sh:property [ sh:path ex:name ; sh:minCount 1 ] .
```

Validated with this shapes graph, `ex:a ex:age 5 .` violates `ex:PersonShape`:
the rule makes it an `ex:Person`, and it has no name. Without the declaration
the rule does not run during validation and the data conforms.

* **Every validation run** of the shapes graph does this: on-demand dataset
  validation, SHACL Studio pipelines, write gates (Studio gates and the
  dataset's validate-on-write) and the engine's `shacl::validate`. The rules run
  with the engine `/infer` uses — `sh:order`, `sh:condition`, `sh:deactivated`,
  to a fixed point — and the validation sees the data graphs plus everything
  they inferred.
* **Nothing is written.** The run copies its data graphs and the shapes graph
  into a run-local in-memory store, materialises the inferences into a graph of
  its own there, validates the data plus that graph, and drops the store
  (SHACL-AF §8.4 allows exactly this split: the original data plus a dedicated
  inferences graph). The stored data graphs are unchanged after the run, a gate
  stores only what was written, and the regime needs no write access. `/infer`
  is not affected by the declaration: it still materialises, as above.
* **The rules read what they read under `/infer`:** the run's data graphs (and,
  here, the inferences graph), nothing else — see the previous section.
* **Cost:** the copy. A run under a regime holds its data graphs in memory a
  second time, so it suits shapes graphs that validate datasets of moderate
  size. Shapes graphs without the declaration are unaffected.

**Other regimes.** SHACL §1.5 makes every `sh:entailment` value either a regime
the processor validates under or a failure:

| `sh:entailment` value | Behaviour |
|---|---|
| `sh:Rules` | The shapes graph's SHACL rules, as above. |
| `<http://www.w3.org/ns/entailment/RDFS>` | The data's RDFS entailments (rdfs1–rdfs13, the [`rdfs-entailment`](rdfs-entailment.md) materialiser; in `full` through the OWL features) are computed into the same run-local graph. With `sh:Rules` declared too, RDFS and the rules run in turn until neither adds a triple (at most 16 rounds). A build without the feature fails the run, as below. |
| anything else (OWL, D, a literal, …) | The run fails with an error naming the value — the dataset validation route answers with that error, a write gate refuses the write (`422`) — never a report computed as if the declaration were not there. |

---

## SPARQL-based constraints and constraint components

### `sh:sparql` and pre-binding

A `sh:SPARQLConstraint` (`sh:select`) is evaluated once per focus node with
`$this` **pre-bound** as SHACL §5.3 defines it: the focus node reaches every
scope of the query — a `FILTER` in a nested group or a `UNION` branch, a
sub-select that projects `$this`, the projection and `GROUP BY` of an aggregate
— and `bound($this)` is true. The focus node is bound as an RDF term, never
pasted into the query text, so blank-node focus nodes are checked like any
other (they used to be skipped, so their constraints never ran) and a literal
cannot change the query. On a property shape, `$PATH` is replaced by the
shape's path. A constraint with `sh:deactivated true` produces no results.

Every solution is a violation (§5.3.2):

* `?value` becomes `sh:value`, and `?path` becomes `sh:resultPath` when it is
  an IRI (otherwise the shape's path is used).
* The message is the solution's `?message` binding if there is one, else the
  constraint's `sh:message` with every `{?var}` / `{$var}` replaced by that
  variable's binding in the solution. A block naming an unbound variable is
  left as written.
* A solution that binds `?failure` to `true` is a **failure**, not a result.
  The run reports it as a violation that the constraint could not be
  evaluated, so the focus node does not conform and a write gate refuses the
  write.

The features the specification forbids under pre-binding (Appendix A) —
`MINUS`, `VALUES`, `SERVICE`, a nested `SELECT` that does not project every
pre-bound variable explicitly (`SELECT *` included), and assigning to a
pre-bound variable (`… AS $this`) — make the shapes graph **fail to load**, so
a constraint that uses them fails loudly instead of silently never firing. `$shapesGraph` and
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
  When the component has a **single** parameter, each value the shape gives it
  is a constraint of its own (`ex:forbidden "red", "blue"` checks both). When
  it has several, a shape that gives any of them more than one value is
  ill-formed and fails the shapes graph (SHACL §4).
* **Validators** — `sh:nodeValidator` (node shapes), `sh:propertyValidator`
  (property shapes) or `sh:validator` (either). An `sh:ask` validator runs once
  per value node with `$this`, `$value` and the parameters pre-bound; `false`
  is a violation. An `sh:select` validator runs once per focus node; every row
  is a violation, with `?value`, `?path`, `?message` and `?failure` read as
  for `sh:sparql`. `$PATH` is available in property validators. Blank-node
  focus and value nodes are pre-bound like any other term. A sub-select inside
  a validator must project every pre-bound variable, the parameters included.
* **`sh:message`** on the validator is the result message, falling back to the
  component's own `sh:message`, with `{$param}`, `{?param}`, `{$this}` and
  `{$value}` — and, for a SELECT validator, any variable of the solution —
  rendered; `source_constraint` and `source_constraint_component` name the
  component.
* **`sh:deactivated true`** on a validator takes it out: the shape falls back
  to `sh:validator`, and if no validator is left the component checks nothing.

A component a shape uses without a validator for the shape's kind, or a
validator that does not parse, fails the shapes graph.

### SPARQL functions (`sh:SPARQLFunction`, SHACL-AF §5)

A shapes graph can declare functions its own `sh:sparql` constraints,
validators, SPARQL targets and rules call:

```turtle
ex:double a sh:SPARQLFunction ;
  sh:parameter [ sh:path ex:x ; sh:order 0 ] ;
  sh:returnType xsd:integer ;
  sh:select "SELECT ($x * 2 AS ?result) WHERE {}" .
```

* **Scope.** A function belongs to the runs of the shapes graph that declares
  it: validation, inference, Studio pipelines and write gates that use that
  graph. No other shapes graph's run sees it, and `/sparql`, SPARQL Update and
  the reasoners do not see it at all.
* **Reserved IRIs.** A function may not take an IRI in the `xsd:`, `rdf:`,
  `rdfs:`, `owl:`, `sh:`, `sparql:`, XPath `fn:`/`math:` or GeoSPARQL `geof:`
  namespaces, in the server's own function namespace
  (`https://open-triplestore.org/def/function/`), or any IRI the server
  registers itself (GeoSPARQL functions and aggregates, the 3D functions,
  RDF 1.2, `ADJUST`). The SPARQL engine consults custom functions before its
  `xsd:` casts, so a definition at such an IRI would change what every caller
  computes. A shapes graph that declares one fails its run, naming the
  function.
* **Functions for every query.** An admin can make functions callable from
  `/sparql` by storing them in a graph named in `OTS_SPARQL_FUNCTION_GRAPHS`
  (see [administration](administration.md)). Only `urn:system:functions` and
  graphs under `urn:system:functions:` can be named: no dataset can hold such
  a graph, so only an admin can write one. Shapes runs see these functions too,
  and a shapes graph may not redefine one.
* **Bodies.** `sh:select` (a `SELECT` with exactly one result variable; the
  function returns its binding in the first solution) or `sh:ask` (the
  function returns the ASK result as `xsd:boolean`). The `sh:prefixes`
  prologue follows `owl:imports`, as for constraints. A body that does not
  parse, or a `SELECT` with more or fewer than one result variable, fails the
  run of the shapes graph that declares it.
* **Arguments are bound as terms.** Each argument is bound to its parameter's
  variable (the local name of its `sh:path`) as an RDF term, never pasted into
  the query text. Parameters are ordered by `sh:order` (0 when unset) when any
  has one, otherwise by the local names of their paths. A call without a
  mandatory argument, or with too many, is an error — unbound in a `BIND`;
  `sh:optional true` parameters may be left out.
* **Bodies read the run's data.** A body called from a shapes run reads that
  run's data graphs, from the same snapshot the rest of the run reads —
  `SELECT ?l WHERE { $node rdfs:label ?l }` returns the node's label. Like the
  constraint calling it, a body cannot widen its dataset: whatever `FROM`,
  `FROM NAMED` or `GRAPH` it names, it reads the run's data graphs (the default
  graph when the run names none) and no named graph. A function called from
  `/sparql` (from a designated graph) has no run to read and sees an empty
  dataset, so it can compute from its arguments only.
* **Recursion is bounded.** Calls may nest 16 deep (a function whose body calls
  a function …); a call beyond that is unbound and logged, so a function that
  calls itself ends instead of exhausting the stack.

### Custom targets (SHACL-AF §3)

`sh:target` gives a shape focus nodes computed by SPARQL, and makes its subject
a shape even with no `rdf:type sh:NodeShape`:

* **A SPARQL-based target** has a `sh:select` that projects `?this`.
* **A SPARQL-based target type** is a class declared `a sh:SPARQLTargetType`
  with a `sh:select` and `sh:parameter`s; a target that is an instance of it
  supplies the parameter values, which are bound into the query as terms:

```turtle
ex:BornIn a sh:SPARQLTargetType ; rdfs:subClassOf sh:Target ;
  sh:parameter [ sh:path ex:country ] ;
  sh:select "SELECT ?this WHERE { ?this ex:bornIn $country }" .
ex:DutchCitizenShape a sh:NodeShape ;
  sh:target [ a ex:BornIn ; ex:country ex:NL ] ;
  sh:property [ sh:path ex:name ; sh:minCount 1 ] .
```

  A target that lacks a value for a non-optional parameter selects nothing
  (§3.2). A parameter value that is a blank node, or two values for one
  parameter, fail the shapes graph.

Both read the run's data graphs only. A `sh:target` with neither a `sh:select`
nor a `sh:SPARQLTargetType` type fails the shapes graph: the engine cannot
compute its focus nodes, and a shape that validated nothing would pass every
write.

### Result annotations (`sh:resultAnnotation`, SHACL-AF §4)

The node that carries a `sh:sparql` constraint's query, or a component
validator's `sh:select` / `sh:ask`, can declare extra properties for the
results that query produces:

```turtle
ex:AnnotationExample a sh:NodeShape ;
  sh:targetNode ex:ExampleResource ;
  sh:sparql [
    sh:resultAnnotation [ sh:annotationProperty ex:time ; sh:annotationVarName "time" ] ;
    sh:select """
      SELECT $this ?message ?time WHERE {
        BIND (CONCAT("The ", "message.") AS ?message) .
        BIND (NOW() AS ?time) .
      }""" ] .
```

* Each result of a solution gets `sh:annotationProperty` set to the solution's
  binding of `sh:annotationVarName` — or, without one, of the property's local
  name (`ex:time` → `?time`).
* When that variable is unbound, the annotation's `sh:annotationValue`s are
  used instead, if it has any. An `sh:ask` validator has no solution to read,
  so its results get the `sh:annotationValue`s only.
* The RDF report writes each annotation as a property of the
  `sh:ValidationResult` node, typed (`ex:time "…"^^xsd:dateTime`). The JSON
  result lists them under `annotations`, as `{"property": IRI, "value":
  display string}`, and so does the 422 body of a write gate; results without
  annotations have no `annotations` key.
* An annotation that is a literal, has no or several `sh:annotationProperty`
  values (or one that is not an IRI), or more than one `sh:annotationVarName`
  (or one that is not a string) fails the shapes graph.

### Node expressions (SHACL-AF §6)

A node expression computes a set of nodes for a focus node. Triple rules use
them for their three terms, and expression constraints for their condition.
All seven kinds of the 2017 Note are evaluated:

| Kind | Syntax | Produces |
|---|---|---|
| Focus node | `sh:this` | the focus node |
| Constant | any other IRI, or a literal | that term |
| Path | `[ sh:path P ; sh:nodes N ]` | the values of `P` from each node of `N` (the focus node when `sh:nodes` is absent) |
| Filter shape | `[ sh:filterShape S ; sh:nodes N ]` | the nodes of `N` that conform to `S` |
| Intersection | `[ sh:intersection ( E1 E2 … ) ]` | the nodes every `Ei` produces |
| Union | `[ sh:union ( E1 E2 … ) ]` | the nodes any `Ei` produces |
| Function | `[ f ( E1 E2 … ) ]` | `f` called with every combination of the `Ei`'s nodes |

A function can be a `sh:SPARQLFunction` of the shapes graph, a designated
function, or a built-in one (`geof:distance`, an `xsd:` cast). A mandatory
argument whose expression produces nothing means no call; a call whose result
is unbound produces no node; more than 10 000 calls for one focus node fail the
evaluation. Paths read the run's data graphs, like every other path.

### Expression constraints (`sh:expression`, SHACL-AF §7)

`sh:expression` holds a node expression that must produce exactly `{ true }`
for each value node (the focus node on a node shape), evaluated with that node
as its focus node. Anything else — `false`, another value, several values, or
nothing — is a result whose `sh:value` is the value node and whose
`sh:sourceConstraint` is the node expression; the expression node's
`sh:message` is the result message:

```turtle
ex:atLeast a sh:SPARQLFunction ;
  sh:parameter [ sh:path ex:value ; sh:order 1 ] ;
  sh:parameter [ sh:path ex:minimum ; sh:order 2 ] ;
  sh:ask "ASK { FILTER ($value >= $minimum) }" .

ex:ClearanceShape a sh:NodeShape ; sh:targetClass ex:NavigableBridge ;
  sh:expression [ sh:message "Clearance must be at least 9.10 m" ;
    ex:atLeast ( [ sh:path ( ex:clearanceHeight qudt:numericValue ) ] 9.10 ) ] .
```

Before 2026-10 this engine read a form of its own here: a path plus comparison
constraints on the expression node, `sh:expression [ sh:path P ;
sh:minExclusive 9.09 ]`. That node is a plain path expression under the Note,
so a shapes graph written that way now reports every focus node whose values
are not `true`. Rewrite it as a function expression, as above, or as a property
shape (`sh:property [ sh:path P ; sh:minExclusive 9.09 ]`).

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

### What the serializer leaves out

`/api/shaclc/serialize` (and `Accept: text/shaclc`) writes only part of a shapes graph, and
**drops the rest without a warning or a comment** in the output:

- Only subjects typed `sh:NodeShape` are written, each with its first `sh:targetClass` and
  `sh:closed`; other targets are dropped.
- Per property shape it writes the path, `sh:datatype`, `sh:nodeKind`, `sh:node`,
  `sh:minCount`/`sh:maxCount`, `sh:pattern` and `sh:message`. Node-level constraints and
  `sh:class`, `sh:in`, `sh:hasValue`, value ranges, string lengths, the logical constraints and
  SPARQL-based constraints are dropped. A property shape without `sh:path` is skipped.
- A complex property path (sequence, inverse, alternative) comes out as a blank-node label,
  which the parser cannot read back.
- `sh:pattern` is written without escaping.

So a SHACL-C export is not a faithful copy of a shapes graph; keep Turtle as the source of
truth.

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
  the match semantics differ; a flagged one is dropped. So is every pattern
  after the first: several `sh:pattern` values must all match, while several
  `xs:pattern` facets are alternatives. Likewise only the first of several
  `sh:hasValue` values is exported, and a deactivated shape or property shape
  is not exported at all.
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

