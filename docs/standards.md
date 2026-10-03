# Supported Standards

The following W3C and OGC standards are implemented. Support levels are this
project's own grades, from its test suites and a conformance review of the
engine and high-complexity edge cases. They are not W3C or OGC conformance
claims, and nothing here is OGC-certified:

- **Full** — the normative core plus tested edge cases pass.
- **Partial** — core works; specific features are unimplemented or deviate (see
  [Known limitations](#known-limitations--conformance-findings)).

| Standard | Role | Support |
|---|---|---|
| RDF 1.1 | Core triple data model | Full |
| RDF-star (CG) / RDF 1.2 (WD) | Quoted/nested triples `<< >>` | Partial¹ |
| SPARQL 1.1 Query | SELECT, ASK, CONSTRUCT, DESCRIBE | Full² |
| SPARQL 1.1 Update | INSERT, DELETE, LOAD, CLEAR, COPY, WITH/USING | Full |
| SPARQL 1.1 Graph Store HTTP | Named-graph CRUD over HTTP | Full |
| SPARQL 1.1 Federated Query (`SERVICE`) | Remote query | Partial³ — off by default; per-endpoint allowlist |
| SPARQL 1.1 Service Description | Capability advertisement | Full |
| SPARQL 1.2 (WD) | Triple terms, accessor functions | Partial¹ |
| RDFS | subClass/subProperty/domain/range inference | Full |
| OWL 2 QL | Profile reasoning (materialised) | Full |
| OWL 2 EL | Profile reasoning (materialised) | Partial¹¹ |
| OWL 2 RL | Profile reasoning (materialised) | Partial¹¹ |
| OWL 2 DL | Description-logic expressivity | Partial⁴ |
| GeoSPARQL 1.1 | Spatial RDF, relation/metric functions | Partial⁵ |
| SHACL Core | Structural constraint validation | Partial⁶ |
| SHACL Advanced (AF / SPARQL) | SPARQL constraints, rules, targets | Partial⁷ |
| SHACL-C | Compact-syntax parser/serializer | Partial⁸ |
| OPM (Ontology for Property Management) | Property states with history | Partial — `opm:Property` / `opm:PropertyState` / current-outdated / reliability classes via the property-state API; no `opm:Calculation` or derived-property inference. See [datasets.md](datasets.md#time-evolving-properties-opm-profile). |
| buildingSMART IDS 1.0 | Information Delivery Specification → SHACL | Partial — entity, property, attribute, partOf facets with value restrictions and cardinality; classification and material facets target `props:ifcClassification` / `props:ifcMaterial`, which the IFC lift emits; predefinedType by convention only; dataset-level existence not enforced. See [shacl.md](shacl.md#importing-constraint-specifications-ids). Round-trips: export back to IDS 1.0 covers the shared subset, and anything outside the IDS facet model is reported as a loss. |
| ISO 21597-1 ICDD | Information container for linked document delivery | Partial — Part 1 containers import (documents, linksets, payload triples, ontology resources, index) and export (RDF/XML index); Part 2 not interpreted. See [containers.md](containers.md). |
| RDF Patch (RDF Delta) | Change log line format | Partial — version diffs served as patches; patches applied atomically per dataset (`H`, one `TX`/`TC`/`TA`, `PA`/`PD`, `A`/`D` quads); no blank-node deletes, no nested transactions. See [versioning.md](versioning.md#rdf-patch). |
| LDES / TREE | Event streams of version objects; hypermedia fragmentation | Partial — time-ordered fixed-size fragments with `GreaterThanOrEqualToRelation`, frozen once full; entity-level version objects, tombstones; retention policies (`fullLogDuration`, `versionAmount`, `versionDuration`, `versionDeleteDuration`, `startingFrom`) enforced inside frozen pages with `410 Gone` for a compacted node; an incremental client that treats 410 as an empty page. No `tree:shape`, `ldes:versionKey` or spatial/substring fragmentations. Spec-derived rules in `tests/ldes_conformance.rs` (no external corpus exists). See [ldes.md](ldes.md). |
| LDP (Linked Data Platform) 1.0 | Basic/Direct/Indirect Containers; NonRDFSource; per-resource access control with Web Access Control (`.acl` resources, `acl:` vocabulary, `Link rel="acl"`, `WAC-Allow`) | Full — WAC agents are this store's principals; no WebID-TLS / Solid-OIDC, no `acl:origin`. See [ldp.md](ldp.md#access-control). |
| DCAT 3 / DCAT-AP 3 / DCAT-AP-NL 3 | Dataset catalogue description; EU / NL application profiles | Partial — DCAT 3 catalogue with VoID statistics; `DCAT_PROFILE` adds the AP/AP-NL mandatory properties (typed agents, identifiers, language, file types, data services, EU-authority statuses); no `dcat:CatalogRecord`, no temporal coverage, and the official DCAT-AP SHACL suite is not run in CI. See [dcat.md](dcat.md). |
| RML / R2RML | RML-Core / RML-IO, legacy RML and R2RML: CSV/JSON/XML files and SQL / SPARQL datasources → RDF | Full⁹ |
| JWT / OAuth 2.0 / OIDC | Authentication | Full |
| SAML 2.0 | Authentication | Experimental — not in the `full` feature or the published image; the ACS handler has a known request-ID validation defect, so no login can currently succeed. See [auth.md](auth.md). |
| ShEx | Shape Expressions (ShExC) | Partial — node kinds, datatypes with lexical checks, string/numeric facets, value sets, cardinalities, EachOf/OneOf, inverse constraints, CLOSED/EXTRA, shape references; no semantic actions, imports or annotations. Semantics pinned by `tests/shex_conformance.rs`. |
| SWRL | Horn-clause rules | Partial — class/property atoms and the built-ins in `src/swrl`; an unsupported built-in is a hard error rather than a silently dropped filter. Semantics pinned by `tests/swrl_conformance.rs`. |

## Conformance test suites

Conformance and high-complexity stress tests live in `tests/`. Each suite encodes
expected results taken from the specification text; intentional non-conformances
are encoded as documented, flip-when-fixed tests. Two things the table makes
explicit: only the **vendored** rows run a published test corpus (the W3C SPARQL
1.1 query and update manifests, the W3C SHACL core and sparql manifests and the OGC
GeoSPARQL validator shapes) — every other suite is hand-written and *derived
from* its spec, not the W3C/OGC corpus — and the counts are generated from the
suites themselves, so they cannot drift from the code. Results on the vendored
SHACL and GeoSPARQL corpora are in [conformance/shacl.md](conformance/shacl.md)
and [conformance/geosparql.md](conformance/geosparql.md).
[conformance/sparql11.md](conformance/sparql11.md) describes the SPARQL run and
tracks its known gaps; it publishes no score, because the vendored SPARQL
sections are a subset of a W3C test suite and W3C's test-suite licence policy
allows no performance claims on a subset.

<!-- conformance-table:start -->
| Standard | Suite | Basis | Tests | Notes |
|---|---|---|---:|---|
| SPARQL 1.1 Protocol / Graph Store | `tests/api_protocol_conformance.rs` | spec-derived | 17 |  |
| DCAT 2 / VoID | `tests/dcat_conformance.rs` | spec-derived | 4 |  |
| GeoSPARQL 1.1 | `tests/geosparql_conformance.rs` | spec-derived | 130 |  |
| LDP 1.0 (store level) | `tests/ldp_conformance.rs` | spec-derived | 43 |  |
| LDP 1.0 (HTTP) | `tests/ldp_http_conformance.rs` | spec-derived | 13 |  |
| OGC GeoSPARQL 1.1 validator shapes | `tests/ogc_geosparql_shacl_roundtrip.rs` | **vendored OGC corpus** (unmodified) | 2 |  |
| OWL 2 DL extension rules | `tests/owl2_dl_conformance.rs` | spec-derived | 34 |  |
| OWL 2 EL | `tests/owl2_el_conformance.rs` | spec-derived | 14 |  |
| OWL 2 QL | `tests/owl2_ql_conformance.rs` | spec-derived | 21 |  |
| OWL 2 RL | `tests/owl2_rl_conformance.rs` | spec-derived | 30 |  |
| RDF 1.1 formats | `tests/rdf11_conformance.rs` | spec-derived | 63 |  |
| RDFS entailment | `tests/rdfs_conformance.rs` | spec-derived | 23 |  |
| RML / R2RML | `tests/rml_conformance.rs` | spec-derived | 50 |  |
| RML-Core | `tests/rml_core_conformance.rs` | **vendored KG-Construct CG corpus** (the RML-Core test cases, unmodified; manifest-driven) | 2 | 76 corpus cases: 75 pass, 1 known failure, 0 runner-side skips (floor ≥70 asserted) |
| RML-IO (sources) | `tests/rml_io_conformance.rs` | **vendored KG-Construct CG corpus** (the RML-IO source test cases, unmodified; manifest-driven) | 2 | 32 corpus cases: 29 pass, 1 known failure, 2 runner-side skips (floor ≥25 asserted) |
| RML (legacy vocabulary) | `tests/rml_legacy_conformance.rs` | **vendored RML.io corpus** (the CSV, JSON and XML cases of rml-test-cases, unmodified) | 2 | 117 corpus cases: 112 pass, 5 known failures, 0 runner-side skips (floor ≥100 asserted) |
| SHACL Core | `tests/shacl_conformance.rs` | spec-derived | 23 |  |
| SHACL-AF rules | `tests/shacl_rules_conformance.rs` | spec-derived | 20 |  |
| SHACL Compact Syntax | `tests/shaclc_conformance.rs` | spec-derived | 11 |  |
| ShEx | `tests/shex_conformance.rs` | spec-derived | 10 |  |
| SPARQL 1.2 / RDF-star | `tests/sparql12_conformance.rs` | spec-derived | 14 |  |
| SP2B / BSBM query shapes | `tests/sparql_benchmarks.rs` | benchmark-derived | 28 |  |
| SPARQL 1.1 functions | `tests/sparql_functions_conformance.rs` | spec-derived | 9 |  |
| SPARQL engine coverage (sparqloscope) | `tests/sparqloscope_conformance.rs` | sparqloscope-derived | 67 |  |
| Cross-standard HTTP smoke | `tests/standards_conformance.rs` | spec-derived | 26 |  |
| SWRL | `tests/swrl_conformance.rs` | spec-derived | 4 |  |
| R2RML | `tests/w3c_r2rml_conformance.rs` | **fetched W3C test cases** (pinned commit + sha256, not vendored; SQLite here, PostgreSQL and MySQL in the live-database job) | 5 | runs in CI as a development and regression ratchet over the cases fetched by `scripts/fetch-w3c-r2rml-tests.sh`; no score is published (W3C test-suite policy) |
| SHACL Core | `tests/w3c_shacl_conformance.rs` | **vendored W3C corpus** (core + sparql sections, manifest-driven) | 1 | 136 corpus cases: 119 pass, 2 known failures, 15 runner-side skips (floor ≥90 asserted) |
| SPARQL 1.1 Query/Update | `tests/w3c_sparql11_conformance.rs` | spec-derived (+ cx01–cx15 high-complexity) | 125 |  |
| SPARQL 1.1 Query/Update | `tests/w3c_sparql11_manifests.rs` | **vendored W3C test-suite subset** (query + update sections of w3c/rdf-tests, unmodified; manifest-driven) | 1 | runs in CI as a development and regression ratchet; no score is published (W3C test-suite policy); known gaps in `docs/conformance/sparql11.md` |

794 conformance tests across 30 suites; a further 713 tests in 104 integration, security and regression suites under `tests/`, plus the crate's unit tests. Only the 7 **vendored** rows run a published corpus; every other suite is hand-written and derived from the specification text. The SHACL, GeoSPARQL and RML corpus results are development and regression results on the vendored sections (`docs/conformance/`), not W3C or OGC conformance claims. The SPARQL 1.1 sections are a subset of a W3C test suite, so under W3C's test-suite licence policy they are used for development and bug tracking only, and no score is published for them; the same holds for the W3C R2RML test cases, which CI fetches at a pinned commit rather than vendoring.

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
2. **Zero-length property paths.** `:x :p* ?y` does not yield a constant start node
   `:x` when `:x` is absent from the data (oxigraph behavior; the ALP algebra would
   include it).
3. **Federation/`SERVICE` is off by default** as an SSRF mitigation and can be
   enabled per endpoint: `OTS_REMOTE_ALLOWLIST` lists the URL prefixes the
   server may contact; a `SERVICE` naming anything else errors (or yields no
   rows under `SERVICE SILENT`). Every call has a timeout and a row cap
   (`OTS_SERVICE_MAX_ROWS`). Without an allowlist the service description does
   not advertise `sd:BasicFederatedQuery`; with one it does. Not supported:
   `SERVICE ?var` (a variable endpoint) and pushing local bindings to the
   remote — each SERVICE is evaluated as a stand-alone query and joined locally.
4. **OWL 2 DL** reasoning is RL-based forward-chaining plus DL-syntax extension
   rules (hasSelf, disjointUnion, NegativePropertyAssertion, hasKey, cardinality).
   Full DL tableau (consistency detection, profile validation, nominal/datatype
   reasoning) requires the external reasoner bridge (e.g. Konclude).
5. **GeoSPARQL 1.1:** WKT, GML and GeoJSON (RFC 7946, CRS84) geometry literals,
   with `geof:asGeoJSON`; the full topology family
   (sf/eh/rcc8); `geof:relate` with DE-9IM patterns; distance, area, buffer,
   getSRID and the constructive functions; the metric family
   (`geof:metricDistance`, `metricLength`, `metricPerimeter`, `metricArea`,
   `metricBuffer`) in metres on the WGS84 ellipsoid; `geof:transform` between the
   built-in CRSs (RD New, CRS84, EPSG:4326 in authority axis order, Web
   Mercator), and binary predicates harmonise their operands' CRSs.
   `geof:distance` and `geof:buffer` with a metre unit on a geographic CRS are
   geodesic; without a unit, or on a projected CRS, they stay planar in the CRS's
   units. The `geof:aggUnion` aggregate, with or without `GROUP BY`. **Not
   implemented:** KML/DGGS literals, the Query Rewrite Extension, the other
   aggregates (`aggBoundingBox`, `aggBoundingCircle`, `aggCentroid`,
   `aggConcaveHull`, `aggConvexHull`), and the non-metric functions GeoSPARQL 1.1
   added besides `transform` and `asGeoJSON` (`length`, `perimeter`, `centroid`,
   `geometryN`, `isEmpty`, `asWKT`, …).
6. **SHACL Core** — the Core constraint components are implemented, and
   blank-node property shapes (`sh:property [ … ]`, the standard idiom) are
   enforced (the loader dereferences blank nodes through the raw quad index;
   this applies to SHACL-on-write too). Graded *Partial* on the results of the
   W3C SHACL test suite's core section ([conformance/shacl.md](conformance/shacl.md)):
   one known failure remains, `core/property/uniqueLang-002` (storage reads
   `"1"^^xsd:boolean` back as `"true"`, so `sh:uniqueLang "1"` activates the
   constraint), and results are compared on `sh:conforms` and the violation
   focus nodes only, not on full result-set equality (constraint-component
   IRIs, `sh:resultPath`, `sh:value`).
7. **SHACL Advanced** — SPARQL-based targets (`sh:target` with `sh:select`),
   SPARQL constraints (`sh:sparql` with `sh:select`; `$this` is pre-bound via
   `VALUES` + `FROM <data-graph>`), custom constraint components
   (`sh:ConstraintComponent` / `sh:parameter` with `sh:ask` or `sh:select`
   validators), rules with `sh:condition` and `sh:order`, and
   `sh:qualifiedValueShape` counting work ([shacl.md](shacl.md)). **Not
   implemented:** the `$shapesGraph` and `$currentShape` pre-bindings — a
   constraint that uses them fails the shapes graph at load. The W3C corpus
   score compares `sh:conforms` and the focus-node multiset, not result
   component IRIs, paths or values.
8. **SHACL-C** is a pragmatic subset: `[min..max]` counts, `closed`, and `// "msg"`
   messages. The parser rejects unrecognized trailing input (it used to discard it
   silently, which could empty a shape graph on upload with a 200).
9. **RML / R2RML** — one row for the family (owner decision D1, 2026-10-03),
   graded on three corpora: the W3C R2RML test cases on SQLite, PostgreSQL
   and MySQL (`tests/w3c_r2rml_conformance.rs`, fetched at a pinned commit, no
   score published), the RML-Core test cases (`tests/rml_core_conformance.rs`)
   and the offline RML-IO source test cases (`tests/rml_io_conformance.rs`),
   both vendored; the CSV, JSON and XML cases of the legacy rml-test-cases run
   as well (`tests/rml_legacy_conformance.rs`). Mappings are read in R2RML,
   legacy RML and RML-Core / RML-IO, mixed freely ([rml.md](rml.md)). File
   sources: CSV with CSVW dialects, JSON and JSON Lines with RFC 9535 JSONPath,
   XML with XPath 1.0 and namespaces; `rml:encoding`, `rml:compression`
   (gzip, zip, tar.gz, tar.xz) and `rml:null`. Relational sources: `rr:tableName`,
   `rml:query`, R2RML's `rr:logicalTable` / `rr:sqlQuery` and RML-IO's
   `rml:SQL2008Table` / `rml:SQL2008Query` over registered PostgreSQL, MySQL /
   MariaDB and SQL Server datasources and virtual SPARQL sources
   ([sources.md](sources.md)). Multi-valued references generate a term per
   value, templates the cartesian product; language and datatype maps, join
   expression maps, `rml:URI` / `rml:UnsafeIRI` / `rml:UnsafeURI`, blank-node
   term maps without an expression, `rml:baseIRI`; joins on every source, and
   a join-less reference joins a row to itself (R2RML §8). Terms follow R2RML
   §7.3, §7.4, §10.2 and §11 (IRI-safe templates, natural datatypes and
   lexical forms, blank nodes per value and graph, union graph maps), a
   non-conforming mapping is refused at parse naming the construct, and a data
   error aborts the run naming the rows unless the run opts into skipping and
   reporting them. A datasource mapping version frozen before these rules keeps
   the old term rules. Cases that do not pass, each listed with its reason in
   its runner: R2RMLTC0002f (a regular SQL identifier is matched as the
   database reports its columns, not case-folded as SQL 2008 would), R2RMLTC0018a
   on SQLite and MySQL (neither returns `CHAR` values padded), RMLTC0027b (its
   expected output holds IRIs with spaces, which RDF does not allow) and
   RMLSTC0009a (the suite's manifest expects an error its own description and
   expected output do not); the RML-IO SPARQL-endpoint and D2RQ cases map
   through registered datasources and are not run.
   **Not implemented:** RML-FNML (new-vocabulary functions; the legacy
   `fnml:functionValue` with this store's own functions works), RML-CC
   (collections and containers), RML-LV (logical views), RML-star and RML-IO
   logical targets — a mapping that uses their terms is refused naming the
   module — and remote sources (never fetched; the file is given to the run).
10. **Zero-length property paths:** `:x :p* ?y` includes start nodes present in the
    data; the pure ALP edge of a *constant* start node absent from the graph is an
    oxigraph-evaluator divergence.
11. **OWL 2 RL / EL:** RL materialises the equality, property, class and schema
    rule families; the Table 8 datatype rules (`dt-type1/2`, `dt-eq`, `dt-diff`,
    `dt-not-type`) are not implemented. EL does not apply `owl:equivalentClass`
    or `owl:TransitiveProperty` — use RL where those matter. Both are pinned by
    `tests/owl2_rl_conformance.rs` / `tests/owl2_el_conformance.rs`.

Related guides: [OWL Reasoning](/docs/reasoning), [SHACL Validation](/docs/shacl),
[GeoSPARQL](/docs/geosparql), [Performance](/docs/performance),
[Triplestore comparison](/docs/triplestore-comparison),
[Authentication & API Tokens](/docs/auth).
