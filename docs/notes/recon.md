# Phase 0 — Repository reconnaissance: GeoSPARQL + SHACL for the Waalbrug brief

> Status: **recon complete; scope confirmed; visualization requirement added.** Date: 2026-06-09.
>
> **Review decisions (locked):** full **R0–R6** scope; **also vendor** the official W3C
> SHACL + OGC GeoSPARQL suites into CI (alongside the Waalbrug oracle); **match repo
> test style** (inline canonicalized-report asserts, no `insta`).
>
> **Added requirement (post-recon):** the geometry/3D linked data must be *visualizable*,
> not just served as JSON — an interactive **3D viewer** (glTF via FOG/OMG) **and** a
> **map view** (a kunstwerk + all its parts on a basemap), where selecting a sub-element
> surfaces that element's linked data. This becomes **R7** (frontend), built on the R6 feed.
>
> Headline: `open-triplestore` is **not greenfield** for this task. It already ships
> GeoSPARQL 1.1 (36 `geof:` functions over GEOS), a native SHACL engine (Core +
> SHACL-SPARQL + part of AF), an R-tree spatial index, SHACL-C, and a large
> SHACL-Studio management layer. The brief was written assuming a from-scratch
> build; the real work is **closing a small set of specific gaps** so the Waalbrug
> dataset, queries and shapes all behave as documented. This note records what
> exists, what's missing, and a revised, much smaller milestone plan.

---

## 1. Workspace shape

Cargo **workspace**, resolver 2, two members:

- **`.` (`open-triplestore`)** — the binary + the whole engine/API. `edition = 2021`,
  `rust-version = 1.88` (MSRV, CI-enforced). `publish = false` (source-available,
  AGPL-3.0 + Commons Clause).
- **`opengraph/`** — a thin layer over Oxigraph providing durable blank-node identity
  (canonical labels + opt-in Skolemization), an MVCC layer, a hash-join + optimizer,
  and the `ParallelMirror` used for multi-core `/sparql`.

Relevant `src/` modules (all are real, populated subsystems):

| Module | Role |
|---|---|
| `src/store/` | `TripleStore` over Oxigraph `Store`; query/update entrypoints, result cache, `ParallelMirror`, raw quad-index helpers, path cache |
| `src/geo/` | **GeoSPARQL**: `datatypes.rs` (WKT parse/serialize + WKB cache), `functions.rs` (36 `geof:` fns), `spatial_index.rs` (R-tree), `vocabulary.rs` |
| `src/shacl/` | **SHACL engine**: `engine.rs` (shape/rule loading, targets, validate+infer), `constraints.rs` (constraint components + SHACL-SPARQL), `shapes.rs` (model), `report.rs`, `advanced.rs` (doc-only stub) |
| `src/shacl_studio/` | ~5.8k LOC management layer: shape-graphs, validation-layer bindings, pipelines, scheduler, RDF report persistence, meta (SHACL-SHACL) |
| `src/shaclc/` | SHACL Compact Syntax parser + serializer |
| `src/sparql/` | service description, RDF-1.2 functions |
| `src/dcat/`, `src/data_models/`, `src/reasoning/`, `src/rml/`, `src/swrl/`, `src/shex/`, `src/ldp/`, `src/text_search/`, `src/auth/`, … | adjacent standards/features |

## 2. RDF foundation

Built on **Oxigraph 0.4** (`oxigraph`, `oxrdf 0.2`, `spargebra 0.3`) — *not* custom and
*not* sophia. Terms/quads are oxrdf. Named-graph/quad support is native; the SHACL
engine and dataset layer use named graphs throughout. Serializations via Oxigraph:
Turtle, TriG, N-Triples, N-Quads, RDF/XML (+ JSON-LD, OWL/XML per the standards tests).
**RDF 1.2 / RDF-star** is present behind the `rdf-12` feature (`oxrdf/rdf-star`), with
SPARQL-1.2 triple-term functions in `src/sparql/rdf12_functions.rs`.

## 3. SPARQL engine + function dispatch (the key finding for SHACL-SPARQL)

Reuses Oxigraph's parser/algebra/evaluator. `spargebra` algebra is inspected directly in
a few fast paths (e.g. COUNT(\*) shortcut, parallel-mirror routing) in `src/store/engine.rs`.

**Custom functions** are registered via Oxigraph's `QueryOptions::with_custom_function`
in `TripleStore::query_options()` (`src/store/engine.rs:~280`). Every execution path —
`query()`, `query_opt()`, `update()`, `update_targeted()`, `batch_update()`, and SWRL —
funnels through `query_options()`, so the `geof:*` functions are registered **uniformly**.

> **Critical consequence:** a SHACL-SPARQL constraint that calls `geof:distance(...)`
> via `store.query(...)` **does** have the function available. There is no separate
> "HTTP-only" function path. This de-risks M4's GeoSPARQL-in-constraints requirement —
> the wiring already exists; the blocker is `sh:prefixes` (below), not dispatch.

`SERVICE` (federation): not in scope here; not verified.

## 4. Indexing

Oxigraph's own term dictionary + triple indexes underneath. On top, `src/geo/spatial_index.rs`
is an **`rstar` R-tree** (bbox prefilter → exact predicate). M1's "build a spatial index"
item is therefore **already done**; what's worth checking is whether the evaluator wires it
in as a prefilter for topological predicates automatically or only on explicit use.

## 5. Existing GeoSPARQL / SHACL (what's already green)

**GeoSPARQL** (`src/geo/`, README claims "all 30 OGC requirements"):
- WKT literal parse/serialize with optional `<crs-uri>` prefix; process-wide WKT→WKB cache.
- 36 `geof:` functions: 8 Simple Features, 8 Egenhofer, 8 RCC8, 8 constructive
  (`boundary/buffer/convexHull/difference/envelope/intersection/symDifference/union`),
  + `distance/area/getSRID/relate`. All via GEOS.
- R-tree spatial index.
- `tests/geosparql_conformance.rs`: **100 tests** mapping OGC Req 1–30 (inline WKT/Turtle).

**SHACL Core** (`src/shacl/constraints.rs`): cardinality, `datatype`, `nodeKind`, `class`,
`in`, `hasValue`, `pattern`(+flags), `minLength/maxLength`, `languageIn`, `uniqueLang`, the
four value-range comparisons, `equals/disjoint/lessThan/lessThanOrEquals`, `closed`,
`not/and/or/xone`, `node`, **`qualifiedValueShape` + min/max** — all present. Targets:
`targetClass/targetNode/targetSubjectsOf/targetObjectsOf` + implicit class target + SPARQL
target. Emits a `ValidationReport` with focus/path/value/severity/sourceConstraint/message;
honours `sh:severity` and `sh:message`. Parallelised with rayon. Recursion-depth-bounded.
RDF-list and **blank-node** attribute dereferencing go through the raw quad index (recent
fixes), so inline `sh:property [ … ]` with **simple predicate paths** does load.

**SHACL-SPARQL** (`Constraint::SparqlConstraint`): `sh:select` with `$this` pre-bound via
`VALUES` + `FROM <data-graph>` injection (`bind_this`), so aggregates/`GROUP BY` work; each
result row → one result; `?value`/`?path` read from rows.

**SHACL-AF** (in `engine.rs`): `sh:SPARQLTarget` (`sh:select ?this`); `sh:rule` as both
**SPARQLRule** (`sh:construct`, run as INSERT, `$this`-bound) and **TripleRule**
(`sh:subject/predicate/object`, `sh:this`→focus); fixpoint inference loop with exact
count-delta convergence.

Tests already green: `shacl_conformance.rs` (9), `shacl_rules_conformance.rs` (12),
`shaclc_conformance.rs` (7), plus `standards_conformance.rs`, `standards_demo_e2e.rs`.

**SHACL-Studio** (`src/shacl_studio/`): shape-graphs as first-class versioned RDF artifacts;
validation-layer bindings (`<target> ots:validatedBy <shape-graph>`); pipelines + cron
scheduler; **persists `sh:ValidationReport` into `urn:system:*` named graphs** (queryable
as RDF — covers §7.4) and JSON run history in SQLite. Endpoints already exist:
`POST /api/datasets/:id/validate`, `GET/PUT /api/datasets/:id/shapes`,
`POST /api/datasets/:id/infer`, `POST /api/shaclc/parse|serialize`, plus the
`/api/shacl/shape-graphs|bindings|pipelines|shapes` Studio surface.

## 6. Tests & CI conventions

- **Tests are inline-string driven** (Turtle/WKT/SHACL-C constants), no external fixtures,
  no git submodules, **no vendored W3C/OGC suites** despite the brief assuming they exist.
  HTTP tests use `tests/common/mod.rs` + `tower::ServiceExt::oneshot` (no port bind).
- **`insta` is NOT a dependency**; no snapshot tests. (The brief's snapshot strategy would
  be net-new — see Open Questions.)
- **No Waalbrug fixture** anywhere yet.
- **CI** (`.github/workflows/ci.yml`): jobs `backend` (fmt-check **hard gate**, `clippy
  --all-targets --all-features`, build, `test --all-features --locked`, security-test
  floor ≥40), `conformance` (`--test '*conformance*'`), `frontend`, `audit` (cargo-deny),
  `secret-scan` (gitleaks). `.gitlab-ci.yml` mirrors it and adds a **perf gate** (`cargo
  bench --features full -- 'query|path|geosparql'` vs baseline). System libs installed:
  libgeos-dev, cmake, pkg-config, libclang, libssl, libxmlsec1, lld. **No feature-matrix
  job** — everything runs `--all-features`.
- **Makefile**: `dev`, `release`, `bench`, `perf-check`, `install-hooks`. Default
  `FEATURES=full`.
- **Errors**: `thiserror` for domain enums (`StoreError`, …) + `anyhow` for context; SHACL
  returns a `ValidationReport`/`Result<usize,String>` rather than an error enum. No
  `unwrap`/`panic` in library paths is the prevailing style.
- **Conventions** (from project memory, not a repo `settings.json` — `.claude/` holds only
  `launch.json`): Conventional Commits; **no `Co-Authored-By: Claude` trailer**; feature
  branches (`feat/…`, `fix/…`, `docs/…`); **validate Docker/Rust changes via the Docker
  builder image** (native cargo build fails on geos/pkg-config); expect working-tree churn
  from concurrent agents → **stage only my own files**.

## 7. Build-vs-reuse decision

**Decision: continue the established hybrid — extend the engine natively (Strategy A),
keep leaning on GEOS for topology and add `proj` for CRS (Strategy B for geometry libs);
treat `oxirs-*`/`rudof` purely as reference/test-vector sources (Strategy C), not deps.**

Rationale: the architecture already *is* "native SHACL + native GeoSPARQL dispatched into
Oxigraph's custom-function table, with GEOS underneath." Swapping in `oxirs-shacl` or
`rudof` would duplicate a working 2.3k-LOC engine, fork the report/Studio integration, and
bloat the dependency/audit surface for no conformance gain. New geometry math stays behind
the existing `src/geo` boundary; `proj` goes in **feature-gated** (`proj-transform`) because
it pulls a system PROJ lib (CI already installs GEOS/PROJ-adjacent libs, but keep min-build
clean). This matches the repo's additive-feature-flag style.

---

## 8. Gap analysis — what the Waalbrug brief needs that is *missing*

Ordered by how hard they block the Definition-of-Done. "Tracked gap" = already
acknowledged in code/tests.

| # | Gap | Evidence | Blocks |
|---|---|---|---|
| **G1** | **`sh:prefixes` / `sh:declare` not injected** into SHACL-SPARQL `sh:select`/`sh:construct`/`sh:target`. Prefixed names (`da:`, `geo:`, `geof:`…) → query parse error → constraint **silently skipped**. | `src/shacl_studio/shacl-shacl.ttl:16-18` explicitly avoids `sh:sparql` for this reason; no `sh:declare` handling in `engine.rs`/`constraints.rs`. | **All** of Waalbrug §5, §6.2-6.3 (every SPARQL constraint/rule/target uses `sh:prefixes ex:prefixes`). |
| **G2** | **Complex property-path parsing.** Loader only ever builds `PropertyPath::Predicate`; a blank-node path (sequence/inverse/`sh:alternativePath`) is read as `Predicate("_:bn")` → wrong. (The enum + `to_sparql` already support all path kinds — only the *RDF→enum loader* is missing.) | `engine.rs:638-645` `single_value(sh:path)`; `shapes.rs:44-76`. | Waalbrug §4.3 `BrugStructuurShape` (`sh:path ( geo:hasGeometry [ sh:alternativePath (…) ] )`). |
| **G3** | **`sh:expression` node expressions** not implemented at all. | no `expression` handling in `src/shacl`. | Waalbrug §6.4 `DoorvaarthoogteEisShape`. (Brief allows the path+comparison subset; the same rule is also expressible via §5 SPARQL — documented fallback.) |
| **G4** | **`sh:SPARQLFunction`** (user-defined function from data, e.g. `ex:afstandMeter`) — registering a data-defined function into Oxigraph's function table. | no `SPARQLFunction`/`sh:parameter`/`sh:returnType` anywhere. | Waalbrug §6.1; DoD item 3 ("`ex:afstandMeter` callable from SPARQL"). |
| **G5** | **`geo:gmlLiteral` not parsed** (WKT-only). | `geo/datatypes.rs:55-77` accepts wktLiteral/string only; `tests/geosparql_conformance.rs:2005` "tracked gap". | Waalbrug §3 `wb:Landhoofd-Noord` (`geo:asGML`) and §4.3 GML branch; M1 round-trip. |
| **G6** | **No CRS transforms** (`proj` absent); `geof:transform` unimplemented; viewer-feed reprojection 28992→4326/3857 impossible. | no `proj` dep; `geosparql_conformance.rs:1866-1878` "tracked gap"; agent confirmed. | M1 transform acceptance; M6 viewer feed reprojected coordinate; DoD item 4. |
| **G7** | **`uom:` ignored** in `geof:distance`/`buffer` — planar in the CRS's own units. | `geo/datatypes.rs:139-152` `parse_uom` → 1.0; `geosparql_conformance.rs:1800-1812` documents planar-degrees nuance. | Spec purity. **Not** a Waalbrug blocker: EPSG:28992 is metres, so `uom:metre` distances are already numerically correct. Fix for geographic CRS only. |
| **G8** | **No Waalbrug fixtures**, no `insta`, no `docs/conformance/` dir, no vendored W3C/OGC suites. | §6 above. | M1-M6 fixtures + DoD item 5 (conformance reports). |
| **G9** | **`$value`/`$PATH` not pre-bound** in property-shape-context SHACL-SPARQL (only read back from result rows). | `constraints.rs:484-501`, `bind_this` only handles `$this`. | Minor; only matters for property-shape `sh:sparql` that *reads* `$value`. None of the Waalbrug constraints need it (they're node-shape constraints). Document or add cheaply. |
| **G10** | **Inline blank-node `sh:qualifiedValueShape` not resolved.** The value shape is looked up by IRI in the top-level shapes list (`constraints.rs:800` `shapes.iter().find(...)`); an inline `[ sh:class … ]` is never a top-level shape, so the qualified constraint is silently skipped. `sh:not`/`and`/`or`/`xone` avoid this by calling `load_inline_shape` at load time — `qualifiedValueShape` only stores the IRI. **Discovered empirically by the R0 oracle**, not from reading. | `engine.rs:544-569` (loads IRI only), `constraints.rs:795-845`. | Waalbrug §4.3 `BrugStructuurShape` qualifiedMinCount. Fix: store the inline shape (like the logical operators do) or fall back to `load_inline_shape` on lookup miss. |

> **R0 empirical result (first run).** 13 cases: **4 pass active**, 9 failed-as-expected →
> 1 was a **fixture bug** (the doc's `ifcGuid` literal is 21 chars, not 22 — the
> `sh:pattern {22}` correctly rejected it; fixed), 8 are genuine gaps now `#[ignore]`d with
> IDs: 5×G1 (§5 SPARQL) + 1×G1 (rule) + 1×G3 (expression) + **1×G10 (new)**. Confirms Core
> datatype/minInclusive/minCount/pattern/uniqueLang all work; isolates the backlog precisely.

**Already satisfied by existing code (no work, just fixtures + a verifying test):**
GeoSPARQL functions incl. `distance`/`sfWithin` (M2), spatial index (M1), SHACL Core incl.
`pattern`/`uniqueLang`/`qualifiedValueShape` (M3), `sh:SPARQLConstraint` select/ask with
`$this` + aggregates (M4 mechanics), `sh:SPARQLTarget` + `sh:SPARQLRule`/`sh:TripleRule`
fixpoint (M5 mechanics), report-as-queryable-RDF + validate/infer endpoints (M6).

---

## 9. Revised milestone plan

The original M1–M6 assumed greenfield. Revised: a **fixtures-first** slice that converts
the brief's pass/fail matrix into the test oracle, then small, independently-mergeable gap
fixes. Each ends green (fmt + clippy `-D warnings` + `test --all-features` + conformance).

### R0 — Fixtures + oracle (no engine change)  ·  branch `test/waalbrug-fixtures`
Extract the companion doc's Turtle into `tests/fixtures/waalbrug/` (`vocab.ttl`,
`waalbrug.trig`, `shapes-core.ttl`, `shapes-sparql.ttl`, `shapes-af.ttl`, `pass/*.ttl`,
`fail/*.ttl`). Add a `tests/waalbrug_conformance.rs` that loads them and encodes the §6
matrix. **Run it to discover the real, empirical gap set** (some "gaps" above may already
pass; some may surface new ones). Adjust the §6.2/§6.3 rule fixtures so a 5/6 conditiescore
actually fires, plus a ≤4 negative case (brief calls this out). Mark expected-failing cases
`#[ignore]` with a gap ID so the suite is green and the backlog is visible. **Decide here**
whether to adopt `insta` for report snapshots or assert on a canonicalized report string with
the repo's existing inline style (lean: match existing style unless snapshots clearly win).

### R1 — `sh:prefixes` injection (G1)  ·  branch `feat/shacl-sparql-prefixes`
Load `sh:prefixes`→`sh:declare`(`sh:prefix`/`sh:namespace`) and prepend `PREFIX` lines to
every `sh:select`/`sh:ask`/`sh:construct`/SPARQL-target/SPARQL-function body; fail loudly
(not silently) on unparseable queries. Highest leverage: unblocks most of §5 and §6.

### R2 — Complex property paths (G2)  ·  branch `feat/shacl-property-paths`
Add an RDF→`PropertyPath` parser (predicate IRI, `sh:inversePath`, RDF-list sequence,
`sh:alternativePath`, `sh:zeroOrMorePath`/`oneOrMore`/`zeroOrOne`) walking blank-node cells
via the raw quad index. The enum + `to_sparql` already exist.

### R3 — GML parse + round-trip (G5)  ·  branch `feat/geosparql-gml`
Parse `geo:gmlLiteral` (GML 3.2 point/line/polygon at minimum) → GEOS; round-trip; make it
work everywhere `parse_wkt_literal` is used. Flip the `geosparql_conformance.rs` tracked-gap
test to passing.

### R4 — CRS transforms + `geof:transform` (G6)  ·  branch `feat/geosparql-crs-transform`
Add feature-gated `proj-transform` (`proj` crate) with a CRS registry + `geof:transform`,
covering 28992↔4326↔3857. Property test for inverse within tolerance. Skip-with-notice when
the feature is off (CI runs `--all-features`). Unblocks the M6 viewer-feed reprojection.

### R5 — `sh:expression` subset (G3) + `sh:SPARQLFunction` (G4)  ·  branch `feat/shacl-af-expr-fn`
Implement the `sh:expression` path+comparison subset (enough for §6.4) and document the
boundary. Implement `sh:SPARQLFunction`: parse `sh:parameter`/`sh:order`/`sh:returnType`,
register a closure that binds params and runs the stored `sh:select`/`sh:ask` via
`store.query`, into the function table so it's callable from queries, constraints and rules
(`ex:afstandMeter`). `$value`/`$PATH` pre-binding (G9) folds in cheaply here.

### R6 — Backend product capabilities + conformance suites (M6 + DoD 5)  ·  branch `feat/waalbrug-viewer-compliance`
- **Viewer feed**: HTTP route returning, per element, `{id, label, type, ifcGuid, gltfUrl,
  ifcUrl, conditiescore, wkt4326, wkt3857, coordinateSystem}` — FOG/OMG/GOM path resolution
  (§7.1) + R4 reprojection. Plus an **element-detail** path (all RDF properties of one
  element) — likely reuses the existing resource/dereference endpoint, so the frontend can
  fetch linked data on selection.
- **Compliance**: reuse Studio's report persistence; add the §7.3/§7.4 queries + severity
  rollup. Most of this exists — wire + test end-to-end on the Waalbrug data.
- **Vendor official conformance suites** (locked decision): git-submodule the W3C SHACL
  data-shapes suite + OGC GeoSPARQL 1.1 suite; run as CI tests with an EARL/Turtle report
  under `docs/conformance/`, plus the GeoSPARQL-SHACL round-trip (Waalbrug data validated by
  GeoSPARQL's own SHACL requirements shapes, via our engine). Author `docs/conformance/`
  summaries + update `docs/geosparql.md`/`docs/shacl.md` capability matrices.

### R7 — Visualization: 3D viewer + interactive map (added requirement)  ·  branch `feat/waalbrug-visualization`
Frontend (Svelte 5 + Vite + Tailwind). Builds on the R6 feed and the existing
`GeoPreview.svelte` (Leaflet) + linked-data panels (`RdfTerm`, browse components,
cytoscape `graphViewer.ts`).

- **Map view** — generalise `GeoPreview` into a *kunstwerk* map: draw the bridge tracé +
  **each part as an identified, clickable feature** (carrying its element IRI), styled by
  type/conditiescore. Coordinates come pre-reprojected to EPSG:4326 from the R6 feed (R4),
  so RD-New data lands correctly in Nijmegen. Reuse the already-loaded **Leaflet** (decide:
  promote from CDN to an npm dep for offline/CSP — see Q5).
- **3D viewer** — *new* component loading `fog:asGltf_v2.0-glb` per element via **three.js +
  GLTFLoader**, GOM coordinate-system aware (ref: brief §9 fog-demo-app). Element **picking**
  (raycaster) selects a part. glTF-first; IFC rendering (web-ifc) is a documented stretch —
  the feed still exposes the `fog:asIfc` URL. (Decide three.js vs CesiumJS+3D-Tiles — Q4.)
- **Shared selection → linked data**: clicking a feature (map) or mesh (3D) loads that
  element's RDF (R6 element-detail) into a side panel, reusing the existing resource/term
  rendering + cytoscape neighbourhood. Map ↔ 3D ↔ panel stay in sync on selection.
- New Svelte components get i18n keys (project convention). Vitest unit + a Playwright e2e
  smoke (load Waalbrug demo → select a part → assert its data panel shows).

Un-ignore fixture cases as each gap closes; DoD met when the §6 matrix is exactly satisfied
**and** a part is selectable in both map and 3D with its linked data shown.

---

## 10. Open questions

**Resolved at review:** Q1 vendoring → **yes, vendor official suites** (folded into R6).
Q2 snapshots → **no, match repo inline style**. Q3 scope → **full R0–R6** (+R7 added).

**Resolved at review (R7 + uom):**

4. **3D library → `three.js + GLTFLoader`** (with a documented CesiumJS/3D-Tiles path left
   open for future globe-scale needs).
5. **Leaflet → bundle as an npm dependency**; migrate `GeoPreview.svelte` off the CDN loader.
6. **`uom` (G7) → defer to R4**; planar-in-CRS-units is correct for the metre-based EPSG:28992
   Waalbrug data, generalise alongside the CRS-transform work.
