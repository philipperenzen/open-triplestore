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
| SPARQL 1.1 Query | SELECT, ASK, CONSTRUCT, DESCRIBE | Partial² |
| SPARQL 1.1 Update | INSERT, DELETE, LOAD, CLEAR, COPY, WITH/USING | Full |
| SPARQL 1.1 Graph Store HTTP | Named-graph CRUD over HTTP | Full |
| SPARQL 1.1 Federated Query (`SERVICE`) | Remote query | Partial³ — off by default; per-endpoint allowlist |
| SPARQL 1.1 Service Description | Capability advertisement | Full |
| SPARQL 1.2 (WD) | Triple terms, accessor functions | Partial¹ |
| RDFS | subClass/subProperty/domain/range inference | Full |
| OWL 2 QL | Query rewriting (`/api/reasoning/rewrite`); TBox closure materialised | Partial¹⁰ |
| OWL 2 EL | Profile reasoning (materialised) | Partial¹⁰ |
| OWL 2 RL | Profile reasoning (materialised) | Partial¹⁰ |
| OWL 2 DL | Description-logic expressivity | Partial⁴ |
| GeoSPARQL 1.1 | Spatial RDF, relation/metric functions | Partial⁵ |
| SHACL Core | Structural constraint validation | Partial⁶ |
| SHACL Advanced (AF / SPARQL) | SPARQL constraints, rules, targets | Partial⁷ |
| SHACL-C | Compact-syntax parser/serializer | Partial⁸ |
| OPM (Ontology for Property Management) | Property states with history | Partial — `opm:Property` / `opm:PropertyState` / current-outdated / reliability classes via the property-state API; no `opm:Calculation` or derived-property inference. See [datasets.md](datasets.md#time-evolving-properties-opm-profile). |
| buildingSMART IDS 1.0 | Information Delivery Specification → SHACL | Partial — entity, property, attribute, partOf facets with value restrictions and cardinality; classification and material facets target `props:ifcClassification` / `props:ifcMaterial`, which the IFC lift emits; predefinedType by convention only; dataset-level existence not enforced. See [shacl.md](shacl.md#importing-constraint-specifications-ids). Round-trips: export back to IDS 1.0 covers the shared subset, and anything outside the IDS facet model is reported as a loss. |
| ISO 21597-1 ICDD | Information container for linked document delivery | Partial — Part 1 containers import (documents, linksets, payload triples, ontology resources, index) and export (RDF/XML index); Part 2 not interpreted. See [containers.md](containers.md). |
| RDF Patch (RDF Delta) | Change log line format | Partial — version diffs served as patches; patches applied atomically per dataset (`H`, one `TX`/`TC`/`TA`, `PA`/`PD`, `A`/`D` quads). Not supported: more than one transaction block per patch (the format allows several), the spec's `PA`/`PD` forms (`PA "ex" "<iri>"`; this parser takes `PA ex: <iri>`, and a prefix abbreviates terms in the patch rather than changing the dataset's prefixes), deletes that name a blank node, `<_:label>` blank nodes, triples without a graph, and patch logs (`H prev`). See [versioning.md](versioning.md#rdf-patch). |
| LDES / TREE | Event streams of version objects; hypermedia fragmentation | Partial — time-ordered fixed-size fragments with `GreaterThanOrEqualToRelation`, frozen once full; entity-level version objects, tombstones; retention policies (`fullLogDuration`, `versionAmount`, `versionDuration`, `versionDeleteDuration`, `startingFrom`) enforced inside frozen pages with `410 Gone` for a compacted node; an incremental client that treats 410 as an empty page. No `tree:shape`, and the stream declares no `ldes:versionDeletePath`/`versionDeleteObject` for its tombstones. Spatial and substring fragmentations are optional TREE views and are not offered. Spec-derived rules in `tests/ldes_conformance.rs` (no external corpus exists). See [ldes.md](ldes.md). |
| LDP (Linked Data Platform) 1.0 | Basic/Direct/Indirect Containers; NonRDFSource; per-resource access control with Web Access Control (`.acl` resources, `acl:` vocabulary, `Link rel="acl"`, `WAC-Allow`) | Full — WAC agents are this store's principals; no WebID-TLS / Solid-OIDC, no `acl:origin`. See [ldp.md](ldp.md#access-control). |
| DCAT 3 / DCAT-AP 3 / DCAT-AP-NL 3 | Dataset catalogue description; EU / NL application profiles | Partial — DCAT 3 catalogue with VoID statistics; `DCAT_PROFILE` adds the AP/AP-NL mandatory properties (typed agents, identifiers, language, file types, data services, EU-authority statuses); no `dcat:CatalogRecord`, no temporal coverage, and the official DCAT-AP SHACL suite is not run in CI. See [dcat.md](dcat.md). |
| RML / R2RML | CSV/JSON/XML files and SQL / SPARQL datasources → RDF | Partial⁹ |
| JWT / OAuth 2.0 / OIDC | Authentication | Full |
| SAML 2.0 | Authentication | Experimental — not in the `full` feature or the published image; the ACS handler has a known request-ID validation defect, so no login can currently succeed. See [auth.md](auth.md). |
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
| SPARQL 1.1 Protocol / Graph Store | `tests/api_protocol_conformance.rs` | spec-derived | 17 |  |
| DCAT 3 / DCAT-AP 3 / VoID | `tests/dcat_conformance.rs` | spec-derived | 4 |  |
| GeoSPARQL 1.1 | `tests/geosparql_conformance.rs` | spec-derived | 130 |  |
| LDES 1.0 / TREE | `tests/ldes_conformance.rs` | spec-derived | 8 |  |
| LDP 1.0 (store level) | `tests/ldp_conformance.rs` | spec-derived | 43 |  |
| LDP 1.0 (HTTP) | `tests/ldp_http_conformance.rs` | spec-derived | 13 |  |
| OGC GeoSPARQL 1.1 validator shapes | `tests/ogc_geosparql_shacl_roundtrip.rs` | **vendored OGC corpus** (unmodified) | 2 |  |
| OWL 2 DL | `tests/owl2_dl_conformance.rs` | spec-derived | 34 |  |
| OWL 2 EL | `tests/owl2_el_conformance.rs` | spec-derived | 14 |  |
| OWL 2 QL | `tests/owl2_ql_conformance.rs` | spec-derived | 21 |  |
| OWL 2 RL | `tests/owl2_rl_conformance.rs` | spec-derived | 30 |  |
| RDF 1.1 formats | `tests/rdf11_conformance.rs` | spec-derived | 63 |  |
| RDFS entailment | `tests/rdfs_conformance.rs` | spec-derived | 23 |  |
| RML / R2RML | `tests/rml_conformance.rs` | spec-derived | 19 |  |
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
| SHACL Core | `tests/w3c_shacl_conformance.rs` | **vendored W3C corpus** (core + sparql sections, manifest-driven) | 1 | 136 corpus cases: 119 pass, 2 known failures, 15 runner-side skips (floor ≥90 asserted) |
| SPARQL 1.1 Query/Update | `tests/w3c_sparql11_conformance.rs` | spec-derived (+ cx01–cx15 high-complexity) | 125 |  |
| SPARQL 1.1 Query/Update | `tests/w3c_sparql11_manifests.rs` | **vendored W3C test-suite subset** (query + update sections of w3c/rdf-tests, unmodified; manifest-driven) | 1 | runs in CI as a development and regression ratchet; no score is published (W3C test-suite policy); known gaps in `docs/conformance/sparql11.md` |

760 conformance tests across 27 suites; a further 702 tests in 102 integration, security and regression suites under `tests/`, plus the crate's unit tests. Only the 3 **vendored** rows run a published corpus; every other suite is hand-written and derived from the specification text. A vendored row gives results only where its corpus licence allows performance claims; those are development and regression results on the vendored sections (`docs/conformance/`), not W3C, OGC or other conformance claims. The W3C SPARQL 1.1 sections are a subset of a W3C test suite, so they carry no results and are used for development and bug tracking only.

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
2. **SPARQL 1.1 Query** is graded *Partial*. The engine is oxigraph 0.5.11 with
   spareval 0.2.7, the latest release; the first two groups below sit there, the
   last two in this server.
   - **Entries of the W3C SPARQL 1.1 query sections that fail** (tracked, with no
     score, in [conformance/sparql11.md](conformance/sparql11.md)):
     - `bindings#graph`, `aggregates#agg-empty-group-count-graph` and
       `negation#graph-minus`: `GRAPH ?g` around a non-BGP inner pattern
       (`VALUES`, a sub-select, `MINUS`) loses the graph variable
       ([oxigraph#1905](https://github.com/oxigraph/oxigraph/issues/1905),
       fixed on oxigraph's main branch for its next major release; upstream
       ties `graph-minus` to the same cause);
     - `property-path#zero_or_more_set_start`, `…_set_end`,
       `zero_or_one_set_start` and `…_set_end`: a zero-length path (`*`, `?`)
       whose constant end is absent from the data yields no solution, where the
       spec's path semantics include the start term (`ASK { :x :p* :x }` is
       false on an empty graph);
     - `aggregates#agg-groupconcat-04` and `-06`: `GROUP_CONCAT` keeps a
       language tag that every input shares; SPARQL 1.1, and the SPARQL 1.2
       draft too, return a plain `xsd:string`;
     - `functions#bnode01`: `BNODE(str)` returns the same blank node for the
       same string in every solution (and across requests), and no node when
       the string is not a legal blank-node label.
   - **Several `FROM` graphs keep duplicates.** A triple present in two `FROM`
     graphs matches twice, which inflates rows, `COUNT` and `SUM`
     ([oxigraph#1919](https://github.com/oxigraph/oxigraph/issues/1919), fixed by
     [#1920](https://github.com/oxigraph/oxigraph/pull/1920) on main, not yet
     released). `/sparql` scopes every non-admin query with one `FROM` /
     `FROM NAMED` pair per readable graph, so a triple held in two readable
     graphs counts twice there; the columnar copy matches this on purpose.
   - **The HTTP dataset is rewritten.** `/sparql` intersects a caller's
     `FROM` / `FROM NAMED` graphs with the graphs the caller may read, and every
     graph it keeps becomes both a `FROM` and a `FROM NAMED` graph: `FROM <a>`
     alone also makes `<a>` a named graph, and the reverse. An admin's query gets
     every registered graph added to the dataset it names. The protocol's
     `default-graph-uri` / `named-graph-uri` parameters are ignored.
   - **Parallel shards and `EXISTS`.** An aggregate query can be split by
     subject across parallel shards (on by default). A `FILTER EXISTS` /
     `NOT EXISTS` inside it is then evaluated within one shard, so it sees only
     that shard's subjects.
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
   Full DL tableau reasoning (consistency detection, profile validation,
   nominal/datatype reasoning) needs an external reasoner; none ships with the
   project, and the bridge to one (`OTS_EXTERNAL_REASONER=konclude`) is
   experimental and not exercised in CI. See [owl2-dl.md](owl2-dl.md).
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
   this applies to SHACL-on-write too). Graded *Partial* on the results of the
   W3C SHACL test suite's core section ([conformance/shacl.md](conformance/shacl.md)):
   one known failure remains, `core/property/uniqueLang-002` (storage reads
   `"1"^^xsd:boolean` back as `"true"`, so `sh:uniqueLang "1"` activates the
   constraint), and results are compared on `sh:conforms` and the violation
   focus nodes only, not on full result-set equality (constraint-component
   IRIs, `sh:resultPath`, `sh:value`). The storage behaviour behind that failure
   reaches further: the store keeps XSD values natively, so a typed literal reads
   back in canonical form (`"01"^^xsd:integer` as `"1"`), every integer-derived
   datatype (`xsd:byte` … `xsd:nonNegativeInteger`) reads back as `xsd:integer`
   and `xsd:dateTimeStamp` as `xsd:dateTime`. `sh:datatype` compares datatype
   IRIs exactly, so validating stored data against
   `sh:datatype xsd:nonNegativeInteger` reports every stored
   `"5"^^xsd:nonNegativeInteger` as a violation, and an out-of-range
   `"300"^^xsd:byte` is stored as a valid integer that no later check can see.
   See [datatypes.md](datatypes.md).
7. **SHACL Advanced** — SPARQL-based targets (`sh:target` with `sh:select`),
   SPARQL constraints (`sh:sparql` with `sh:select`; `$this` is pre-bound),
   custom constraint components (`sh:ConstraintComponent` / `sh:parameter` with
   `sh:ask` or `sh:select` validators), `sh:qualifiedValueShape` counting, SPARQL
   and triple rules with `sh:condition` and `sh:order`, and `sh:SPARQLFunction`
   with a `sh:select` body work ([shacl.md](shacl.md)). **Gaps:**
   - SPARQL constraints and `sh:ask` validators skip blank-node focus nodes and
     values, so a violation on a blank node is not reported.
   - `?failure`, `sh:deactivated` on a SPARQL constraint or validator, `{?var}`
     message templates and `sh:sourceConstraint` are not handled, and a
     component parameter with several values uses the first.
   - `$shapesGraph` and `$currentShape` are not supported: a constraint that
     uses them fails the shapes graph at load, as SHACL §5.3.1 allows for these
     optional variables.
   - `sh:SPARQLFunction`: no `sh:ask` bodies; a body runs against an empty
     dataset, so it cannot read data; arguments are substituted as text.
   - No `sh:SPARQLTargetType` and no `sh:resultAnnotation`.
   - Node expressions: only a project-specific `sh:expression` form (a path plus
     comparison constraints) exists. A triple rule whose `sh:subject` or
     `sh:object` is a node expression such as `[ sh:path ex:p ]` writes that
     blank node from the shapes graph instead of evaluating it.

   The W3C corpus score compares `sh:conforms` and the focus-node multiset, not
   result component IRIs, paths or values, and no SHACL-AF test corpus is
   vendored yet.
8. **SHACL-C** is a pragmatic subset: `[min..max]` counts, `closed`, and `// "msg"`
   messages. The parser rejects unrecognized trailing input (it used to discard it
   silently, which could empty a shape graph on upload with a 200).
9. **RML / R2RML** — CSV/JSON/XML *file* sources with template/reference/constant
   term maps, datatype and language tags, `rr:class`, and inline blank-node term
   maps ([rml.md](rml.md)); relational logical sources (`rr:tableName`,
   `rml:query`, R2RML's `rr:logicalTable` / `rr:sqlQuery`) over registered
   PostgreSQL, MySQL / MariaDB and SQL Server datasources and virtual SPARQL
   sources, with referencing object maps (`rr:parentTriplesMap` joins) resolved
   there ([sources.md](sources.md)). **Not implemented:** joins on file sources
   (the mapping is refused), and more than one predicate map or object map per
   predicate-object map (the first of each is used).
10. **OWL 2 RL / EL / QL.**
    - **RL** runs the RL/RDF rules of Tables 4–9 except `eq-diff2` / `eq-diff3`
      (`owl:AllDifferent`), `prp-pdw` / `prp-adp` (property disjointness),
      `cls-nothing1` and `prp-ap`; `eq-ref`, `cls-thing`, `scm-op` and `scm-dp`
      are left out by design (`UNIMPLEMENTED_RULES` in
      `src/reasoning/owl2_rl.rs` gives each reason). Of Table 8, `dt-type1` and
      `dt-not-type` are implemented; `dt-type2`, `dt-eq` and `dt-diff` need
      literal subjects, which an RDF graph cannot hold. `dt-not-type` sees
      literals as stored, and storage turns derived datatypes into
      `xsd:integer` (footnote 6), so an out-of-range `"300"^^xsd:byte` is not
      caught. A run without a dataset or source graphs reads only the unnamed
      default graph while it writes to its target graph, so a rule whose
      premises are both derived does not fire (a three-hop transitive chain,
      `owl:equivalentProperty` feeding `prp-spo1`, a consistency check on a
      derived fact); dataset runs read the target graph too and are not
      affected.
    - **EL** has no rules for `owl:equivalentClass`, `owl:TransitiveProperty`,
      `rdfs:subPropertyOf` / `owl:equivalentProperty`, `owl:hasValue`,
      `owl:oneOf` or `owl:disjointWith`, and handles single-property keys only.
      Its CR3 rule is unsound: from `A ⊑ ∃p.B` and `B ⊑ C` it derives
      `∃p.C ⊑ A`, so an individual with a `p`-successor typed `C` can be typed
      `A`. Use RL where these matter.
    - **QL** rewrites a query over the class and property hierarchies, inverses
      and existential domains (`/api/reasoning/rewrite`); the `owl2-ql` regime
      materialises only the subclass and subproperty closure, so
      `?entailment=owl2-ql` adds no inferences about individuals. The rewriting
      is unsound in two places: `C ⊑ ∃P.D` is read as `∃P ⊑ C` (so any
      `x P y` makes `x` a `C`), and every existential alternative shares one
      fresh variable, which joins independent existentials and shows up in
      `SELECT *`. `rdfs:range` is not used, domains are not expanded through
      sub-classes or sub-properties, inverses are not composed with
      sub-properties, and negative inclusions, consistency checks and data
      properties are absent.

    See [owl2-rl.md](owl2-rl.md), [owl2-el.md](owl2-el.md) and
    [owl2-ql.md](owl2-ql.md).

Related guides: [OWL Reasoning](/docs/reasoning), [SHACL Validation](/docs/shacl),
[GeoSPARQL](/docs/geosparql), [Performance](/docs/performance),
[Triplestore comparison](/docs/triplestore-comparison),
[Authentication & API Tokens](/docs/auth).
