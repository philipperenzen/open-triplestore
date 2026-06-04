# Model & Vocabulary Versioning

The Model Registry and Vocabulary Registry provide version-controlled storage for data models and SKOS vocabularies. Versions follow a **draft → published** lifecycle. Users with the **publish permission** (or admins / super-admins) can upload and publish versions; anyone with read access can browse and download.

## Lifecycle

1. **Register** — Create a model or vocabulary entry with a title and optional namespace URI. This records it in the registry but uploads no triples.
2. **Upload a version** — Upload an RDF file in any supported format. The system assigns a version identifier and stores the triples in a named graph. The version starts as *draft*.
3. **Inspect & compare** — Use the Diff Viewer to compare any two versions. Added triples are highlighted green, removed triples red, modified triples amber.
4. **Stage** — Promote a draft to *staged* for review before it goes live. Staging is optional but lets reviewers see a candidate without it becoming the canonical latest.
5. **Publish** — Mark a version *published*. The published version becomes the canonical latest version and is served at `/api/data-models/{id}/latest/data` or `/api/vocabularies/{id}/latest/data`. Published versions are immutable.
6. **Deprecate** — Older published versions can be deprecated to signal that consumers should upgrade.

## Storage

- **Model version graph IRI** — `urn:data-model:{id}:v{version}`
- **Vocabulary version graph IRI** — `urn:vocabulary:{id}:v{version}`
- **Latest published endpoint** — `/api/data-models/{id}/latest/data` or `/api/vocabularies/{id}/latest/data` — content-negotiated RDF
- **Dereferenceable terms** — Individual terms resolve to a description via `/api/data-models/{id}/term`, and any stored IRI is content-negotiable at `/resource/<path>` (scoped to graphs the caller can read).

## Branches, merging & subgraphs

- **Branches** — Fork a version line to develop changes in parallel. List or create branches at `/api/data-models/{id}/branches` (and the matching `/api/vocabularies/{id}/branches`), naming the branch and the version it forks from.
- **Merge preview** — Before merging two version lines, preview the triple-level diff at `/api/data-models/{id}/merge/preview?from=<v>&into=<v>` to see exactly what a merge would add or remove.
- **Subgraphs** — A single version can be split into named subgraphs that are staged, published, or deprecated independently — useful when one model bundles several modules with different release cadences.

See also: [Linked Data Modelling](/docs/modelling), [Data Modeling Architecture](/docs/data-modeling), and [Named Graphs](/docs/named-graphs).
