# Supported Standards

The following W3C and OGC standards are implemented. Support levels reflect a
golden-standard conformance pass (engine + high-complexity edge cases):

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
| SHACL Core | Structural constraint validation | Full⁶ |
| SHACL Advanced (AF / SPARQL) | SPARQL constraints, rules, targets | Partial⁷ |
| SHACL-C | Compact-syntax parser/serializer | Partial⁸ |
| OPM (Ontology for Property Management) | Property states with history | Partial — `opm:Property` / `opm:PropertyState` / current-outdated / reliability classes via the property-state API; no `opm:Calculation` or derived-property inference. See [datasets.md](datasets.md#time-evolving-properties-opm-profile). |
| buildingSMART IDS 1.0 | Information Delivery Specification → SHACL | Partial — entity, property, attribute, partOf facets with value restrictions and cardinality; classification/material/predefinedType by convention only; dataset-level existence not enforced. See [shacl.md](shacl.md#importing-constraint-specifications-ids). |
| LDES / TREE | Event streams of version objects; hypermedia fragmentation | Partial — time-ordered fixed-size fragments with `GreaterThanOrEqualToRelation`, entity-level version objects, tombstones, an incremental client; no retention policies, `tree:shape` or spatial/substring fragmentations. See [ldes.md](ldes.md). |
| LDP (Linked Data Platform) 1.0 | Basic/Direct/Indirect Containers; NonRDFSource | Full |
| DCAT 3 / DCAT-AP 3 / DCAT-AP-NL 3 | Dataset catalogue description; EU / NL application profiles | Partial — DCAT 3 catalogue with VoID statistics; `DCAT_PROFILE` adds the AP/AP-NL mandatory properties (typed agents, identifiers, language, file types, data services, EU-authority statuses); no `dcat:CatalogRecord`, no temporal coverage, and the official DCAT-AP SHACL suite is not run in CI. See [dcat.md](dcat.md). |
| RML / R2RML | CSV/JSON/XML → RDF mapping | Partial⁹ |
| JWT / OAuth 2.0 / OIDC | Authentication | Full |
| SAML 2.0 | Authentication | Experimental — not in the `full` feature or the published image; the ACS handler has a known request-ID validation defect, so no login can currently succeed. See [auth.md](auth.md). |
| ShEx | Shape Expressions (ShExC) | Partial — node kinds, datatypes with lexical checks, string/numeric facets, value sets, cardinalities, EachOf/OneOf, inverse constraints, CLOSED/EXTRA, shape references; no semantic actions, imports or annotations. Semantics pinned by `tests/shex_conformance.rs`. |
| SWRL | Horn-clause rules | Partial — class/property atoms and the built-ins in `src/swrl`; an unsupported built-in is a hard error rather than a silently dropped filter. Semantics pinned by `tests/swrl_conformance.rs`. |

## Conformance test suites

Conformance and high-complexity stress tests live in `tests/`. Each suite encodes
expected results taken from the specification text; intentional non-conformances
are encoded as documented, flip-when-fixed tests. Two things the table makes
explicit: only the **vendored** rows run a published test corpus (the W3C SHACL
Core manifests and the OGC GeoSPARQL validator shapes) — every other suite is
hand-written and *derived from* its spec, not the W3C/OGC corpus — and the counts
are generated from the suites themselves, so they cannot drift from the code.

<!-- conformance-table:start -->
| Standard | Suite | Basis | Tests | Notes |
|---|---|---|---:|---|
| SPARQL 1.1 Protocol / Graph Store | `tests/api_protocol_conformance.rs` | spec-derived | 14 |  |
| DCAT 2 / VoID | `tests/dcat_conformance.rs` | spec-derived | 4 |  |
| GeoSPARQL 1.1 | `tests/geosparql_conformance.rs` | spec-derived | 107 |  |
| LDP 1.0 (store level) | `tests/ldp_conformance.rs` | spec-derived | 43 |  |
| LDP 1.0 (HTTP) | `tests/ldp_http_conformance.rs` | spec-derived | 13 |  |
| OGC GeoSPARQL 1.1 validator shapes | `tests/ogc_geosparql_shacl_roundtrip.rs` | **vendored OGC corpus** | 2 |  |
| OWL 2 DL extension rules | `tests/owl2_dl_conformance.rs` | spec-derived | 34 |  |
| OWL 2 EL | `tests/owl2_el_conformance.rs` | spec-derived | 14 |  |
| OWL 2 QL | `tests/owl2_ql_conformance.rs` | spec-derived | 21 |  |
| OWL 2 RL | `tests/owl2_rl_conformance.rs` | spec-derived | 23 |  |
| RDF 1.1 formats | `tests/rdf11_conformance.rs` | spec-derived | 63 |  |
| RDFS entailment | `tests/rdfs_conformance.rs` | spec-derived | 23 |  |
| RML / R2RML | `tests/rml_conformance.rs` | spec-derived | 18 |  |
| SHACL Core | `tests/shacl_conformance.rs` | spec-derived | 9 |  |
| SHACL-AF rules | `tests/shacl_rules_conformance.rs` | spec-derived | 16 |  |
| SHACL Compact Syntax | `tests/shaclc_conformance.rs` | spec-derived | 8 |  |
| ShEx | `tests/shex_conformance.rs` | spec-derived | 10 |  |
| SPARQL 1.2 / RDF-star | `tests/sparql12_conformance.rs` | spec-derived | 14 |  |
| SP2B / BSBM query shapes | `tests/sparql_benchmarks.rs` | benchmark-derived | 28 |  |
| SPARQL 1.1 functions | `tests/sparql_functions_conformance.rs` | spec-derived | 9 |  |
| SPARQL engine coverage (sparqloscope) | `tests/sparqloscope_conformance.rs` | sparqloscope-derived | 67 |  |
| Cross-standard HTTP smoke | `tests/standards_conformance.rs` | spec-derived | 25 |  |
| SWRL | `tests/swrl_conformance.rs` | spec-derived | 3 |  |
| SHACL Core | `tests/w3c_shacl_conformance.rs` | **vendored W3C corpus** (manifest-driven) | 1 | 113 corpus cases: 97 pass, 1 known failure, 15 runner-side skips (floor ≥90 asserted) |
| SPARQL 1.1 Query/Update | `tests/w3c_sparql11_conformance.rs` | spec-derived (+ cx01–cx15 high-complexity) | 125 |  |

694 conformance tests across 25 suites; a further 350 tests in 36 integration, security and regression suites under `tests/`, plus the crate's unit tests. Only the two **vendored** rows run a published corpus; every other suite is hand-written and derived from the specification text.

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
5. **GeoSPARQL 1.1:** WKT and GML geometry literals; the full topology family
   (sf/eh/rcc8); `geof:relate` with DE-9IM patterns; distance, area, buffer,
   getSRID and the constructive functions; `geof:transform` between the built-in
   CRSs (RD New, CRS84, EPSG:4326 in authority axis order, Web Mercator), and
   binary predicates harmonise their operands' CRSs. **Not implemented:** the
   geodesic *metric* family (`geof:metricDistance`, `metricArea`, …),
   `geof:aggUnion`, GeoJSON/KML/DGGS literals, and the Query Rewrite Extension.
   `geof:distance` is planar (CRS units), not geodetic.
6. **SHACL Core** — *fixed.* Blank-node property shapes (`sh:property [ … ]`, the
   standard idiom) are now enforced: the loader dereferences blank nodes through the
   raw quad index rather than via invalid `<_:bn>` SPARQL. Applies to SHACL-on-write
   too.
7. **SHACL Advanced** — SPARQL-based targets (`sh:target` with `sh:select`),
   SPARQL constraints (`sh:sparql` with `sh:select`; `$this` is pre-bound via
   `VALUES` + `FROM <data-graph>`), rules, and `sh:qualifiedValueShape` counting
   work. **Not implemented:** custom constraint components
   (`sh:ConstraintComponent` / `sh:parameter` validators), `sh:ask`-based
   constraints, and rule `sh:condition` / `sh:order`. The W3C corpus score
   compares `sh:conforms` and the focus-node multiset, not result component
   IRIs, paths or values.
8. **SHACL-C** is a pragmatic subset: `[min..max]` counts, `closed`, and `// "msg"`
   messages. The parser rejects unrecognized trailing input (it used to discard it
   silently, which could empty a shape graph on upload with a 200).
9. **RML / R2RML** — CSV/JSON/XML *file* sources with template/reference/constant
   term maps, datatype and language tags, `rr:class`, and inline blank-node term
   maps. **Not implemented:** SQL logical tables (`rr:logicalTable`,
   `rr:sqlQuery`) and referencing object maps (`rr:parentTriplesMap` joins); a
   predicate-object map honours its first predicate map and first object map
   only.
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
