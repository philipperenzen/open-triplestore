# Dataset Governance & SHACL Model (Admin)

> **Audience:** administrators and super-admins only. This page is hidden from
> regular users and is served only to admin/super_admin accounts (it returns
> `404` to everyone else).

This page documents the built-in **governance layer** that ships with every
Open Triplestore instance and is seeded automatically on a fresh install.

## 1. The dataset-structure SHACL model

Every dataset has a metadata graph at `urn:system:metadata:dataset:{id}` — a
`dcat:Dataset` description (DCAT 2 / VoID / ADMS). The built-in shapes graph
`urn:system:shapes:dataset-structure` (visible in **SHACL Studio → Shapes
Library** as *"Dataset structure (governance)"*) asserts the contract a dataset
must satisfy:

| Constraint | Severity | Meaning |
|------------|----------|---------|
| `rdf:type dcat:Dataset` | target | The metadata node is a dataset. |
| `dct:title` (minCount 1, string) | **Violation** | A dataset must be titled. |
| `dct:identifier` (minCount 1, string) | **Violation** | A dataset must have a stable id. |
| `ots:visibility` (minCount 1, string) | **Violation** | A dataset must declare its visibility. |

The required identity fields (`title`, `identifier`, `visibility`) are always
emitted by `build_dataset_metadata_ttl`, so well-formed datasets always conform.
Per-graph roles (`void:subset` → `ots:graphRole`) are emitted into the metadata
and validated by the standards/ots shapes rather than enforced here, so creating
an empty dataset or adding a not-yet-classified graph is never blocked.

## 2. Enforcement & the startup audit

- **Write-time:** `write_dataset_metadata_graph_checked` validates a dataset's
  metadata against the model before writing it and rejects non-conforming
  metadata (HTTP 422 + report). The best-effort `write_dataset_metadata_graph`
  remains for trusted seed/import paths.
- **Startup audit:** `audit_dataset_metadata` runs on boot. For every dataset it
  validates the stored metadata graph and, if non-conforming, **repairs** it by
  regenerating the metadata from the current record (which now emits the required
  identifier + visibility). Anything still non-conforming is flagged in the
  `urn:system:audit` graph with `ots:auditStatus "nonconforming"` — datasets are
  **never deleted**.

## 3. Standards shapes & pipelines

A SHACL shape graph (`urn:system:shapes:std-{key}`) and a validation pipeline
(`urn:system:pipeline:std-{key}`) are seeded for each supported standard
(RDF, RDFS, OWL 2 QL/EL/RL/DL, GeoSPARQL, SHACL Core/Advanced, ShEx, SWRL, LDP,
DCAT/VoID, plus the capabilities registry covering the protocol/auth standards).
Each pipeline targets that standard's bundled demo graph, so a fresh instance can
demonstrate validation end-to-end from **SHACL Studio → Pipelines**. The
SHACL-of-SHACL meta-shapes (`urn:system:shapes:shacl-shacl`) validate the shape
graphs themselves.

## 4. The Open Triplestore model & vocabulary

The bundled **Open Triplestore** org publishes its own model and vocabulary in
the *"Open Triplestore Model & Vocabulary"* dataset:

- **Model** (`…/ots-model/ots-model`, role `Model`, the T-Box): OWL/RDFS **class**
  definitions — `ots:Standard`, `ots:AuthMethod`, and the six `GraphRole` classes.
- **Vocabulary** (`…/ots-model/ots-vocabulary`, role `Vocabulary`, the R-Box): the
  **property/relation** terms (`ots:conformance`, the `ots:graphRole` relation,
  `ots:visibility`) and a SKOS concept scheme for graph roles, conformance levels
  and the supported standards. The `ots:graphRole` property lives here, in the
  Vocabulary layer, even though the `GraphRole` classes it points at are defined
  in the Model.

The role individuals are minted in the role namespace
`https://opentriplestore.org/ns/role#` (e.g. `…/ns/role#Model`,
`…/ns/role#Vocabulary`, `…/ns/role#Instances`).

## 5. Derived-data write targets

Validation pipelines can persist what a run produces (see **Pipelines → a
pipeline → Derived data**):

- **Inferred triples** (from SHACL-AF inference / functions): keep *in place*,
  copy to a *new named graph* (tagged `Entailment`), or capture in a *new dataset
  version*.
- **Validation results**: serialise the `sh:ValidationReport` to RDF and write to
  a report graph, a chosen graph, or a new version — or keep run-history only.

All of the above is idempotent and re-seeded on every boot; `SEED_STANDARDS_DEMO=false`
disables the demo-org seed (the governance model + standards shapes still seed).
