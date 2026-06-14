# Model & Vocabulary Versioning

The **Model Registry** (also called the **Model & Vocabulary Registry**) provides version-controlled storage for the schema layer in one place: **models** (the T-Box — classes and class axioms), **vocabularies** (the R-Box — properties, relations and SKOS concept schemes), and the **shapes** and **entailment** roles that cut across them. Each entry carries a **kind** (`data-model` or `vocabulary`), auto-detected from the uploaded RDF — the detector compares the class count against the combined property + SKOS count and routes to the dominant side — and shown as a badge (with a model/vocabulary filter) in the web UI. Versions follow a **draft → staged → published → deprecated** lifecycle. Users with the **publish permission** (or admins / super-admins) can upload and publish versions; anyone with read access can browse and download.

## Bundled standard vocabularies

A fresh instance seeds the registry with the common W3C/OGC standards, each stamped with its **real, multiple versions** out of the box rather than a single placeholder — for example OWL `1.0.0` / `2.0.0` / `2.1.0`, RDF `1.0` / `1.1` / `1.2` (draft), DCAT `1` / `2` / `3`, GeoSPARQL `1.0` / `1.1`, and DCMI Terms dated `2008` / `2012` / `2020`. Draft releases (e.g. RDF 1.2, SHACL 1.2, OWL-Time 1.1) are seeded with status **Draft** and are never returned as the "latest" published version.

> **OWL profiles are not versions.** EL, QL, RL, DL and Full are **profiles** of the single OWL 2 language, not separate versions of OWL — they describe the reasoning fragment, not a release line. See the per-profile docs ([OWL 2 DL](/docs/owl2-dl), [EL](/docs/owl2-el), [QL](/docs/owl2-ql), [RL](/docs/owl2-rl)).

## Lifecycle

1. **Register** — Create a model or vocabulary entry with a title and optional namespace URI. This records it in the registry but uploads no triples.
2. **Upload a version** — Upload an RDF file in any supported format. The system assigns a version identifier and stores the triples in a named graph. The version starts as *draft*.
3. **Inspect & compare** — Use the Diff Viewer to compare any two versions. Added triples are highlighted green, removed triples red, modified triples amber.
4. **Stage** — Promote a draft to *staged* for review before it goes live. Staging is optional but lets reviewers see a candidate without it becoming the canonical latest.
5. **Publish** — Mark a version *published*. The published version becomes the canonical latest version and is served at `/api/models/{id}/latest/data`. On publish, version metadata is stamped into the graph by content: OWL `owl:versionIRI`/`owl:priorVersion` for class/property models and DCAT/PAV/SKOS metadata for vocabularies (both for mixed packages). A version still in **Draft** status is never served as "latest". Published versions are immutable.
6. **Deprecate** — Older published versions can be deprecated to signal that consumers should upgrade.

## Storage

- **Version graph IRI** — `{base-url}/data-model/{id}/version/{version}`. Models and vocabularies share this scheme; the entry's `kind` distinguishes them.
- **Latest published endpoint** — `/api/models/{id}/latest/data` — content-negotiated RDF.
- **Dereferenceable terms** — Individual terms resolve to a description via `/api/models/{id}/term` (for a SKOS concept this also pulls in the enclosing `skos:ConceptScheme`), and any stored IRI is content-negotiable at `/resource/<path>` (scoped to graphs the caller can read).

## Branches, merging & subgraphs

- **Branches** — Fork a version line to develop changes in parallel. List or create branches at `/api/models/{id}/branches`, naming the branch and the version it forks from.
- **Merge preview** — Before merging two version lines, preview the triple-level diff at `/api/models/{id}/merge/preview?from=<v>&into=<v>` to see exactly what a merge would add or remove.
- **Subgraphs** — A single version can be split into named subgraphs that are staged, published, or deprecated independently — useful when one model bundles several modules with different release cadences.

See also: [Linked Data Modelling](/docs/modelling), [Data Modeling Architecture](/docs/data-modeling), and [Named Graphs](/docs/named-graphs).
