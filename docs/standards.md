# Supported Standards

The following W3C and OGC standards are implemented. Support levels are this
project's own grades, from its test suites and a conformance review of the
engine and high-complexity edge cases. They are not W3C or OGC conformance
claims, and nothing here is OGC-certified:

- **Full** — the normative core plus tested edge cases pass.
- **Partial** — core works; specific features are unimplemented or deviate (see
  [Known limitations](#known-limitations--conformance-findings)).

A grade that depends on an optional component says so in its footnote. Such a
component is one this project builds, publishes and tests in CI itself (for
example the OWL 2 DL reasoner sidecar); without it the footnote's fallback
applies.

| Standard | Role | Support |
|---|---|---|
| RDF 1.1 | Core triple data model | Full |
| RDF-star (CG) / RDF 1.2 (WD) | Quoted/nested triples `<< >>` | Partial¹ |
| SPARQL 1.1 Query | SELECT, ASK, CONSTRUCT, DESCRIBE | Full² |
| SPARQL 1.1 Update | INSERT, DELETE, LOAD, CLEAR, COPY, WITH/USING | Full |
| SPARQL 1.1 Protocol | Query and update over HTTP (`/sparql`) | Full² |
| SPARQL 1.1 Graph Store HTTP | Named-graph CRUD over HTTP | Full |
| SPARQL 1.1 Federated Query (`SERVICE`) | Remote query | Full³ — deny-by-default: off until endpoints are allowlisted |
| SPARQL 1.1 Service Description | Capability advertisement | Full |
| SPARQL 1.2 (WD) | Triple terms, accessor functions | Partial¹ |
| RDFS | subClass/subProperty/domain/range inference | Full |
| OWL 2 QL | Profile reasoning (materialised) | Full¹⁰ |
| OWL 2 EL | Profile reasoning (materialised) | Full¹⁰ |
| OWL 2 RL | Profile reasoning (materialised) | Partial¹⁰ |
| OWL 2 DL | Description-logic expressivity | Full⁴ (with the reasoner sidecar) |
| GeoSPARQL 1.1 | Spatial RDF, relation/metric functions | Partial⁵ |
| SHACL Core | Structural constraint validation | Full⁶ |
| SHACL Advanced (AF / SPARQL) | SPARQL constraints, rules, targets | Partial⁷ |
| SHACL-C | Compact-syntax parser/serializer | Partial⁸ |
| OPM (Ontology for Property Management) | Property states with history | Partial — `opm:Property` / `opm:PropertyState` / current-outdated / reliability classes via the property-state API; no `opm:Calculation` or derived-property inference. See [datasets.md](datasets.md#time-evolving-properties-opm-profile). |
| buildingSMART IDS 1.0 | Information Delivery Specification → SHACL | Partial — entity, property, attribute, partOf facets with value restrictions and cardinality; classification and material facets target `props:ifcClassification` / `props:ifcMaterial`, which the IFC lift emits; predefinedType by convention only; dataset-level existence not enforced. See [shacl.md](shacl.md#importing-constraint-specifications-ids). Round-trips: export back to IDS 1.0 covers the shared subset, and anything outside the IDS facet model is reported as a loss. |
| ISO 21597-1 ICDD | Information container for linked document delivery | Partial — Part 1 containers import (documents, linksets, payload triples, ontology resources, index) and export (RDF/XML index); Part 2 not interpreted. See [containers.md](containers.md). |
| RDF Patch (RDF Delta) | Change log line format; patch logs | Full — the format (`H`, several `TX`/`TC`/`TA` blocks, `PA`/`PD` in keyword or quoted form, `A`/`D` triples and quads) applied atomically per dataset, through the SHACL write gates; blank nodes (`_:x`, `<_:x>`) name the store's own nodes; `PA`/`PD` change the dataset's prefix table, which its Turtle/TriG exports declare; version diffs served as patches with prefix changes, chained through `H prev`. RDF Patch Logs per dataset: append with `H id`/`H prev` checking (409), `init`, `current`, `patch/{version\|id}`; version cuts are journaled, other writes are not. Extensions: prefixed names in `A`/`D` rows, `?graph=` for triples. RDF 1.2 triple terms are refused (RDF Patch has no syntax for them). Spec-derived rules in `tests/rdf_patch_conformance.rs`. See [versioning.md](versioning.md#rdf-patch). |
| LDES / TREE | Event streams of version objects; hypermedia fragmentation | Full — publisher: time-ordered fixed-size fragments under one root node with `GreaterThanOrEqualToRelation` / `LessThanOrEqualToRelation` bounds, frozen once full; monotonic timestamps; entity-level version objects with declared delete path and object; `tree:shape`, `ldes:pollingInterval`, ETag / `304`, `429` when busy, dereferenceable members; retention policies (`fullLogDuration`, `versionAmount`, `versionDuration`, `versionDeleteDuration`, `startingFrom`) enforced inside frozen pages with `410 Gone` for a compacted node. Client (unordered mode): §3.1 initialisation, allowlisted redirects, retries with back-off, every listed RDF format, §3.4 member extraction with named graphs, SHACL-path version semantics, a bookmark on `xsd:dateTime` values, immutable pages fetched once, legacy retention classes. Optional and not implemented: spatial and substring fragmentations, search forms, transactions, ordered mode, scheduled polling. Spec-derived rules in `tests/ldes_conformance.rs` (no external corpus exists). See [ldes.md](ldes.md). |
| LDP (Linked Data Platform) 1.0 | Basic/Direct/Indirect Containers; NonRDFSource; per-resource access control with Web Access Control (`.acl` resources, `acl:` vocabulary, `Link rel="acl"`, `WAC-Allow`) | Full — WAC agents are this store's principals; no WebID-TLS / Solid-OIDC, no `acl:origin`. See [ldp.md](ldp.md#access-control). |
| DCAT 3 / DCAT-AP 3 / DCAT-AP-NL 3 | Dataset catalogue description; EU / NL application profiles | Partial — DCAT 3 catalogue with VoID statistics; `DCAT_PROFILE` adds the AP/AP-NL mandatory properties (typed agents, identifiers, language, file types, data services, EU-authority statuses); no `dcat:CatalogRecord`, no temporal coverage, and the official DCAT-AP SHACL suite is not run in CI. See [dcat.md](dcat.md). |
| RML / R2RML | CSV/JSON/XML files and SQL / SPARQL datasources → RDF | Partial⁹ |
| JWT / OAuth 2.0 / OIDC | Authentication | Full |
| SAML 2.0 | Authentication | Experimental — not in the `full` feature or the published image. SP-initiated Web Browser SSO (HTTP-Redirect AuthnRequest, HTTP-POST response bound to the request, signed by the configured IdP certificate); no IdP-initiated SSO, signed AuthnRequests, encrypted assertions or Single Logout. Tested against a simulated IdP only. See [auth.md](auth.md#saml-20). |
| ShEx | Shape Expressions (ShExC) | Partial — node kinds, datatypes with lexical checks, string/numeric facets, value sets, cardinalities, EachOf/OneOf, inverse constraints, CLOSED/EXTRA, shape references; no semantic actions, imports or annotations. Semantics pinned by `tests/shex_conformance.rs`. |
| SWRL | Horn-clause rules | Partial — class/property atoms and the built-ins in `src/swrl`; an unsupported built-in is a hard error rather than a silently dropped filter. Semantics pinned by `tests/swrl_conformance.rs`. |

## Conformance test suites

Conformance and high-complexity stress tests live in `tests/`. Each suite encodes
expected results taken from the specification text; intentional non-conformances
are encoded as documented, flip-when-fixed tests. Two things the table makes
explicit: only the rows marked **vendored** run a published test corpus — every
other suite is hand-written and *derived from* its spec, not from a W3C, OGC or
community corpus — and the counts are generated from the suites themselves, so
they cannot drift from the code. Whether a vendored corpus's results are
published depends on its licence: each corpus has its own page under
[conformance/](conformance/) (for example [shacl.md](conformance/shacl.md),
[geosparql.md](conformance/geosparql.md) and [sparql11.md](conformance/sparql11.md)).
The SPARQL sections, for one, are a subset of a W3C test suite, and W3C's
test-suite licence policy allows no performance claims on a subset, so their
page tracks known gaps but gives no score.

<!-- conformance-table:start -->
| Standard | Suite | Basis | Tests | Notes |
|---|---|---|---:|---|
| SPARQL 1.1 Protocol / Graph Store | `tests/api_protocol_conformance.rs` | spec-derived | 27 |  |
| DCAT 3 / DCAT-AP 3 / VoID | `tests/dcat_conformance.rs` | spec-derived | 4 |  |
| GeoSPARQL 1.1 | `tests/geosparql_conformance.rs` | spec-derived | 143 |  |
| LDES 1.0 / TREE | `tests/ldes_conformance.rs` | spec-derived | 28 |  |
| LDP 1.0 (store level) | `tests/ldp_conformance.rs` | spec-derived | 43 |  |
| LDP 1.0 (HTTP) | `tests/ldp_http_conformance.rs` | spec-derived | 13 |  |
| OGC GeoSPARQL 1.1 validator shapes | `tests/ogc_geosparql_shacl_roundtrip.rs` | **vendored OGC corpus** (unmodified) | 2 |  |
| OWL 2 DL | `tests/owl2_dl_conformance.rs` | spec-derived (+ live tests against the reasoner sidecar) | 73 |  |
| OWL 2 EL | `tests/owl2_el_conformance.rs` | spec-derived | 57 |  |
| OWL 2 QL | `tests/owl2_ql_conformance.rs` | spec-derived | 45 |  |
| OWL 2 RL | `tests/owl2_rl_conformance.rs` | spec-derived | 61 |  |
| RDF 1.1 formats | `tests/rdf11_conformance.rs` | spec-derived | 69 |  |
| RDF Patch (RDF Delta) | `tests/rdf_patch_conformance.rs` | spec-derived | 24 |  |
| RDFS entailment | `tests/rdfs_conformance.rs` | spec-derived | 23 |  |
| RML / R2RML | `tests/rml_conformance.rs` | spec-derived | 38 |  |
| SHACL Advanced Features | `tests/shacl_af_corpus.rs` | **vendored TopQuadrant corpus** (expression, function, rule and target tests of TopQuadrant/shacl, unmodified; dash-driven) | 1 | 10 corpus cases: 9 pass, 1 known failure, 0 runner-side skips (floor ≥9 asserted) |
| SHACL Core | `tests/shacl_conformance.rs` | spec-derived | 62 |  |
| SHACL-AF rules | `tests/shacl_rules_conformance.rs` | spec-derived | 43 |  |
| SHACL Compact Syntax | `tests/shaclc_conformance.rs` | spec-derived | 11 |  |
| ShEx | `tests/shex_conformance.rs` | spec-derived | 10 |  |
| SPARQL 1.2 / RDF-star | `tests/sparql12_conformance.rs` | spec-derived | 17 |  |
| SP2B / BSBM query shapes | `tests/sparql_benchmarks.rs` | benchmark-derived | 28 |  |
| SPARQL 1.1 functions | `tests/sparql_functions_conformance.rs` | spec-derived | 9 |  |
| SPARQL engine coverage (sparqloscope) | `tests/sparqloscope_conformance.rs` | sparqloscope-derived | 67 |  |
| Cross-standard HTTP smoke | `tests/standards_conformance.rs` | spec-derived | 26 |  |
| SWRL | `tests/swrl_conformance.rs` | spec-derived | 15 |  |
| OWL 2 DL | `tests/w3c_owl2_dl_manifests.rs` | **vendored W3C test cases** (approved OWL 2 DL / Direct Semantics cases of the OWL 2 Test Case Repository, unmodified; manifest-driven, against the reasoner sidecar) | 2 | runs in CI against the reasoner sidecar as a development and regression ratchet; no score is published (W3C licence: no performance claims on a partial run); known gaps in `docs/conformance/owl2-dl.md` |
| SHACL Core | `tests/w3c_shacl_conformance.rs` | **vendored W3C corpus** (core + sparql sections, manifest-driven, full report equality) | 1 | 136 corpus cases: 120 pass, 0 known failures, 1 optional feature unsupported (reported as the failure the spec requires), 15 runner-side skips (floor ≥90 asserted) |
| SPARQL 1.1 Query/Update | `tests/w3c_sparql11_conformance.rs` | spec-derived (+ cx01–cx15 high-complexity) | 126 |  |
| SPARQL 1.1 Federated Query | `tests/w3c_sparql11_federation.rs` | **vendored W3C test-suite subset** (`service/` + `syntax-fed/` sections of w3c/rdf-tests, unmodified; manifest-driven, local endpoints) | 1 | runs in CI as a development and regression ratchet against local endpoints; no score is published (W3C test-suite policy); see `docs/conformance/sparql11.md` §Federation |
| SPARQL 1.1 Query/Update | `tests/w3c_sparql11_manifests.rs` | **vendored W3C test-suite subset** (query + update sections of w3c/rdf-tests, unmodified; manifest-driven) | 1 | runs in CI as a development and regression ratchet; no score is published (W3C test-suite policy); known gaps in `docs/conformance/sparql11.md` |

1070 conformance tests across 31 suites; a further 795 tests in 106 integration, security and regression suites under `tests/`, plus the crate's unit tests. Only the 6 **vendored** rows run a published corpus; every other suite is hand-written and derived from the specification text. A vendored row gives results only where its corpus licence allows performance claims; those are development and regression results on the vendored sections (`docs/conformance/`), not W3C, TopQuadrant, OGC or other conformance claims. The W3C SPARQL 1.1 sections (query, update and federation) and the OWL 2 DL test cases are partial runs of W3C test suites, so they carry no results and are used for development and bug tracking only.

_Generated by `scripts/conformance_table.py` — edit the suites, not the table._
<!-- conformance-table:end -->

Run them in the Docker builder image (native build needs GEOS/pkg-config):

```bash
docker run --rm -v "$PWD:/app" -v ots_target:/app/target -w /app ots-builder \
  cargo test --all-features --locked --test '*conformance*'
```

## Known limitations & conformance findings

These were surfaced by the conformance suites above. Tracked tests pin current
behavior and will flip green when the limitation is resolved.

1. **RDF 1.2, not RDF-star CG.** The engine (oxigraph 0.5) implements the
   **RDF 1.2 / SPARQL 1.2** model: a *triple term* `<<( s p o )>>` in **object
   position only**, attached through `rdf:reifies`, plus `{| |}` annotation
   syntax. `<< s p o >>` is reifier shorthand — it mints a reifier and does not
   assert the base triple. The reifier is an ordinary IRI/blank node, so
   `isTRIPLE` is false for it and true for the triple term it points at.
   Code written against the older RDF-star CG model (quoted triples usable in
   subject position) needs updating; see `tests/sparql12_conformance.rs`.
2. **SPARQL 1.1 Query** and **SPARQL 1.1 Protocol** are graded *Full* (since
   2026-10-03).
   - **The engine** is oxigraph 0.5.11 with its SPARQL parser, evaluator and
     optimizer (spargebra 0.4.7, spareval 0.2.7, sparopt 0.3.7) vendored and
     patched ([vendor/README.md](../vendor/README.md), each change with a draft
     upstream PR): `GRAPH ?g` scoping around non-BGP patterns and the RDF merge of
     several `FROM` graphs (both backported from oxigraph's main branch),
     zero-length property paths with a constant endpoint, `GROUP_CONCAT` returning
     an `xsd:string`, `BNODE(str)` fresh per solution, and nested aggregates
     refused. Among them are all the engine gaps that the vendored W3C query and
     update sections had found (bug tracking only, with no score:
     [conformance/sparql11.md](conformance/sparql11.md)); none is open. The hand-written, spec-derived suite is
     `tests/w3c_sparql11_conformance.rs`, and every W3C query entry also runs
     through the in-memory mirror (shards, columnar copy, full copy), which must
     answer as the engine does. (Parallel shards do not evaluate `EXISTS` per
     shard: since 2026-10-02 a query with `EXISTS` / `NOT EXISTS` is not split.)
   - **The dataset over HTTP** is the one SPARQL defines, confined to the graphs
     the caller may read ([api-reference.md](api-reference.md#the-dataset-of-a-sparql-query)):
     `FROM` / `FROM NAMED`, or the protocol's `default-graph-uri` /
     `named-graph-uri` (which take precedence), keep their meaning; a graph the
     caller may not read is dropped as if empty; only a request that names no
     dataset gets the union of the readable graphs as its default graph (each
     also a named graph, advertised as `sd:UnionDefaultGraph`). The dataset is set
     on the parsed query, not spliced into its text; a fail-closed check re-parses
     the final query of every non-admin request. Updates honour
     `using-graph-uri` / `using-named-graph-uri`, and refuse them next to an
     operation's own `USING` / `WITH` (400).
   - **Protocol evidence:** `tests/api_protocol_conformance.rs` covers query by
     GET, direct POST and form POST, update by direct and form POST (and its
     authentication), the four dataset parameters, content negotiation and
     status codes; `tests/sparql_scope_boundary_http.rs` the read boundary.
     The W3C protocol test section is not vendored.
3. **Federation/`SERVICE` is deny-by-default.** It is off as an SSRF mitigation
   until `OTS_REMOTE_ALLOWLIST` lists the URL prefixes the server may contact; a
   `SERVICE` naming anything else is a failed invocation — an error, or under
   `SERVICE SILENT` the single empty solution — which is the spec's own failure
   semantics, so the posture does not lower the grade. The §3.2 semantics
   (`Service(IRI, P, Silent)`) are tested against the `service/` and
   `syntax-fed/` sections of the W3C SPARQL 1.1 test suite, vendored unscored
   (`tests/w3c_sparql11_federation.rs`, local endpoints), and `SERVICE ?var`
   follows informative §4: the variable is bound by the pattern before the
   `SERVICE`, and the endpoint is called once per distinct value. Exceeding the
   per-call timeout (`OTS_REMOTE_TIMEOUT_SECS`), body limit
   (`OTS_REMOTE_MAX_BYTES`) or row cap (`OTS_SERVICE_MAX_ROWS`), or the
   per-query endpoint and request caps (`OTS_SERVICE_MAX_ENDPOINTS`,
   `OTS_SERVICE_MAX_CALLS`), is a failed invocation, never a truncated result;
   past the per-query deadline (`OTS_SERVICE_DEADLINE_SECS`) the query fails,
   `SILENT` or not. Without an allowlist the service description does not
   advertise `sd:BasicFederatedQuery`; with one it does. Not implemented:
   pushing local bindings to the remote (informative §2.4) — each `SERVICE`
   pattern is sent as written and joined locally. See
   [federation.md](federation.md#service-what-a-call-returns).
4. **OWL 2 DL** is Full with the reasoner sidecar the project ships:
   OWL API 5 + HermiT (`sidecars/reasoner/`, image
   `ghcr.io/philipperenzen/open-triplestore-reasoner`, started with
   `docker compose --profile reasoner` and `OTS_DL_BACKEND=sidecar`). CI builds
   it and runs the approved OWL 2 DL / Direct Semantics test cases of the W3C
   OWL 2 Test Case Repository through `POST /api/reasoning/check` against it,
   as a regression ratchet whose known failures are listed in
   [conformance/owl2-dl.md](conformance/owl2-dl.md); no score is published, as
   the suite's licence terms do not allow one for a partial run. Every run
   checks the OWL 2 DL profile (typing constraints, §11 global restrictions);
   `POST /api/reasoning/check` answers consistency, entailment,
   satisfiability and profile questions. What a materialisation writes is
   limited to named entities, and HermiT does not report data values entailed
   by `owl:hasValue` ([owl2-dl.md](owl2-dl.md)). **Without the sidecar** the
   grade does not apply: `OTS_DL_BACKEND=native` runs OWL 2 RL plus DL-syntax
   rules (hasSelf both ways, ReflexiveProperty, disjointUnion) to one joint
   fixed point, which is sound but not complete, and Konclude
   (`OTS_DL_BACKEND=konclude`) is not shipped or run in CI.
5. **GeoSPARQL 1.1:** WKT, GML and GeoJSON (RFC 7946, CRS84) geometry literals,
   with `geof:asGeoJSON`; the full topology family
   (sf/eh/rcc8); `geof:relate` with DE-9IM patterns; distance, area, buffer,
   getSRID and the constructive functions; the metric family
   (`geof:metricDistance`, `metricLength`, `metricPerimeter`, `metricArea`,
   `metricBuffer`) in metres on the WGS84 ellipsoid; `geof:transform` between the
   built-in CRSs (RD New, CRS84, EPSG:4326 in authority axis order, Web
   Mercator), keeping Z and unbound outside a CRS's domain; binary functions,
   `geof:relate` included, harmonise their operands' CRSs. A literal's CRS is its
   WKT prefix or GML `srsName` for every function, axis order included, and an
   empty WKT/GML/GeoJSON literal is the empty geometry. Units are OGC, QUDT or
   EPSG IRIs (or `xsd:anyURI`); `geof:distance` and `geof:buffer` with a linear
   unit on a geographic CRS are geodesic, on a projected CRS planar in its
   metres, and an unknown or incompatible unit is unbound. The `geof:aggUnion`
   aggregate, with or without `GROUP BY`. **Not
   implemented:** KML/DGGS literals, the Query Rewrite Extension, the other
   aggregates (`aggBoundingBox`, `aggBoundingCircle`, `aggCentroid`,
   `aggConcaveHull`, `aggConvexHull`), and the non-metric functions GeoSPARQL 1.1
   added besides `transform` and `asGeoJSON` (`length`, `perimeter`, `centroid`,
   `geometryN`, `isEmpty`, `asWKT`, …). **Wrong or missing answers today:** a GML
   literal's `srsName` is honoured by the metric functions only, not by the
   topology and constructive functions, `getSRID`, `transform` or `aggUnion`; a
   unit IRI other than the OGC units (a QUDT unit, say) is ignored and the
   answer comes back in the CRS's own units; `geof:transform` passes a
   coordinate through unchanged when it cannot transform it, and drops Z/M;
   `geof:relate` does not bring its operands into one CRS; `ehCoveredBy` uses
   GEOS `coveredBy` rather than the spec's DE-9IM mask; and an empty WKT, GML or
   GeoJSON literal (`""`) is unbound instead of the empty geometry. Operations
   across two CRSs work only between the built-in CRSs above. No query uses the
   spatial R-tree in `src/geo/spatial_index.rs`.
6. **SHACL Core** — the Core constraint components are implemented, and
   blank-node property shapes (`sh:property [ … ]`, the standard idiom) are
   enforced (the loader dereferences blank nodes through the raw quad index;
   this applies to SHACL-on-write too). Graded *Full* since 2026-10-03: the
   W3C SHACL test suite's core section passes with no known failure
   ([conformance/shacl.md](conformance/shacl.md)), compared at full result-set
   equality — focus node, `sh:resultPath`, `sh:value`, source shape,
   constraint-component IRI, severity and `sh:sourceConstraint`, all but the
   message (since 2026-10-02; it used to be `sh:conforms` and the focus nodes
   only). The last known failure, `core/property/uniqueLang-002`, passes since
   the store keeps literals as written: it used to read `"1"^^xsd:boolean` back
   as `true`, the 12 types derived from `xsd:integer` as `xsd:integer` and
   `xsd:dateTimeStamp` as `xsd:dateTime`, so `sh:uniqueLang "1"` activated the
   constraint and `sh:datatype` with a derived type rejected valid data
   ([shacl.md](shacl.md#literal-forms)). Multi-valued
   parameters, `sh:deactivated` on property and inline shapes, ill-formed paths
   and failing SPARQL targets follow the spec since 2026-10-01 (they used to
   pass data the shapes forbid). A run over several data graphs validates their
   merge (§3.4): since 2026-10-02 `sh:path` reads it too, for every focus node
   (it used to walk an IRI focus node's path inside each graph in turn, so a
   path crossing graphs found nothing that `sh:sparql` found).
7. **SHACL Advanced** — implemented: SPARQL-based targets (`sh:target` with
   `sh:select`, `sh:SPARQLTarget` and `sh:SPARQLTargetType`, parameters bound
   as terms); `sh:sparql` constraints and custom constraint components
   (`sh:parameter`, `sh:validator` / `sh:nodeValidator` / `sh:propertyValidator`
   with `sh:ask` or `sh:select`, optional parameters, one constraint per value of
   a single parameter); pre-binding of `$this`, `$value` and parameters as RDF
   terms, so blank-node focus and value nodes are checked (since 2026-10-02;
   they used to be skipped); `?failure`, `?message` and `{?var}` message
   templates; `sh:deactivated` on constraints and validators;
   `sh:SPARQLFunction` (`sh:select` or `sh:ask` bodies, arguments bound as
   terms, bodies reading the run's data graphs, scoped to the declaring shapes
   graph); all seven node expressions of the 2017 Note and expression
   constraints (`sh:expression`, with the Note's semantics since 2026-10-02);
   triple and SPARQL rules with `sh:order`, `sh:condition` and `sh:deactivated`
   ([shacl.md](shacl.md)). Results name their constraint component IRI, and a
   SPARQL constraint's results its `sh:sourceConstraint`. **Optional, not
   supported:** `$shapesGraph` and `$currentShape` (SHACL §5.3.1). A constraint
   that uses them fails the shapes graph at load, which is the failure the spec
   requires of a processor without them; the W3C test `shapesGraph-001` is
   counted as "optional, unsupported", not as a pass or a known failure
   (w3c/data-shapes#426). The W3C corpus is compared at full result-set
   equality (all but `sh:resultMessage`;
   [conformance/shacl.md](conformance/shacl.md)); TopQuadrant's SHACL-AF tests:
   9 of 10 cases pass ([conformance/shacl.md](conformance/shacl.md)). **Not
   implemented:** `sh:resultAnnotation`.
8. **SHACL-C** is a pragmatic subset: `[min..max]` counts, `closed`, and `// "msg"`
   messages. The parser rejects unrecognized trailing input (it used to discard it
   silently, which could empty a shape graph on upload with a 200).
9. **RML / R2RML** — CSV/JSON/XML *file* sources with template/reference/constant
   term maps, datatype and language tags, `rr:class`, and inline blank-node term
   maps ([rml.md](rml.md)); relational logical sources (`rr:tableName`,
   `rml:query`, R2RML's `rr:logicalTable` / `rr:sqlQuery`) over registered
   PostgreSQL, MySQL / MariaDB and SQL Server datasources and virtual SPARQL
   sources, with referencing object maps (`rr:parentTriplesMap` joins) resolved
   there ([sources.md](sources.md)). Terms follow R2RML: §7.4 term types
   (constants keep their kind, datatype and language; template objects are
   IRIs), §7.3 IRI-safe encoding of IRI templates only, blank nodes per value
   and graph, union graph-map semantics with `rr:defaultGraph`, a base IRI
   (`rml:baseIRI` or the run's), and delimited / schema-qualified SQL
   identifiers. A predicate-object map generates every predicate map × every
   object map. A non-conforming mapping is refused at parse with the construct
   named, a column the source lacks is an error, and a data error (§4.3: an
   invalid IRI, an ill-typed datatype override) aborts the run and names the
   rows unless the run opts into skipping and reporting them. An empty value
   is a value; RML-IO `rml:null` lists the values that count as NULL. A
   datasource mapping version frozen before these rules keeps the old term
   rules (an empty value generates no term there). **Not implemented:** joins
   on file sources (the mapping is refused), and the RML-Core / RML-IO
   vocabulary beyond `rml:baseIRI` and `rml:null`.
10. **OWL 2 RL / EL / QL.**
    - **RL** runs 75 of the 78 RL/RDF rules, lists of any length and inverse
      property expressions included (`eq-ref` on request); the Table 8 rules
      `dt-type2`, `dt-eq` and `dt-diff` are not run, so literals are matched as
      terms, not by value.
    - **EL** is a native EL++ saturation engine covering the whole profile —
      intersections, existentials, `owl:hasValue`, one-individual `owl:oneOf`,
      `owl:hasSelf`, the role hierarchy, property chains, transitivity,
      reflexivity, ranges, disjointness, keys, equality and negative assertions,
      and the EL datatype map with value semantics — with classification,
      realization and the property-assertion closure; axioms outside the profile
      are left out and reported (`ignored`). One storage limit applies: an
      integer-derived XSD literal is stored as `xsd:integer`, so an ill-typed
      one (`"-5"^^xsd:nonNegativeInteger`) is not detected. Both are pinned by
      `tests/owl2_rl_conformance.rs` / `tests/owl2_el_conformance.rs`; the EL
      suite includes randomised differential tests against RL on the EL ∩ RL
      fragment.
    - **QL** covers the whole profile: the DL-Lite_R closure, ground
      materialisation, consistency (negative inclusions, asymmetric/irreflexive
      properties, ill-typed literals, data ranges) and existential rewriting of
      query blank nodes ([OWL 2 QL](/docs/owl2-ql)). Data ranges are decided on
      values through the OWL 2 datatype map. Oxigraph stores integer-derived
      types as `xsd:integer`, so a check reads the value, not the datatype it
      was written with.

    See [owl2-rl.md](owl2-rl.md), [owl2-el.md](owl2-el.md) and
    [owl2-ql.md](owl2-ql.md).

Related guides: [OWL Reasoning](/docs/reasoning), [SHACL Validation](/docs/shacl),
[GeoSPARQL](/docs/geosparql), [Performance](/docs/performance),
[Triplestore comparison](/docs/triplestore-comparison),
[Authentication & API Tokens](/docs/auth).
