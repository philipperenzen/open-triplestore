# Linked Data Modelling Styleguide

**The canonical standard for modelling linked data in the Open Triplestore ecosystem.**

| | |
|---|---|
| **Status** | Normative — canonical source of truth |
| **Applies to** | Open Triplestore and compatible tools |
| **Supersedes** | Ad-hoc conventions in individual repos |
| **Companion docs** | [`data-modeling.md`](data-modeling.md), [`dcat.md`](dcat.md), [`shacl.md`](shacl.md), [`versioning.md`](versioning.md) |

This document is the **single source of truth** for how we model, store, validate, and describe linked data. The triplestore implements it; companion tools (a graph viewer, a form platform, validation services) can teach, enforce, and generate against it. When another document, prompt, or piece of code disagrees with this one, **this document wins** — fix the other.

---

## How to read this document

Requirement keywords follow [RFC 2119](https://www.rfc-editor.org/rfc/rfc2119):

- **MUST** / **MUST NOT** — a hard rule. Data that breaks it is wrong; tooling will reject or flag it.
- **SHOULD** / **SHOULD NOT** — a strong default. Deviate only with a documented reason.
- **MAY** — an allowed option with no preference.

All Turtle/TriG examples in this document are normative reference style: copy their shape, not just their content.

### Table of contents

1. [The layered model](#1-the-layered-model)
2. [Graph roles and named graphs](#2-graph-roles-and-named-graphs)
3. [IRIs, namespaces and naming](#3-iris-namespaces-and-naming)
4. [Layer 1 — the Knowledge Model (ontology)](#4-layer-1--the-knowledge-model-ontology)
5. [Layer 2 — the Information Model (SHACL constraints)](#5-layer-2--the-information-model-shacl-constraints)
6. [Layer 3 — Instance Data](#6-layer-3--instance-data)
7. [Dataset, catalogue and organisation metadata (DCAT / VoID / ADMS / ORG)](#7-dataset-catalogue-and-organisation-metadata-dcat--void--adms--org)
8. [Versioning and lifecycle](#8-versioning-and-lifecycle)
9. [Provenance (PROV-O)](#9-provenance-prov-o)
10. [Domain conventions (optional)](#10-domain-conventions-optional)
11. [Semantic validity rules — the do/don't checklist](#11-semantic-validity-rules--the-dodont-checklist)
12. [Worked end-to-end example](#12-worked-end-to-end-example)
- [Appendix A — Namespace & prefix registry](#appendix-a--namespace--prefix-registry)
- [Appendix B — Graph-role detection cheat-sheet](#appendix-b--graph-role-detection-cheat-sheet)
- [Appendix C — Dataset metadata → RDF field mapping](#appendix-c--dataset-metadata--rdf-field-mapping)
- [Appendix D — Conformance checklist](#appendix-d--conformance-checklist)
- [Appendix E — Where this standard is enforced](#appendix-e--where-this-standard-is-enforced)

---

## 1. The layered model

Every knowledge graph we build separates into layers. Confusing the layers is the single most common modelling mistake, so always know which layer you are working in.

| # | Layer | Question it answers | Primary vocabularies | Description-logic term |
|---|---|---|---|---|
| 1 | **Knowledge Model** (ontology) | "What kinds of things exist and how can they relate?" | SKOS, SKOS-XL, RDFS, OWL | T-Box |
| 2 | **Information Model** (constraints) | "What makes a piece of data valid?" | SHACL (ShEx) | — |
| 3 | **Instance Data** | "Which specific things are we describing?" | the model's own terms + RDF | A-Box |

Three cross-cutting layers wrap these:

| Layer | Question | Vocabularies |
|---|---|---|
| **Catalogue & statistics** | "What datasets exist, where, under what licence, how big?" | DCAT, VoID, ADMS |
| **Provenance** | "Who made this, when, derived from what?" | PROV-O |
| **Organisations & agents** | "Who owns/publishes/maintains this?" | ORG, FOAF, vCard |

> **Mental model.** Layer 1 is the *dictionary*, Layer 2 is the *rulebook*, Layer 3 is the *story*. The catalogue is the *library index*, provenance is the *audit trail*.

You **MUST NOT** mix layers in a single graph (see §2). A class definition, a SHACL shape, and an instance of that class belong in three different named graphs.

---

## 2. Graph roles and named graphs

The triplestore is quad-based: every triple lives in a **named graph**, and every named graph has a **role**. Roles are the operational form of the layered model.

### 2.1 The six graph roles

The store recognises exactly six roles (`GraphKind` in [`src/auth/models.rs`](../src/auth/models.rs)):

| Role | Layer | Holds | Registered under |
|---|---|---|---|
| `model` | 1 | OWL/RDFS class & property axioms (the T-Box) | Ontology Registry |
| `vocabulary` | 1 | SKOS concept schemes & concepts | Ontology Registry |
| `shapes` | 2 | SHACL node/property shapes | Ontology Registry |
| `entailment` | — | SWRL / SPIN rule sets | Ontology Registry |
| `instances` | 3 | Concrete facts (the A-Box) | Datasets |
| `system` | — | Internal bookkeeping (`urn:system:*`) | reserved |

**Rules:**

- A named graph **MUST** hold exactly one role. Do not put shapes in your model graph or instances in your vocabulary graph.
- Instance data **MUST** be registered as a **Dataset**; models/vocabularies/shapes **MUST** be registered in the **Ontology Registry** with a version lifecycle (§8).
- The role of each registered graph is published in the DCAT catalogue as `ots:graphRole` on the graph IRI (§7).

### 2.2 Automatic role detection

When you upload RDF, the store classifies it in a single pass over the quads ([`src/kind_detector.rs`](../src/kind_detector.rs)). It tallies evidence and picks the dominant signal:

| Evidence counted | Pushes toward |
|---|---|
| `owl:Ontology`, `owl:Class`, `owl:ObjectProperty`/`DatatypeProperty`/`AnnotationProperty`, `rdf:Property`, `rdfs:Class` | `model` |
| `skos:ConceptScheme`, `skos:Concept`, any `skos:` predicate | `vocabulary` |
| `sh:NodeShape`, `sh:PropertyShape`, `sh:targetClass`, any `sh:` predicate | `shapes` |
| `swrl:Imp`, any `spin:`/`sp:` predicate | `entailment` |
| a subject typed with a class that is **not** a schema construct | `instances` |

The heuristic is **dominance-based**: a role wins when its evidence is roughly 3× the competing schema signals. A balanced mix (e.g. equal OWL classes and SKOS concepts) is reported as **ambiguous** (`primary: None`, `mixed: true`) and is left unclassified for a human to label.

**What this means for you:**

- **Keep each graph single-purpose** so detection is unambiguous. This is the same rule as §2.1, stated operationally.
- If you upload a mixed graph, **classify it explicitly** with the `?kind=` override (`model`/`tbox`, `vocabulary`/`vocab`, `shapes`, `entailment`, `instances`/`abox`) rather than relying on the heuristic.
- The detector is a safety net, not a substitute for clean modelling.

### 2.3 Dual-typing vs. role detection — important reconciliation

The Knowledge Model pattern (§4) uses **dual typing**: a concept is declared `a skos:Concept , owl:Class` so it is both a navigable vocabulary entry *and* a formal class. This is correct and encouraged.

But dual typing produces **both** OWL-class and SKOS evidence in the same graph. Resolve it like this:

- A dual-typed **knowledge model graph** is normally detected as `vocabulary` when SKOS hierarchy predicates (`skos:broader`, `skos:inScheme`, …) dominate — which they usually do, because every concept carries several. This is the intended outcome: **register a dual-typed concept scheme as a Vocabulary.**
- Keep **OWL-only** axioms that have no SKOS counterpart (e.g. `owl:Restriction`, `owl:disjointWith`, property characteristics) viable as a separate `model` graph if they grow large, or accept the `model` classification when OWL dominates.
- **Always keep SHACL shapes in their own `shapes` graph.** Never co-locate shapes with the concept scheme — that muddies both detection and the lifecycle.
- When in doubt, set `?kind=` explicitly. Do not contort your modelling to please the detector.

### 2.4 Serialisation: TriG for multi-graph documents

When a single document carries more than one graph (e.g. concepts + shapes + instances + catalogue), use **TriG** and put each role in its own named graph:

```trig
@prefix ex:   <https://example.org/showcase/> .
@prefix skos: <http://www.w3.org/2004/02/skos/core#> .
@prefix sh:   <http://www.w3.org/ns/shacl#> .

ex:vocabulary { ex:Book a skos:Concept , owl:Class ; skos:prefLabel "Book"@en . }
ex:shapes     { ex:BookShape a sh:NodeShape ; sh:targetClass ex:Book . }
ex:instances  { ex:Dracula a ex:Book . }
```

Single-role documents **SHOULD** use Turtle. Use N-Triples/N-Quads for bulk interchange, JSON-LD for web APIs, RDF/XML only when a consumer demands it.

---

## 3. IRIs, namespaces and naming

IRIs are the most permanent thing you create. A bad IRI outlives the data it names.

### 3.1 IRI design rules

- IRIs **MUST** be `http(s)` and dereferenceable in principle (resolve to a description of the thing).
- IRIs **MUST NOT** contain spaces, and **SHOULD NOT** contain characters that need percent-encoding.
- Prefer **named nodes over blank nodes** for anything that another graph might reference (every concept, class, property, instance of record). Blank nodes are acceptable only for genuinely anonymous structured values (a geometry, a contact card, a SHACL constraint list).
- An IRI's local name **SHOULD** be opaque-stable: do not encode mutable facts (status, owner, year) into it. Put those in triples.
- Do not reuse one IRI for two different things. Do not mint two IRIs for one thing — link them with `owl:sameAs` / `skos:exactMatch` if it already happened.

### 3.2 Naming conventions

| Resource | Case | Example |
|---|---|---|
| Class / Concept | `PascalCase` | `ex:Ebook` |
| Property | `camelCase` | `ex:pageCount`, `ex:publishedBy` |
| Object property linking to a resource | `camelCase`, verb or `has…` | `ex:author`, `ex:hasCoverImage` |
| Individual / instance | `PascalCase` or stable code | `ex:Dracula`, `ex:BOOK-0042` |
| Concept scheme | `PascalCase` noun | `ex:PublicationVocabulary` |
| SHACL shape | target class + `Shape` | `ex:BookShape` |

Every term **SHOULD** also carry a short `skos:notation` / `dct:identifier` for compact, language-neutral reference (e.g. `"BOOK"`).

### 3.3 The triplestore's IRI scheme

The store mints and serves IRIs in a fixed namespace anchored at the configured `--base-url` (default `http://localhost:7878`). Use these patterns; do not invent parallel ones.

| Pattern | Names |
|---|---|
| `{base}/catalog` | the DCAT catalogue |
| `{base}/dataset` | the aggregate VoID dataset (whole store) |
| `{base}/dataset/{id}` | a registered dataset |
| `{base}/org/{id}` | an organisation |
| `{base}/user/{id}` · `{base}/group/{id}` | user / group owners |
| `{base}/data-model/{id}` · `{base}/data-model/{id}/version/{semver}` | a data model and a specific version |
| `{base}/vocabulary/{id}` · `{base}/vocabulary/{id}/version/{semver}` | a vocabulary and a specific version |
| `{base}/sparql` · `{base}/store` | the SPARQL and Graph Store endpoints |
| `{base}/resource/{local}` | a dereferenceable resource (content-negotiated) |

In production **always** set a stable public base URL so minted IRIs are durable:

```bash
./open-triplestore --base-url https://triplestore.example.com
```

### 3.4 Prefixes

Declare every prefix you use; never rely on a reader's defaults. Use the **registered prefixes** in [Appendix A](#appendix-a--namespace--prefix-registry) — these are the exact short names the triplestore emits. Do not redefine a well-known prefix to a different namespace.

---

## 4. Layer 1 — the Knowledge Model (ontology)

**Goal:** define a shared vocabulary so humans and machines agree on terminology. We combine **SKOS** (navigable concept hierarchy, labels) with **RDFS/OWL** (formal class semantics) using the **dual-typing** pattern.

### 4.1 The concept scheme container

Every controlled vocabulary **MUST** be wrapped in a `skos:ConceptScheme` that declares its top concepts and carries scheme-level metadata.

```turtle
@prefix ex:   <https://example.org/showcase/> .
@prefix skos: <http://www.w3.org/2004/02/skos/core#> .
@prefix dct:  <http://purl.org/dc/terms/> .
@prefix vann: <http://purl.org/vocab/vann/> .
@prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .

ex:PublicationVocabulary a skos:ConceptScheme ;
    skos:prefLabel "Publication Vocabulary"@en , "Publicatievocabulaire"@nl ;
    dct:title "Publication Classification"@en ;
    dct:creator <https://example.org/agent/JanDeVries> ;
    dct:created "2024-01-15"^^xsd:date ;
    owl:versionInfo "2.1.0" ;
    vann:preferredNamespacePrefix "pub" ;
    vann:preferredNamespaceUri "https://example.org/showcase/" ;
    skos:hasTopConcept ex:Publication , ex:Periodical .
```

### 4.2 Dual-typed concepts

Each concept **MUST** be typed `a skos:Concept , owl:Class` and **MUST** carry:

- at least one `skos:prefLabel` per language (NL **and** EN — see §10);
- a `skos:definition`;
- a `skos:notation`;
- `skos:inScheme` linking it to its scheme.

It **SHOULD** also carry `skos:broader` (its parent) **and** the parallel `rdfs:subClassOf`, plus external mappings.

```turtle
ex:Book a skos:Concept , owl:Class ;
    skos:prefLabel "Book"@en , "Boek"@nl ;
    skos:altLabel "Volume"@en ;
    skos:definition "A written work published as a bound or digital volume."@en ;
    skos:scopeNote "Includes fiction, non-fiction and reference titles."@en ;
    skos:notation "BOOK" ;
    dct:identifier "BOOK-001" ;
    skos:inScheme ex:PublicationVocabulary ;
    skos:broader ex:Publication ;
    skos:narrower ex:Ebook ;
    skos:related ex:Audiobook ;
    rdfs:subClassOf ex:Publication ;
    skos:exactMatch <http://dbpedia.org/resource/Book> .
```

> **Why dual typing?** `skos:Concept` makes the term browsable, labelled and mappable; `owl:Class` lets instances be typed with it and lets reasoners classify. One resource, two complementary views. See §2.3 for how this interacts with graph-role detection.

### 4.3 Properties

Define properties explicitly as `owl:DatatypeProperty` (value is a literal) or `owl:ObjectProperty` (value is an IRI), each with `rdfs:domain`, `rdfs:range`, and multilingual `rdfs:label`.

```turtle
ex:pageCount a owl:DatatypeProperty ;
    rdfs:label "Page count"@en , "Aantal pagina's"@nl ;
    rdfs:domain ex:Book ;
    rdfs:range xsd:integer .

ex:publishedBy a owl:ObjectProperty ;
    rdfs:label "Published by"@en , "Uitgegeven door"@nl ;
    rdfs:domain ex:Publication ;
    rdfs:range org:Organization ;
    owl:inverseOf ex:publishes .
```

- Quantities **SHOULD** reference a unit vocabulary — **QUDT** (`unit:GM`, `unit:MilliM`) or **OM** — rather than baking units into property names. Where a bare number is unavoidable, name the unit in the label (`"Weight (g)"@en`).
- Use OWL characteristics (`owl:FunctionalProperty`, `owl:TransitiveProperty`, `owl:SymmetricProperty`, `owl:inverseOf`) where they genuinely hold — they drive reasoning under the supported OWL 2 profiles (RL/EL/QL/DL).

### 4.4 External alignment

Link your terms to existing vocabularies instead of re-inventing them:

| Relation | Use when |
|---|---|
| `skos:exactMatch` | the two concepts are interchangeable |
| `skos:closeMatch` | nearly the same; safe for most uses |
| `skos:broadMatch` / `skos:narrowMatch` | one is more general than the other |
| `skos:relatedMatch` | associatively related, no hierarchy |
| `owl:equivalentClass` / `owl:sameAs` | formally identical class / individual |

Keep cross-vocabulary alignments (linksets) in a **dedicated named graph**, separate from the scheme itself.

---

## 5. Layer 2 — the Information Model (SHACL constraints)

**Goal:** guarantee data quality. The Knowledge Model *says* a Book has a `pageCount`; the Information Model *ensures* it is an integer between 1 and 5000. We use **SHACL**.

### 5.1 Shape basics

- A shape **MUST** bind to the model via `sh:targetClass` (or another `sh:target…`).
- Name a node shape after its target class plus `Shape`.
- Each `sh:property` block constrains one path; supply `sh:message` (multilingual) so violations are human-readable.
- Use `sh:severity sh:Violation` (default, blocks) vs `sh:Warning` / `sh:Info` (advisory) deliberately.

```turtle
@prefix sh:   <http://www.w3.org/ns/shacl#> .
@prefix ex:   <https://example.org/showcase/> .
@prefix skos: <http://www.w3.org/2004/02/skos/core#> .
@prefix rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .

ex:BookShape a sh:NodeShape ;
    sh:targetClass ex:Book ;
    sh:property [
        sh:path skos:prefLabel ;
        sh:datatype rdf:langString ;
        sh:minCount 1 ;
        sh:uniqueLang true ;
        sh:message "Every book needs a preferred label, one per language."@en ;
    ] ;
    sh:property [
        sh:path skos:notation ;
        sh:datatype xsd:string ;
        sh:minCount 1 ; sh:maxCount 1 ;
        sh:pattern "^BOOK" ;
        sh:message "Notation must start with 'BOOK'."@en ;
    ] ;
    sh:property [
        sh:path ex:pageCount ;
        sh:datatype xsd:integer ;
        sh:minInclusive 1 ; sh:maxInclusive 5000 ;
        sh:message "Page count must be 1–5000."@en ;
    ] ;
    sh:property [
        sh:path ex:rating ;
        sh:datatype xsd:integer ;
        sh:maxCount 1 ;
        sh:in ( 1 2 3 4 5 ) ;
        sh:message "Rating must be 1–5."@en ;
    ] .
```

Common constraint components: `sh:minCount`/`sh:maxCount` (cardinality), `sh:datatype`/`sh:class`/`sh:nodeKind` (type), `sh:pattern` (regex), `sh:minInclusive`/`sh:maxInclusive` (range), `sh:in` (enumeration), `sh:uniqueLang` (one label per language), `sh:hasValue`, `sh:node` (nested shape).

### 5.2 Two levels of validation

The store validates at two distinct points (see [`data-modeling.md`](data-modeling.md)):

1. **Model validation** — are the shapes/axioms themselves well-formed? Run in the Ontology Registry before publishing a version.
2. **Instance validation** — does the A-Box satisfy the shapes? Run against a dataset: `POST /api/datasets/{id}/validate`.

**Shape resolution order** for a dataset:

1. dataset-specific shapes graph (`shapes_graph_iri` on the dataset), if set;
2. otherwise the SHACL shapes from the ontology version the dataset declares via `conforms_to_ontology` + `conforms_to_version`.

This lets many datasets share one model's shapes without copying them. Enable `shacl_on_write` to validate every write before it commits.

### 5.3 SHACL vs OWL — closed vs open world

This is the most consequential choice in the stack:

- **OWL** is **open-world**: what is not stated is *unknown*, not false. OWL *infers* new facts. Use it for classification and entailment.
- **SHACL** is **closed-world** over the data graph: a missing required property is a *violation*. SHACL *validates*; it does not infer.

Do not expect SHACL to "discover" missing types via reasoning, and do not expect OWL to "reject" incomplete data. Use each for its job. (ShEx is available as an alternative shape language; SHACL is the default.)

### 5.4 SHACL Studio — reusable shape graphs, the validation layer, pipelines, write-gating

A shape graph belongs in the Library, not glued to a single dataset. The
**SHACL Studio** (under `/shacl` in the UI) consolidates the workspace:

- **Shape graphs** are first-class, owner-scoped, versioned artifacts with a
  lifecycle status (`draft → staged → published → deprecated`). One shape graph
  can be composed into many pipelines; one pipeline can compose many shape graphs.
  Each dataset's legacy `shapes_graph_iri` is auto-wrapped as a shape graph at
  startup, so existing setups carry over.
- **Everything is a graph; shapes are discoverable.** A shape graph *is* a named
  graph that holds SHACL (default `urn:shapes:{uuid}`), so the Studio's **Shapes**
  catalog (`GET /api/shacl/shapes`) sweeps *every* non-system graph — including
  shapes "joined with instance data" inside a data graph — and lets you pick any
  shape to **compose** into a shape graph: copied with its full blank-node closure
  (`POST …/shape-graphs/{id}/import-shapes`), into a new shape graph or an existing
  one. A pre-existing shapes-bearing graph can instead be **registered in place**
  (`POST /api/shacl/register-shape-graph`) — adopted as a Library artifact without
  copying. Each shape graph surfaces **what data it is applied to** (the reverse
  of its validation-layer bindings), so impact is one query away.
- **The validation layer** is the source of truth for *what validates what*. It
  is **RDF, not config**: every shape↔target link is a triple in the system
  graph `urn:system:validation-layer`, so the wiring is queryable, exportable
  and versioned alongside the data it governs. The object is always a shape
  set's backing graph; the subject (target) may be a dataset, a named graph, or
  another shape graph (for meta-validation, §5.6):

  ```turtle
  # "<target> is validated by <shape-graph graph>".
  <https://ots.example.org/datasets/abc123>  ots:validatedBy <urn:shapes:books> .
  <https://example.org/graph/authors>         ots:validatedBy <urn:shapes:authors> .
  # A standards-friendly mirror is emitted too, so external tools see it:
  <https://ots.example.org/datasets/abc123>  dct:conformsTo  <urn:shapes:books> .
  ```

  Target IRIs are canonical: a dataset is `{base}/datasets/{id}`, a graph is its
  own IRI, a shape graph is its backing `graph_iri`.
- **Shapes travel with the graph.** A binding on a *named graph* is inherited by
  **every dataset that mounts that graph** — dynamically, not by copying. Attach
  shapes to a graph, add that graph to a second dataset, and the dataset is
  immediately validated against them. A dataset's **effective shapes** are the
  union of its own dataset-level bindings and the graph-level bindings of every
  graph it contains. One resolver computes this for write-gating, pipeline runs
  and the form-manifest, so all three agree.
- **Validation pipelines** are the *standardised run* on top of the layer. A
  pipeline saves a set of **targets** (any mix of datasets, graphs and shape
  sets) + composed shape graphs + a severity threshold + triggers, and is runnable
  manually, on every write, and on a 5-field cron schedule (UTC). The legacy
  `dataset_ids` / `graph_iris` scope fields still resolve (additively) for
  existing pipelines.
- **Write-gating** — a write is gated when **either** a `gate_writes` pipeline
  covers the target graph **or** a binding in the validation layer applies to it
  (on the graph itself or via its owning dataset). The on-write hook evaluates
  the relevant shapes against the incoming data in a throwaway store and
  **rejects** writes that meet or exceed the threshold with HTTP **422** and the
  SHACL `ValidationReport` as the body. A binding alone gates (at the default
  `Violation` threshold) — no pipeline required — so graph-attached shapes are
  enforced wherever the graph is mounted. The legacy per-dataset
  `shacl_on_write` boolean still works for back-compat and runs alongside.
- **Versioning & branching.** Shape graphs carry a lifecycle and a commit history
  (logged with a `Shapes` commit kind). When a dataset version is snapshotted,
  the dataset's effective bindings are captured into a version-scoped
  `{base}/dataset/{id}/version/{ver}/validation` graph; restoring the version
  re-applies them (tolerant of targets that no longer exist). So "which shapes
  governed this dataset at version *N*" is answerable, and the validation layer
  branches and restores together with the data it describes.

This means the validation experience scales the same way the rest of the
ecosystem does: shapes get reused, the wiring is data you can query, pipelines
codify "what we check, when and how hard," and runs are persisted under each
pipeline for trend analysis.

### 5.5 Publishing shapes for an external form platform (the form-manifest contract)

The triplestore does **not** render forms. It publishes the dataset and its
attached shapes so an external form platform can load them itself.
The contract is one endpoint per dataset:

```
GET /api/datasets/{dataset_id}/form-manifest
```

Authentication: **optional**. Anyone can read a public dataset's manifest;
member/private datasets require a session or a `ShareLink` token. The response
honours the dataset's existing ACL.

Response (JSON):

```jsonc
{
  "version": 1,
  "dataset": { "id", "name", "description", "visibility", "owner_type", "owner_id" },
  "base_url": "https://ots.example.org",
  "prefixes": { "rdf": "...", "rdfs": "...", "xsd": "...", "owl": "...",
                "sh": "...", "skos": "...", "dcterms": "...", "geo": "...", "qudt": "..." },
  // The dataset's *effective* shape graphs (§5.4), deduplicated: the legacy
  // shapes_graph_iri, every dataset-level binding, and the bindings inherited
  // from each graph the dataset mounts — so a form picks up shapes attached to
  // a graph without any per-dataset wiring.
  "shapes": [
    { "graph_iri": "urn:shapes:…",
      "target_classes": ["https://example.org/ont/Book", …],
      "turtle":  "@prefix sh: <…> .\n…",
      "shaclc":  "shape <…> { … }"      // best-effort SHACL Compact Syntax
    }
  ],
  "target_classes": [ /* union across all attached shapes */ ],
  "data_graphs":    [ /* the dataset's named graphs */ ],
  "endpoints": {
    "sparql":        "<base>/sparql",
    "graph_store":   "<base>/store",
    "shapes":        "<base>/api/datasets/{id}/shapes",
    "manifest":      "<base>/api/datasets/{id}/form-manifest"
  },
  "access": {
    "public": true|false,
    "auth_required": true|false,
    "share_link_api": "<base>/api/datasets/{id}/share-links"
  }
}
```

The form platform renders that into a form using its own
SHACL→field mapping (`FriendlyModel`-style: `sh:datatype`/`sh:class` → widget
kind; `sh:minCount`/`sh:maxCount` → required + cardinality; `sh:in` → enum;
`sh:pattern` → mask; `sh:message` → per-field error). The triplestore stops at
publishing; this keeps the contract small and stable. On submit, the form platform
writes via the standard SPARQL Update / Graph Store endpoints — the same
write-gating pipelines apply, so a form submission is rejected exactly when
any other write would be.

### 5.6 Meta-validation (SHACL-SHACL)

Shapes are data, so shapes can be validated too. The Studio seeds a built-in,
read-only shape graph — **SHACL-SHACL** — in the system graph
`urn:system:shapes:shacl-shacl`, and validates *your* shape graphs as data against
it. This catches the usual authoring mistakes: a `sh:property` with no
`sh:path`, a `sh:datatype` that isn't an IRI, a `sh:minCount` that isn't an
integer, a node shape that targets nothing.

Two ways to run it:

- **Ad-hoc** — `POST /api/shacl/shape-graphs/{id}/validate` runs SHACL-SHACL over
  one shape graph and returns a `ValidationReport`. The Library/editor surfaces it
  as a one-click **Validate** button; nothing is recorded.
- **Standardised** — give a pipeline a **shape-graph target**. The pipeline loads
  that set's graph as *data* and the built-in SHACL-SHACL graph as the *shapes*,
  and records each run like any other (so meta-validation appears in the Results
  timeline, badged `meta`). Persist an `ots:validatedBy` binding whose subject is
  a shape graph when a gate should *enforce* meta-validity.

Meta-validation needs no special engine: it is ordinary SHACL where the data
happens to be shapes, so the same severity thresholds, reports and gating apply.

---

## 6. Layer 3 — Instance Data

**Goal:** describe real things using Layer 1's terms. This is what you query with SPARQL.

### 6.1 Typing and conformance

- Every instance **MUST** be typed with a model class: `ex:Dracula a ex:Book`.
- The dataset holding instances **SHOULD** declare `dct:conformsTo` the model version it targets (set via `conforms_to_ontology` + `conforms_to_version`); the catalogue emits this automatically (§7).

```turtle
@prefix ex:   <https://example.org/showcase/> .
@prefix skos: <http://www.w3.org/2004/02/skos/core#> .
@prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .
@prefix geo:  <http://www.opengis.net/ont/geosparql#> .

ex:Dracula a ex:Book ;
    skos:prefLabel "Dracula"@en , "Dracula"@nl ;
    ex:isbn "978-0-486-41109-3" ;
    ex:pageCount "418"^^xsd:integer ;
    ex:publicationYear "1897"^^xsd:integer ;
    ex:isOpenAccess true ;
    ex:rating 4 ;
    ex:publishedBy ex:ExampleOrg ;
    ex:author ex:BramStoker ;
    geo:hasGeometry [ geo:asWKT "POINT(-0.1276 51.5074)"^^geo:wktLiteral ] .
```

### 6.2 Literals and datatypes

- Type every literal: dates as `xsd:date` / `xsd:dateTime`, numbers as `xsd:integer` / `xsd:decimal` / `xsd:double`, booleans as `xsd:boolean`.
- Human-readable text labels **MUST** carry a language tag (`"..."@nl`), making them `rdf:langString`.
- Do not put an IRI's worth of meaning in a string: `dct:creator` takes an **IRI** of an agent, not a name string; `dcat:accessURL` takes an **IRI**.

### 6.3 Geometry (GeoSPARQL)

Spatial features use the standard blank-node geometry shape: `geo:hasGeometry [ geo:asWKT "..."^^geo:wktLiteral ]`. WKT uses `POINT(lon lat)` order. This is the shape the bundled demo and geo queries expect.

### 6.4 Separation in TriG

When shipping instances alongside their catalogue/provenance, keep them in separate named graphs (`ex:instances`, `ex:catalog`, `ex:provenance`) — never one flat graph. See §12 for the full example.

---

## 7. Dataset, catalogue and organisation metadata (DCAT / VoID / ADMS / ORG)

This is how we describe **a dataset, an organisation, or a service** in linked data — the metadata *about* the data. The triplestore generates a full **W3C DCAT 2** catalogue (with embedded **VoID** statistics, **ADMS** status, **ORG/FOAF** publishers, and a **SPARQL service description**) automatically from the dataset registry. You author the metadata as dataset fields; the store renders the RDF. See [`dcat.md`](dcat.md) for the endpoint reference.

### 7.1 Where the catalogue lives

| Endpoint | Returns |
|---|---|
| `/.well-known/void` | the full catalogue, content-negotiated (Turtle, JSON-LD, N-Triples, RDF/XML; `?format=` override) |
| `{base}/{org-slug}/catalog` | a catalogue scoped to one organisation |

Statistics are computed live via SPARQL `COUNT` at request time.

### 7.2 Describing a dataset

A registered dataset is emitted as both `dcat:Dataset` and `void:Dataset`. Author these fields (API: `POST/PUT /api/datasets`); the store maps them to RDF:

```turtle
@prefix dcat: <http://www.w3.org/ns/dcat#> .
@prefix dct:  <http://purl.org/dc/terms/> .
@prefix void: <http://rdfs.org/ns/void#> .
@prefix adms: <http://www.w3.org/ns/adms#> .
@prefix vcard:<http://www.w3.org/2006/vcard/ns#> .
@prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .

<https://triplestore.example.com/dataset/abc123>
    a dcat:Dataset , void:Dataset ;
    dct:title "Library Catalogue 2025" ;
    dct:description "Records in the public library catalogue." ;
    dct:issued  "2025-01-15T10:00:00"^^xsd:dateTime ;
    dct:modified "2025-03-20T14:30:00"^^xsd:dateTime ;
    dct:accessRights <http://publications.europa.eu/resource/authority/access-right/PUBLIC> ;
    dct:publisher <https://triplestore.example.com/org/example-org> ;
    dct:license <https://creativecommons.org/licenses/by/4.0/> ;
    dct:conformsTo <https://triplestore.example.com/data-model/publication-model/version/2.1.0> ;
    dcat:theme <http://publications.europa.eu/resource/authority/data-theme/EDUC> ;
    dcat:keyword "books"@en , "boeken"@nl ;
    dct:spatial <https://sws.geonames.org/2750405/> ;
    adms:status <http://purl.org/adms/status/UnderDevelopment> ;
    adms:versionNotes "Added 2025 Q1 acquisitions." ;
    dcat:contactPoint [
        a vcard:Organization ;
        vcard:fn "Data Office" ;
        vcard:hasEmail <mailto:data@example.com>
    ] ;
    void:triples 84200 ;
    void:subset <urn:catalogue:2025> ;
    dcat:distribution [ a dcat:Distribution ; dcat:accessURL <https://triplestore.example.com/sparql> ; dct:title "SPARQL Endpoint" ] ;
    dcat:distribution [ a dcat:Distribution ; dcat:accessURL <https://triplestore.example.com/store> ; dct:title "Graph Store HTTP Protocol" ] ;
    dcat:landingPage <https://triplestore.example.com/> .
```

The full field-to-RDF mapping is in [Appendix C](#appendix-c--dataset-metadata--rdf-field-mapping).

### 7.3 Access rights (visibility mapping)

Dataset visibility maps to the EU Publications Office authority codes. **MUST** be used as-is:

| Visibility | `dct:accessRights` |
|---|---|
| `public` | `…/access-right/PUBLIC` |
| `members` | `…/access-right/RESTRICTED` |
| `private` | `…/access-right/NON_PUBLIC` |

(`…` = `http://publications.europa.eu/resource/authority`.)

### 7.4 VoID statistics

The aggregate dataset (`{base}/dataset`) and each per-dataset entry carry VoID stats:

| Property | Meaning |
|---|---|
| `void:triples` | total triples (whole store, or scoped to the dataset's graphs) |
| `void:distinctSubjects` / `void:distinctObjects` | distinct subjects / objects |
| `void:properties` | distinct predicates |
| `void:documents` | named-graph count |
| `void:uriSpace` | `{base}/resource/` |
| `void:sparqlEndpoint` | `{base}/sparql` |
| `void:subset` | each registered graph of the dataset (system graphs excluded) |

### 7.5 Graph roles in the catalogue

Each registered graph is annotated with its role via the project's own predicate:

```turtle
<urn:catalogue:2025>
    <https://opentriplestore.org/ontology/graphRole>
        <https://opentriplestore.org/ontology/Instances> .
```

The `ots:` namespace `https://opentriplestore.org/ontology/` defines `graphRole` and the role individuals `Instances`, `Model`, `Vocabulary`, `Shapes`, `Entailment`, `System` — the RDF form of §2.1.

### 7.6 Describing an organisation

Organisations are modelled with **ORG + FOAF**, with a vCard contact point. The RDF type pairs `foaf:Organization` with the W3C ORG specialisation:

| `org_type` | Types emitted |
|---|---|
| (default) | `foaf:Organization , org:FormalOrganization` |
| `OrganizationalUnit` | `foaf:Organization , org:OrganizationalUnit` |
| `Organization` | `foaf:Organization` |

```turtle
@prefix foaf: <http://xmlns.com/foaf/0.1/> .
@prefix org:  <http://www.w3.org/ns/org#> .
@prefix dct:  <http://purl.org/dc/terms/> .

<https://triplestore.example.com/org/example-org>
    a foaf:Organization , org:FormalOrganization ;
    foaf:name "Example Organization" ;
    dct:description "An organization that publishes and maintains open linked-data datasets." ;
    foaf:homepage <https://example.org/> ;
    dct:identifier "ORG" .
```

Model internal structure with `org:hasUnit` / `org:OrganizationalUnit`, and people with `foaf:Person` + `org:memberOf` (usually in the provenance/organisation graph, §9).

### 7.7 ADMS status

Lifecycle status of a dataset/asset uses the ADMS status scheme (`http://purl.org/adms/status/…`): `UnderDevelopment`, `Completed`, `Deprecated`, `Withdrawn`. This is distinct from the *version* lifecycle of a model (§8), which governs Draft→Published.

### 7.8 SPARQL service description

The endpoint is self-describing via `sd:`:

```turtle
@prefix sd: <http://www.w3.org/ns/sparql-service-description#> .
<https://triplestore.example.com/sparql>
    a sd:Service ;
    sd:endpoint <https://triplestore.example.com/sparql> ;
    sd:supportedLanguage sd:SPARQL11Query , sd:SPARQL11Update .
```

---

## 8. Versioning and lifecycle

Models and vocabularies are **versioned artefacts**, not mutable files. Full rationale in [`versioning.md`](versioning.md).

### 8.1 Version states

Each version moves through a lifecycle:

```
Draft  ──stage──▶  Staged  ──publish──▶  Published  ──deprecate──▶  Deprecated
```

- **Draft** — editable work in progress.
- **Staged** — frozen for review.
- **Published** — immutable, citable; consumers `dct:conformsTo` this.
- **Deprecated** — superseded; still resolvable.

Versions are **semantic** (`MAJOR.MINOR.PATCH`): MAJOR = breaking change to the model's contract, MINOR = backward-compatible additions, PATCH = fixes/clarifications.

### 8.2 Branches, rebase, per-subgraph status

The registry supports git-like flows: a version may belong to a named **branch** (default line is "main"); a branch can be **rebased** onto a newer base; and within one version, individual **subgraphs** can have their own status (e.g. publish the `shapes` subgraph while `concepts` stays draft). Use **diff** (`from`/`to`) to review changes before publishing.

### 8.3 Conforming to a version

Instance datasets **SHOULD** pin to an explicit published version, never a floating "latest":

```turtle
<…/dataset/abc123> dct:conformsTo <…/data-model/publication-model/version/2.1.0> .
```

### 8.4 Deprecation pattern

When retiring a concept, **MUST** mark it three ways so every layer sees it:

```turtle
ex:OldBookType
    owl:deprecated true ;
    adms:status <http://purl.org/adms/status/Deprecated> ;
    skos:historyNote "Merged into ex:Book in v2.0.0."@en .
```

Do not delete published terms; deprecate them so existing references keep resolving.

---

## 9. Provenance (PROV-O)

Track who/what/when so data is auditable. Keep provenance in its own named graph.

```turtle
@prefix prov: <http://www.w3.org/ns/prov#> .
@prefix foaf: <http://xmlns.com/foaf/0.1/> .
@prefix org:  <http://www.w3.org/ns/org#> .
@prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .

ex:Dracula
    prov:wasAttributedTo <https://example.org/agent/JanDeVries> ;
    prov:generatedAtTime "2024-01-15T10:30:00Z"^^xsd:dateTime .

<https://example.org/agent/JanDeVries> a prov:Agent , foaf:Person ;
    foaf:name "Jan de Vries"@nl ;
    prov:actedOnBehalfOf ex:ExampleOrg .
```

Use `prov:Entity` / `prov:Activity` / `prov:Agent`, and `prov:wasGeneratedBy`, `prov:used`, `prov:wasDerivedFrom`, `prov:wasAttributedTo`, `prov:actedOnBehalfOf`. The store also records commit-level provenance (message + metadata) on every model/vocabulary patch.

---

## 10. Domain conventions (optional)

On top of the generic standard above, classification and domain modelling — for example building information (BIM), product catalogues, or any controlled vocabulary — benefit from these **opinionated defaults**. They are conventions, not requirements: strong defaults you can adopt where they fit.

### 10.1 Bilingual labels (NL + EN)

Every concept, class and property **SHOULD** carry `skos:prefLabel` / `rdfs:label` in **English (`@en`)** plus your primary working language, exactly one per language. English is required for vocabulary discovery (LD vocabularies are predominantly English); a local-language label keeps the data usable for its primary audience. (This project's own UI and examples ship English + Dutch (`@nl`).)

### 10.2 Reuse established vocabularies

Anchor your model on established, well-maintained vocabularies rather than inventing top-level classes:

| Use | Vocabulary |
|---|---|
| General-purpose types & properties | **schema.org**, Dublin Core (`dct`) |
| Concepts & taxonomies | **SKOS** (+ SKOS-XL for reified labels) |
| People & organisations | **FOAF**, **ORG**, vCard |
| Quantities & units | **QUDT** / **OM** (§10.4) |
| Geometry & place | **GeoSPARQL** (§6.3) |
| Domain standards | reuse the standard for your field — e.g. **BIBFRAME** for bibliographic data, **GS1** for products, schema.org extensions for the web |

Map your concepts onto them with `rdfs:subClassOf` / `skos:broadMatch`.

### 10.3 Notations and identifiers

Every classification concept **MUST** have a `skos:notation` (its short code) and **SHOULD** have a stable `dct:identifier`. SHACL **SHOULD** enforce the notation pattern (e.g. `sh:pattern "^BOOK"`).

### 10.4 Units

Quantities **MUST** reference **QUDT** (`unit:`) or **OM** units rather than free-text. Geometry uses **GeoSPARQL** WKT (§6.3).

### 10.5 The worked example

A small publication vocabulary (`Book`, `Ebook`, `Audiobook`, `Periodical`) is the canonical worked example used throughout this guide. New examples in docs, prompts and training data **SHOULD** reuse that domain so everything stays consistent.

---

## 11. Semantic validity rules — the do/don't checklist

These are the rules a semantic validator (~38 checks) and the data-correction tooling enforce. Generators (human or model) **MUST** follow them.

**SKOS labels**
- Exactly one `skos:prefLabel` per language. Never the same string as both `prefLabel` and `altLabel`.
- Synonyms → `skos:altLabel`. Search-only spellings → `skos:hiddenLabel`.
- SKOS-XL reified labels (`skosxl:prefLabel`) **MUST** point to a `skosxl:Label` with `skosxl:literalForm`.

**SKOS hierarchy**
- A concept is never `skos:broader` of itself; no broader/narrower cycles.
- Pair `skos:broader`/`skos:narrower` consistently.
- Don't use `skos:related` between hierarchically related concepts.

**Dual typing**
- `a skos:Concept , owl:Class` is correct for classifiable concepts.
- A resource **MUST NOT** be both `skos:Concept` and `skos:ConceptScheme`.

**OWL / RDFS**
- Respect declared `rdfs:domain`/`rdfs:range`. Don't put a literal where an IRI is expected.
- Don't assert `owl:disjointWith` between a class and its own subclass.

**IRIs & literals**
- Named nodes for concepts; blank nodes only for anonymous values.
- `dct:creator`/`dct:publisher` → IRI of an agent, not a string. `dcat:accessURL` → IRI.
- Datatype every literal; language-tag every human label.

**Deprecation**
- Use the three-way pattern in §8.4. Never silently delete a published term.

**Named graphs**
- One role per graph (§2). Keep concepts, shapes, instances, catalogue, provenance and linksets separate.

---

## 12. Worked end-to-end example

A complete, role-separated TriG document tying every layer together. This is the gold-standard shape for a self-contained linked-data deliverable.

```trig
@prefix ex:    <https://example.org/showcase/> .
@prefix skos:  <http://www.w3.org/2004/02/skos/core#> .
@prefix rdfs:  <http://www.w3.org/2000/01/rdf-schema#> .
@prefix owl:   <http://www.w3.org/2002/07/owl#> .
@prefix sh:    <http://www.w3.org/ns/shacl#> .
@prefix rdf:   <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix xsd:   <http://www.w3.org/2001/XMLSchema#> .
@prefix dcat:  <http://www.w3.org/ns/dcat#> .
@prefix dct:   <http://purl.org/dc/terms/> .
@prefix prov:  <http://www.w3.org/ns/prov#> .
@prefix org:   <http://www.w3.org/ns/org#> .
@prefix foaf:  <http://xmlns.com/foaf/0.1/> .

# ── Layer 1: Knowledge Model (role: vocabulary) ──────────────────────────────
ex:vocabulary {
  ex:PublicationVocabulary a skos:ConceptScheme ;
      skos:prefLabel "Publication Vocabulary"@en , "Publicatievocabulaire"@nl ;
      owl:versionInfo "2.1.0" ;
      skos:hasTopConcept ex:Publication .

  ex:Publication a skos:Concept , owl:Class ;
      skos:prefLabel "Publication"@en , "Publicatie"@nl ;
      skos:notation "PUB" ; skos:inScheme ex:PublicationVocabulary ;
      skos:topConceptOf ex:PublicationVocabulary ;
      skos:narrower ex:Book .

  ex:Book a skos:Concept , owl:Class ;
      skos:prefLabel "Book"@en , "Boek"@nl ;
      skos:definition "A written work published as a bound or digital volume."@en ;
      skos:notation "BOOK" ; skos:inScheme ex:PublicationVocabulary ;
      skos:broader ex:Publication ; rdfs:subClassOf ex:Publication .

  ex:pageCount a owl:DatatypeProperty ;
      rdfs:label "Page count"@en , "Aantal pagina's"@nl ;
      rdfs:domain ex:Book ; rdfs:range xsd:integer .

  ex:publishedBy a owl:ObjectProperty ;
      rdfs:label "Published by"@en , "Uitgegeven door"@nl ;
      rdfs:domain ex:Publication ; rdfs:range org:Organization .
}

# ── Layer 2: Information Model (role: shapes) ────────────────────────────────
ex:shapes {
  ex:BookShape a sh:NodeShape ;
      sh:targetClass ex:Book ;
      sh:property [ sh:path skos:prefLabel ; sh:datatype rdf:langString ;
                    sh:minCount 1 ; sh:uniqueLang true ] ;
      sh:property [ sh:path skos:notation ; sh:pattern "^BOOK" ;
                    sh:minCount 1 ; sh:maxCount 1 ] ;
      sh:property [ sh:path ex:pageCount ; sh:datatype xsd:integer ;
                    sh:minInclusive 1 ; sh:maxInclusive 5000 ] .
}

# ── Layer 3: Instance Data (role: instances) ─────────────────────────────────
ex:instances {
  ex:Dracula a ex:Book ;
      skos:prefLabel "Dracula"@en , "Dracula"@nl ;
      ex:pageCount "418"^^xsd:integer ;
      ex:publishedBy ex:ExampleOrg .
}

# ── Catalogue (DCAT) ─────────────────────────────────────────────────────────
ex:catalog {
  ex:CatalogueDataset a dcat:Dataset ;
      dct:title "Library Catalogue"@en , "Bibliotheekcatalogus"@nl ;
      dct:publisher ex:ExampleOrg ;
      dct:conformsTo ex:PublicationVocabulary ;
      dcat:theme ex:Book ;
      dcat:distribution [ a dcat:Distribution ; dcat:mediaType "text/turtle" ;
                          dcat:accessURL <https://example.org/data/catalogue.ttl> ] .
}

# ── Provenance + Organisation (PROV-O / ORG) ─────────────────────────────────
ex:provenance {
  ex:Dracula prov:wasAttributedTo <https://example.org/agent/JanDeVries> ;
      prov:generatedAtTime "2024-01-15T10:30:00Z"^^xsd:dateTime .

  <https://example.org/agent/JanDeVries> a prov:Agent , foaf:Person ;
      foaf:name "Jan de Vries"@nl ; prov:actedOnBehalfOf ex:ExampleOrg .

  ex:ExampleOrg a foaf:Organization , org:FormalOrganization ;
      foaf:name "Example Organization" .
}
```

Three checks tell you which layer a triple belongs in:

1. **"What *is* a Book?"** → Knowledge Model (`skos:Concept , owl:Class`, labels, notation, broader).
2. **"What makes a Book *valid*?"** → Information Model (`sh:NodeShape`, cardinality, patterns, ranges).
3. **"*Which* Book?"** → Instance Data (Dracula, 418 pages, published by the Example Organization).

---

## Appendix A — Namespace & prefix registry

The exact prefixes the triplestore emits. Use these spellings.

| Prefix | Namespace | Purpose |
|---|---|---|
| `rdf` | `http://www.w3.org/1999/02/22-rdf-syntax-ns#` | core RDF |
| `rdfs` | `http://www.w3.org/2000/01/rdf-schema#` | classes, properties, labels |
| `owl` | `http://www.w3.org/2002/07/owl#` | formal ontology semantics |
| `skos` | `http://www.w3.org/2004/02/skos/core#` | concept schemes, labels |
| `skosxl` | `http://www.w3.org/2008/05/skos-xl#` | reified labels |
| `sh` | `http://www.w3.org/ns/shacl#` | validation shapes |
| `dct` | `http://purl.org/dc/terms/` | Dublin Core metadata |
| `dcat` | `http://www.w3.org/ns/dcat#` | catalogue |
| `void` | `http://rdfs.org/ns/void#` | dataset statistics |
| `adms` | `http://www.w3.org/ns/adms#` | asset status / versioning |
| `prov` | `http://www.w3.org/ns/prov#` | provenance |
| `org` | `http://www.w3.org/ns/org#` | organisations |
| `foaf` | `http://xmlns.com/foaf/0.1/` | agents, people |
| `vcard` | `http://www.w3.org/2006/vcard/ns#` | contact points |
| `vann` | `http://purl.org/vocab/vann/` | preferred prefix/namespace |
| `schema` | `http://schema.org/` | general-purpose enrichment |
| `sd` | `http://www.w3.org/ns/sparql-service-description#` | SPARQL service description |
| `geo` | `http://www.opengis.net/ont/geosparql#` | geometry |
| `qudt` | `http://qudt.org/schema/qudt/` | quantities |
| `unit` | `http://qudt.org/vocab/unit/` | units |
| `om` | `http://www.ontology-of-units-of-measure.org/resource/om-2/` | units (alt) |
| `xsd` | `http://www.w3.org/2001/XMLSchema#` | datatypes |
| `ots` | `https://opentriplestore.org/ontology/` | graph-role annotations; validation-layer bindings (`ots:validatedBy`, §5.4) |

## Appendix B — Graph-role detection cheat-sheet

| If a graph is dominated by… | It is detected as | Register it as |
|---|---|---|
| `owl:Class` / `owl:*Property` / `rdfs:Class` | `model` | data model |
| `skos:Concept` / `skos:ConceptScheme` / `skos:*` predicates | `vocabulary` | vocabulary |
| `sh:NodeShape` / `sh:*` predicates / `sh:targetClass` | `shapes` | shapes (in registry) |
| `swrl:Imp` / `spin:`/`sp:` | `entailment` | entailment |
| subjects typed with non-schema classes | `instances` | dataset |
| a balanced mix | *ambiguous* → set `?kind=` | (your call) |

Override values: `model`/`tbox`, `vocabulary`/`vocab`, `shapes`, `entailment`, `instances`/`abox`.

## Appendix C — Dataset metadata → RDF field mapping

How dataset registry fields render in the DCAT catalogue ([`src/dcat/catalog.rs`](../src/dcat/catalog.rs)).

| Dataset field | RDF |
|---|---|
| `name` | `dct:title` |
| `description` | `dct:description` |
| `created_at` / `updated_at` | `dct:issued` / `dct:modified` (`xsd:dateTime`) |
| `visibility` | `dct:accessRights` (EU authority URI — §7.3) |
| owner (org) | `dct:publisher → {base}/org/{id}` |
| owner (user) | `dct:creator → {base}/user/{id}` |
| `license` | `dct:license` (IRI) |
| `themes` | `dcat:theme` (one per IRI) |
| `keywords` | `dcat:keyword` (`@en`) |
| `contact_name`/`email`/`url` | `dcat:contactPoint` → `vcard:Organization` (`vcard:fn`, `vcard:hasEmail`, `vcard:hasURL`) |
| `adms_status` | `adms:status` (IRI) |
| `version_notes` | `adms:versionNotes` |
| `spatial` | `dct:spatial` (IRI) |
| `landing_page` | `dcat:landingPage` |
| `shapes_graph_iri` (when `shacl_on_write`) | `dct:conformsTo` |
| `conforms_to_ontology` + `conforms_to_version` | `dct:conformsTo → …/version/{semver}` |
| registered graphs | `void:subset` + per-graph `ots:graphRole` |
| live counts | `void:triples`, `void:distinctSubjects`, `void:distinctObjects`, `void:properties`, `void:documents` |

## Appendix D — Conformance checklist

A deliverable conforms to this styleguide when:

- [ ] Each named graph holds exactly one role (model / vocabulary / shapes / instances / entailment).
- [ ] Concepts are dual-typed `skos:Concept , owl:Class`, in a `skos:ConceptScheme`.
- [ ] Every concept/class/property has `skos:prefLabel`/`rdfs:label` in **NL and EN**, one per language.
- [ ] Every concept has a `skos:notation`; properties declare `rdfs:domain`/`rdfs:range`.
- [ ] SHACL shapes live in their own graph and bind via `sh:targetClass`.
- [ ] Instances are typed with model classes; the dataset declares `dct:conformsTo` a **published version**.
- [ ] Literals are datatyped; labels are language-tagged; `dct:creator`/`accessURL` are IRIs.
- [ ] Dataset metadata (licence, access rights, publisher, themes, contact, ADMS status) is set so the catalogue is complete.
- [ ] Deprecated terms use the three-way pattern (`owl:deprecated` + `adms:status` + `skos:historyNote`).
- [ ] IRIs are stable, `http(s)`, space-free; blank nodes only for anonymous values.

## Appendix E — Where this standard is enforced

| Component | How it applies this standard |
|---|---|
| **Open Triplestore** | implements graph roles, auto-classification, DCAT/VoID/ADMS/ORG generation, SHACL validation, version lifecycle |
| **Graph viewer** | can teach and enforce it (e.g. a semantic validator) and prompt LLM agents with it |
| **Form / validation tools** | generate forms and corrections that satisfy these shapes |
| **LLM assistants** | system prompts and training data are written to produce output conforming to this guide |

When you change this document, update any dependents that embed or teach these conventions (companion tools, agent prompts, training data).
