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
| RDF 1.2 (CR 2026-04-07) | Triple terms, reifiers and annotations, base direction; N-Triples, N-Quads, Turtle, TriG, RDF/XML 1.2 | Partial¹ |
| SPARQL 1.1 Query | SELECT, ASK, CONSTRUCT, DESCRIBE | Full² |
| SPARQL 1.1 Update | INSERT, DELETE, LOAD, CLEAR, COPY, WITH/USING | Full |
| SPARQL 1.1 Protocol | Query and update over HTTP (`/sparql`) | Full² |
| SPARQL 1.1 Graph Store HTTP | Named-graph CRUD over HTTP | Full |
| SPARQL 1.1 Federated Query (`SERVICE`) | Remote query | Full³ — deny-by-default: off until endpoints are allowlisted |
| SPARQL 1.1 Service Description | Capability advertisement | Full |
| SPARQL 1.2 (WD 2026-10-01) | Triple terms and reified triples, `LANGDIR` family, `VERSION` | Partial¹ |
| RDFS | subClass/subProperty/domain/range inference | Full¹⁴ |
| OWL 2 QL | Profile reasoning (materialised) | Full¹⁰ |
| OWL 2 EL | Profile reasoning (materialised) | Full¹⁰ |
| OWL 2 RL | Profile reasoning (materialised) | Full¹⁰ |
| OWL 2 DL | Description-logic expressivity | Full⁴ (with the reasoner sidecar) |
| GeoSPARQL 1.0 | Spatial RDF, topology functions, RDFS entailment and query rewrite | Full⁵ |
| GeoSPARQL 1.1 | Spatial RDF, relation/metric functions | Full⁵ (all conformance classes but the optional DGGS class) |
| SHACL Core | Structural constraint validation | Full⁶ |
| SHACL Advanced (AF / SPARQL) | SPARQL constraints, rules, targets | Partial⁷ |
| SHACL-C | Compact-syntax parser/serializer (W3C CG report) | Full⁸ |
| OPM (Ontology for Property Management) | Property states with history, calculations | Full¹⁹ — `opm:Property` / `opm:PropertyState` / current-outdated, the reliability classes including `opm:Required`, `opm:documentation`, `opm:Deleted` with restore, `opm:Calculation` with derived states (POST / PUT / outdated), listings and snapshots, canonical OPM export/import and the OPM profile shapes. See [datasets.md](datasets.md#time-evolving-properties-opm-profile). |
| buildingSMART IDS 1.0 | Information Delivery Specification → SHACL | Partial — entity, property, attribute, partOf facets with value restrictions and cardinality; classification and material facets target `props:ifcClassification` / `props:ifcMaterial`, which the IFC lift emits; predefinedType by convention only; dataset-level existence not enforced. See [shacl.md](shacl.md#importing-constraint-specifications-ids). Round-trips: export back to IDS 1.0 covers the shared subset, and anything outside the IDS facet model is reported as a loss. |
| ISO 21597-1 ICDD | Information container for linked document delivery | Partial — Part 1 containers import (documents, linksets, payload triples, ontology resources, index) and export (RDF/XML index); Part 2 not interpreted. See [containers.md](containers.md). |
| RDF Patch (RDF Delta) | Change log line format; patch logs | Full — the format (`H`, several `TX`/`TC`/`TA` blocks, `PA`/`PD` in keyword or quoted form, `A`/`D` triples and quads) applied atomically per dataset, through the SHACL write gates; blank nodes (`_:x`, `<_:x>`) name the store's own nodes; `PA`/`PD` change the dataset's prefix table, which its Turtle/TriG exports declare; version diffs served as patches with prefix changes, chained through `H prev`. RDF Patch Logs per dataset: append with `H id`/`H prev` checking (409), `init`, `current`, `patch/{version\|id}`; version cuts are journaled, other writes are not. Extensions: prefixed names in `A`/`D` rows, `?graph=` for triples. RDF 1.2 triple terms are refused (RDF Patch has no syntax for them). Spec-derived rules in `tests/rdf_patch_conformance.rs`. See [versioning.md](versioning.md#rdf-patch). |
| LDES / TREE | Event streams of version objects; hypermedia fragmentation | Full — publisher: time-ordered fixed-size fragments under one root node with `GreaterThanOrEqualToRelation` / `LessThanOrEqualToRelation` bounds, frozen once full; monotonic timestamps; entity-level version objects with declared delete path and object; `tree:shape`, `ldes:pollingInterval`, ETag / `304`, `429` when busy, dereferenceable members; retention policies (`fullLogDuration`, `versionAmount`, `versionDuration`, `versionDeleteDuration`, `startingFrom`) enforced inside frozen pages with `410 Gone` for a compacted node. Client (unordered mode): §3.1 initialisation, allowlisted redirects, retries with back-off, every listed RDF format, §3.4 member extraction with named graphs, SHACL-path version semantics, a bookmark on `xsd:dateTime` values, immutable pages fetched once, legacy retention classes. Optional and not implemented: spatial and substring fragmentations, search forms, transactions, ordered mode, scheduled polling. Spec-derived rules in `tests/ldes_conformance.rs` (no external corpus exists). See [ldes.md](ldes.md). |
| LDP (Linked Data Platform) 1.0 | Basic/Direct/Indirect Containers; NonRDFSource; per-resource access control with Web Access Control (`.acl` resources, `acl:` vocabulary, `Link rel="acl"`, `WAC-Allow`) | Full — WAC agents are this store's principals; no WebID-TLS / Solid-OIDC, no `acl:origin`. See [ldp.md](ldp.md#access-control). |
| DCAT 3 / DCAT-AP 3 / DCAT-AP-NL 3 | Dataset catalogue description; EU / NL application profiles | Full¹⁷ — DCAT 3 catalogue: DCAT 3 §11 versions, data services with `dcat:servesDataset`, temporal coverage, update frequency, DCAT range typing; `DCAT_PROFILE` adds the AP/AP-NL properties (identifiers, language, file types, catalogue records). CI validates it against SEMIC's DCAT-AP 3.0.1 shapes and Geonovum's DCAT-AP-NL 3 shapes. See [dcat.md](dcat.md). |
| VoID | Dataset statistics, partitions and linksets | Full¹⁷ — per dataset, over the graphs the caller may read: statistics, class and property partitions, vocabularies, example resources, features, data dumps, SPARQL endpoint; linkset-role graphs as `void:Linkset`s; store-wide counts only, never partitions. See [dcat.md](dcat.md#void-statistics). |
| RML / R2RML | RML-Core / RML-IO, legacy RML and R2RML: CSV/JSON/XML files and SQL / SPARQL datasources → RDF | Partial⁹ |
| JWT / OAuth 2.0 / OIDC | Authentication | Full |
| SAML 2.0 | Authentication (this store as the service provider) | Full¹⁸ — Web Browser SSO started here (HTTP-Redirect AuthnRequest, optionally signed) or at the IdP (off by default, replay cache), HTTP-POST response; signatures SHA-256 or stronger; encrypted assertions; IdP metadata import with several signing certificates; SP key rollover; persistent NameID and attribute mapping; Single Logout in both directions. In the published image (`saml` feature), not in `full`. See [auth.md](auth.md#saml-20). |
| ShEx 2.1 | Shape Expressions: ShExC, ShExJ, ShExR, ShapeMap | Full¹⁶ — see [shex.md](shex.md). |
| SWRL | Horn-clause rules | Full¹⁵ — every atom (class expressions via the regime, data ranges natively), all §8 built-ins with binding, six syntaxes, rules stored with a dataset. See [swrl.md](swrl.md). |
| SPARQL + full-text search (Tantivy) | A feature, not a standard: the `ft:search` / `text:search` magic property and `CONTAINS` / `STRSTARTS` push-down | Full¹¹ |
| SKOS | Simple Knowledge Organization System: SKOS-aware inferencing and integrity checking | Full¹² |
| JSON-LD 1.1 | JSON-based RDF syntax: parsing (toRdf), serialisation (fromRdf), remote contexts | Partial¹³ |

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
[geosparql.md](conformance/geosparql.md), [sparql11.md](conformance/sparql11.md),
[sparql12.md](conformance/sparql12.md), [rdf12.md](conformance/rdf12.md) and
[jsonld.md](conformance/jsonld.md)). The SPARQL, RDF and JSON-LD API files, for
example, are subsets of W3C test suites, and W3C's test-suite licence policy
allows no performance claims on a subset, so their pages track known gaps but
give no score.
The SHACL Compact Syntax Community Group test cases (W3C Software and Document
License) are vendored under `tests/fixtures/w3c-shaclc/`, whose `PROVENANCE.md`
gives their source and licence. The shexTest results are in
[shex.md](shex.md#conformance).

<!-- conformance-table:start -->
| Standard | Suite | Basis | Tests | Notes |
|---|---|---|---:|---|
| SPARQL 1.1 Protocol / Graph Store | `tests/api_protocol_conformance.rs` | spec-derived | 27 |  |
| DCAT 3 / DCAT-AP 3 / VoID | `tests/dcat_conformance.rs` | spec-derived + **vendored SEMIC DCAT-AP 3.0.1 shapes** (unmodified; Geonovum DCAT-AP-NL 3 shapes fetched in CI) | 18 |  |
| GeoSPARQL 1.1 | `tests/geosparql_conformance.rs` | spec-derived | 192 |  |
| LDES 1.0 / TREE | `tests/ldes_conformance.rs` | spec-derived | 28 |  |
| LDP 1.0 (store level) | `tests/ldp_conformance.rs` | spec-derived | 43 |  |
| LDP 1.0 (HTTP) | `tests/ldp_http_conformance.rs` | spec-derived | 13 |  |
| OGC GeoSPARQL 1.1 validator shapes | `tests/ogc_geosparql_shacl_roundtrip.rs` | **vendored OGC corpus** (unmodified) | 2 |  |
| OWL 2 DL | `tests/owl2_dl_conformance.rs` | spec-derived (+ live tests against the reasoner sidecar) | 73 |  |
| OWL 2 EL | `tests/owl2_el_conformance.rs` | spec-derived | 57 |  |
| OWL 2 QL | `tests/owl2_ql_conformance.rs` | spec-derived | 45 |  |
| OWL 2 RL | `tests/owl2_rl_conformance.rs` | spec-derived | 73 |  |
| RDF 1.1 formats | `tests/rdf11_conformance.rs` | spec-derived | 69 |  |
| RDF Patch (RDF Delta) | `tests/rdf_patch_conformance.rs` | spec-derived | 24 |  |
| RDFS entailment | `tests/rdfs_conformance.rs` | spec-derived | 32 |  |
| RML / R2RML | `tests/rml_conformance.rs` | spec-derived | 50 |  |
| RML-Core | `tests/rml_core_conformance.rs` | **vendored KG-Construct CG corpus** (the RML-Core test cases, unmodified; manifest-driven) | 2 | 76 corpus cases: 75 pass, 1 known failure, 0 runner-side skips (floor ≥70 asserted) |
| RML-IO (sources) | `tests/rml_io_conformance.rs` | **vendored KG-Construct CG corpus** (the RML-IO source test cases, unmodified; manifest-driven) | 2 | 32 corpus cases: 29 pass, 1 known failure, 2 runner-side skips (floor ≥25 asserted) |
| RML (legacy vocabulary) | `tests/rml_legacy_conformance.rs` | **vendored RML.io corpus** (the CSV, JSON and XML cases of rml-test-cases, unmodified) | 2 | 117 corpus cases: 112 pass, 5 known failures, 0 runner-side skips (floor ≥100 asserted) |
| SHACL Advanced Features | `tests/shacl_af_corpus.rs` | **vendored TopQuadrant corpus** (expression, function, rule and target tests of TopQuadrant/shacl, unmodified; dash-driven, full report equality) | 1 | 10 corpus cases: 9 pass, 0 known failures, 1 expecting behaviour outside the spec (reported as the failure the spec requires), 0 runner-side skips (floor ≥9 asserted) |
| SHACL Core | `tests/shacl_conformance.rs` | spec-derived | 66 |  |
| SHACL-AF rules | `tests/shacl_rules_conformance.rs` | spec-derived | 43 |  |
| SHACL Compact Syntax | `tests/shaclc_conformance.rs` | spec-derived | 17 |  |
| ShEx | `tests/shex_conformance.rs` | spec-derived | 21 |  |
| ShEx 2.1 | `tests/shextest_conformance.rs` | **vendored shexTest corpus** (validation, representation, negative syntax/structure; manifest-driven) | 6 | 1917 corpus cases: 1795 pass, 0 known failures, 122 runner-side skips (floor ≥1795 asserted) |
| SPARQL 1.2 / RDF 1.2 | `tests/sparql12_conformance.rs` | spec-derived | 27 |  |
| SP2B / BSBM query shapes | `tests/sparql_benchmarks.rs` | benchmark-derived | 28 |  |
| SPARQL 1.1 functions | `tests/sparql_functions_conformance.rs` | spec-derived | 9 |  |
| SPARQL engine coverage (sparqloscope) | `tests/sparqloscope_conformance.rs` | sparqloscope-derived | 67 |  |
| Cross-standard HTTP smoke | `tests/standards_conformance.rs` | spec-derived | 28 |  |
| SWRL | `tests/swrl_conformance.rs` | spec-derived | 33 |  |
| JSON-LD 1.1 API | `tests/w3c_jsonld_api_manifests.rs` | **vendored W3C test-suite subset** (toRdf + fromRdf sections of w3c/json-ld-api, unmodified; manifest-driven) | 1 | runs in CI as a development and regression ratchet; no score is published (W3C test-suite policy); known gaps in `docs/conformance/jsonld.md` |
| OWL 2 DL | `tests/w3c_owl2_dl_manifests.rs` | **vendored W3C test cases** (approved OWL 2 DL / Direct Semantics cases of the OWL 2 Test Case Repository, unmodified; manifest-driven, against the reasoner sidecar) | 2 | runs in CI against the reasoner sidecar as a development and regression ratchet; no score is published (W3C licence: no performance claims on a partial run); known gaps in `docs/conformance/owl2-dl.md` |
| OWL 2 RL | `tests/w3c_owl2_rl_manifests.rs` | **vendored W3C test cases** (approved OWL 2 cases of the RL profile, unmodified; manifest-driven) | 2 | runs in CI as a development and regression ratchet; no score is published (W3C licence: no performance claims on a partial run); known gaps in `docs/conformance/owl2-rl.md` |
| R2RML | `tests/w3c_r2rml_conformance.rs` | **fetched W3C test cases** (pinned commit + sha256, not vendored; SQLite here, PostgreSQL and MySQL in the live-database job) | 5 | runs in CI as a development and regression ratchet over the cases fetched by `scripts/fetch-w3c-r2rml-tests.sh`; no score is published (W3C test-suite policy) |
| RDF 1.2 formats | `tests/w3c_rdf12_manifests.rs` | **vendored W3C test-suite subset** (N-Triples, N-Quads, Turtle, TriG, RDF/XML suites of `rdf/rdf12` + the `rdf/rdf11` suites they include, unmodified; manifest-driven) | 1 | runs in CI as a development and regression ratchet; no score is published (W3C test-suite policy); known gaps in `docs/conformance/rdf12.md` |
| RDF 1.1 Semantics (RDF/RDFS entailment) | `tests/w3c_rdf_mt_manifests.rs` | **vendored W3C test-suite subset** (rdf-mt section of w3c/rdf-tests, unmodified; manifest-driven) | 2 | runs in CI as a development and regression ratchet; no score is published (W3C test-suite policy); known gaps in `docs/conformance/entailment.md` |
| SHACL Core | `tests/w3c_shacl_conformance.rs` | **vendored W3C corpus** (core + sparql sections, manifest-driven, full report equality) | 1 | 136 corpus cases: 120 pass, 0 known failures, 1 optional feature unsupported (reported as the failure the spec requires), 15 runner-side skips (floor ≥90 asserted) |
| SHACL Compact Syntax | `tests/w3c_shaclc_conformance.rs` | **vendored W3C CG test cases** (SHACL-C report, line endings normalised; parse + round trip) | 2 | 32 corpus cases: 32 pass, 0 known failures, 0 runner-side skips (floor ≥32 asserted) |
| SPARQL 1.1 Query/Update | `tests/w3c_sparql11_conformance.rs` | spec-derived (+ cx01–cx15 high-complexity) | 126 |  |
| SPARQL 1.1 Entailment Regimes | `tests/w3c_sparql11_entailment_manifests.rs` | **vendored W3C test-suite subset** (entailment section of w3c/rdf-tests, unmodified; manifest-driven) | 2 | runs in CI as a development and regression ratchet; no score is published (W3C test-suite policy); known gaps in `docs/conformance/entailment.md` |
| SPARQL 1.1 Federated Query | `tests/w3c_sparql11_federation.rs` | **vendored W3C test-suite subset** (`service/` + `syntax-fed/` sections of w3c/rdf-tests, unmodified; manifest-driven, local endpoints) | 1 | runs in CI as a development and regression ratchet against local endpoints; no score is published (W3C test-suite policy); see `docs/conformance/sparql11.md` §Federation |
| SPARQL 1.1 Query/Update | `tests/w3c_sparql11_manifests.rs` | **vendored W3C test-suite subset** (query + update sections of w3c/rdf-tests, unmodified; manifest-driven) | 1 | runs in CI as a development and regression ratchet; no score is published (W3C test-suite policy); known gaps in `docs/conformance/sparql11.md` |
| SPARQL 1.2 | `tests/w3c_sparql12_manifests.rs` | **vendored W3C test-suite subset** (`sparql/sparql12` of w3c/rdf-tests, unmodified; manifest-driven, engine and mirror paths) | 1 | runs in CI as a development and regression ratchet; no score is published (W3C test-suite policy); known gaps in `docs/conformance/sparql12.md` |

1245 conformance tests across 43 suites; a further 830 tests in 107 integration, security and regression suites under `tests/`, plus the crate's unit tests. Only the 19 **vendored** rows run a published corpus; every other suite is hand-written and derived from the specification text. A vendored row gives results only where its corpus licence allows performance claims; those are development and regression results on the vendored sections (`docs/conformance/`, `docs/shex.md`, the RML corpora), not W3C, TopQuadrant, OGC or other conformance claims. The W3C SPARQL 1.1 sections (query, update and federation), the SPARQL 1.2 suite, the RDF 1.2 syntax suites, the JSON-LD API sections and the OWL 2 DL test cases are partial runs of W3C test suites, so they carry no results and are used for development and bug tracking only; the same holds for the W3C R2RML test cases, which CI fetches at a pinned commit rather than vendoring.

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

1. **RDF 1.2 and SPARQL 1.2.** The labels carry the date of the draft they
   are graded against: the RDF 1.2 Candidate Recommendations of 2026-04-07 and
   the SPARQL 1.2 Query Working Draft of 2026-10-01. The engine (oxigraph 0.5)
   implements the RDF 1.2 model, not the older RDF-star CG one: a *triple
   term* `<<( s p o )>>` in **object position only**, attached through
   `rdf:reifies`, plus `~ reifier` and `{| |}` annotation syntax.
   `<< s p o >>` is reifier shorthand — it mints a reifier and does not
   assert the base triple. The reifier is an ordinary IRI/blank node, so
   `isTRIPLE` is false for it and true for the triple term it points at.
   Code written against the RDF-star CG model (quoted triples usable in
   subject position) needs updating. The W3C `sparql/sparql12` suite and the
   N-Triples, N-Quads, Turtle, TriG and RDF/XML suites of `rdf/rdf12` (with
   the RDF 1.1 suites they include) run in CI as two-way ratchets, every
   SPARQL entry also through the in-memory mirror; they publish no score (a
   subset of a W3C test suite), and
   [conformance/sparql12.md](conformance/sparql12.md) and
   [conformance/rdf12.md](conformance/rdf12.md) list every entry that fails,
   with its cause. Every RDF 1.2 and RDF 1.1 syntax entry passes except
   those whose literal is a non-canonical number or an `rdf:XMLLiteral`, and
   triple terms and base direction survive the canonical and Skolem
   blank-node modes, version diffs, the commit log and the JSON term views. Both rows stay *Partial*
   because entries fail for reasons other than an open W3C Working Group
   issue (checked 2026-10-03; the issues the plan expected to block SPARQL
   1.2, w3c/sparql-query#282 and #283, closed on 2025-12-26):
   - **RDF 1.2:** storage keeps numeric literals as values (the oxigraph 0.5
     literal encoder), so `1.0`, `1e0`, `+1` or `01` read back in canonical
     form — 22 Turtle and TriG evaluation entries (20 of them from the RDF
     1.1 suites the RDF 1.2 manifests include). The lexical-form storage
     change fixes them, and with it the row becomes *Full*. Four
     `rdf:parseType="Literal"` entries wait on the open issue
     [w3c/rdf-xml#97](https://github.com/w3c/rdf-xml/issues/97) (opened
     2026-04-20) and do not count against the grade.
   - **SPARQL 1.2:** the row waits only on lexical-form storage. The one
     entry that fails, `grouping#group01`, needs numeric lexical forms kept
     (`"001"^^xsd:integer` must not group with `"1"`); the lexical-form
     storage change fixes it, and with it the row becomes *Full*. The parser
     and evaluator gaps the suite found are fixed in the vendored copy of
     spargebra and spareval ([vendor/README.md](../vendor/README.md)): a
     literal or triple term as the subject of a triple-term expression and an
     aggregate inside an aggregate are refused, an aggregating query's SELECT
     expression may reuse an earlier SELECT expression's variable, and `=`
     between two literals that both carry a base direction answers instead of
     panicking (a server error before).
   `LATERAL` (SEP-0006) and `ADJUST` (SEP-0002) are extensions oxigraph
   compiles in; they are not part of SPARQL 1.2 and are pinned separately.
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
5. **GeoSPARQL 1.1:** WKT, GML (a documented GML 3.2 profile, Z kept), GeoJSON
   (RFC 7946, CRS84, altitude kept) and KML (CRS84) geometry literals, with
   `geof:asWKT`, `asGML`, `asGeoJSON` and `asKML`, and geometry results in their
   first operand's serialisation and CRS; the full topology family
   (sf/eh/rcc8); `geof:relate` with DE-9IM patterns; distance, area, buffer,
   getSRID and the constructive functions; the metric family
   (`geof:metricDistance`, `metricLength`, `metricPerimeter`, `metricArea`,
   `metricBuffer`) in metres on the WGS84 ellipsoid; `geof:transform` between the
   built-in CRSs (RD New, CRS84, EPSG:4326 in authority axis order, Web
   Mercator), keeping Z and unbound outside a CRS's domain; binary functions,
   `geof:relate` included, harmonise their operands' CRSs. A literal's CRS is its
   WKT prefix or GML `srsName` for every function, axis order included, and an
   empty WKT/GML/GeoJSON/KML literal is the empty geometry. Units are OGC, QUDT or
   EPSG IRIs (or `xsd:anyURI`); `geof:distance` and `geof:buffer` with a linear
   unit on a geographic CRS are geodesic, on a projected CRS planar in its
   metres, and an unknown or incompatible unit is unbound. The rest of the
   GeoSPARQL 1.1 query functions (`length`, `perimeter`, `centroid`,
   `boundingCircle`, `concaveHull`, `geometryN`, `numGeometries`, `dimension`,
   `coordinateDimension`, `spatialDimension`, `is3D`, `isMeasured`, `isEmpty`,
   `isSimple`, `geometryType`, `minX` … `maxZ`) and all six spatial aggregates
   (`aggUnion`, `aggBoundingBox`, `aggBoundingCircle`, `aggCentroid`,
   `aggConvexHull`, `aggConcaveHull`), with or without `GROUP BY`. The RDFS Entailment Extension: a
   dataset whose graphs use GeoSPARQL terms reasons over the GeoSPARQL ontology
   and the Simple Features and GML 3.2.1 geometry class hierarchies. The Query
   Rewrite Extension: a triple pattern with one of the 24 relation predicates also
   matches where the geometries imply the relation, in queries and update `WHERE`
   clauses (on by default, `OTS_GEOSPARQL_QUERY_REWRITE=off`). **GeoSPARQL 1.0**
   meets every requirement of 11-052r4, per the requirement matrix in
   [conformance/geosparql.md](conformance/geosparql.md#3-requirement-matrix), and
   is graded Full. **GeoSPARQL 1.1** meets every requirement of 22-047r1 outside
   the DGGS conformance class, and is graded Full with that scope stated: DGGS
   (`/conf/geometry-extension-dggs`, Req 35–38 and the DGGS versions of the
   function and aggregate requirements) is a separate, optional conformance class
   of the standard, and DGGS literals and `geof:asDGGS` are not implemented.
   Both grades are the project's own, against the requirement matrix — not an
   OGC certification. Choices the standard leaves open: `getSRID` and
   `geometryType` return IRIs rather than `xsd:anyURI` literals;
   `geof:concaveHull` without a target, and `geof:aggConcaveHull` (which takes
   one argument, because the SPARQL parser allows one expression in a custom
   aggregate call), use the target 0.5 (see [geosparql.md](geosparql.md#aggregates)).
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
7. **SHACL Advanced** — implemented: SPARQL constraints (`sh:sparql`) and
   custom constraint components (`sh:parameter`, `sh:validator` /
   `sh:nodeValidator` / `sh:propertyValidator` with `sh:ask` or `sh:select`,
   optional parameters, one constraint per value of a single parameter);
   pre-binding of `$this`, `$value` and parameters as RDF terms, so blank-node
   focus and value nodes are checked (since 2026-10-02; they used to be
   skipped); `?failure`, `?message` and `{?var}` message templates;
   `sh:deactivated` on constraints and validators; custom targets
   (`sh:SPARQLTarget` and `sh:SPARQLTargetType`, parameters bound as terms);
   `sh:SPARQLFunction` (`sh:select` or `sh:ask` bodies, arguments bound as
   terms, bodies reading the run's data graphs, scoped to the declaring shapes
   graph); all seven node expressions of the 2017 Note; expression constraints
   (`sh:expression`, with the Note's semantics since 2026-10-02); and triple and
   SPARQL rules with `sh:condition` and `sh:order`; and result annotations
   (`sh:resultAnnotation`, since 2026-10-03, in the RDF report and the JSON)
   ([shacl.md](shacl.md)). Results name their constraint component IRI, and
   SPARQL and expression constraints' results their `sh:sourceConstraint`.
   **Optional, not supported:** `$shapesGraph` and `$currentShape` (SHACL
   §5.3.1). A constraint that uses them fails the shapes graph at load, which
   is the failure the spec requires of a processor without them; the W3C test
   `shapesGraph-001` is counted as "optional, unsupported", not as a pass or a
   known failure (w3c/data-shapes#426). **Not implemented:** `sh:entailment`
   (SHACL §1.5; SHACL-AF §8.3): a shapes graph that declares the `sh:Rules`
   regime is validated without running its rules first, and one that declares
   any regime gets neither that regime nor the failure the specs require of a
   processor without it. TopQuadrant's SHACL-AF tests: 9 of 10 cases pass at
   full report equality, and the tenth expects TopBraid behaviour outside the
   spec, where we report the failure the spec requires
   ([conformance/shacl.md](conformance/shacl.md)); the W3C corpus is compared
   at full result-set equality (all but `sh:resultMessage`).
8. **SHACL-C** — graded against the SHACL Community Group report
   [SHACL Compact Syntax](https://w3c.github.io/shacl/shacl-compact-syntax/)
   (not the SHACL 1.2 Compact Syntax draft). The parser implements the report's
   whole grammar and production rules and builds the RDF graph directly; all 32
   of the report's test cases (vendored under `tests/fixtures/w3c-shaclc/`, W3C
   Software and Document License) parse to the expected graph, and the
   serializer writes each expected graph back and parses it again to the same
   graph (`tests/w3c_shaclc_conformance.rs`, two-way ratchet). The serializer is
   lossless or loud: a shapes graph with triples the compact syntax cannot
   express is a `422` listing them (`?lossy=true` for the partial document).
   The proprietary dialect of 0.7 and earlier is accepted for one release behind
   `?dialect=legacy` (deprecated, logged); see
   [shacl.md](shacl.md#migrating-from-the-legacy-dialect). Updated 2026-10-03
   (was Partial: a different dialect, and a serializer that dropped most
   constraints silently).
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
   Graded *Partial* under the project's Full rubric (2026-10-03): R2RMLTC0002f
   is a deliberate deviation, which the rubric does not accept as a remaining
   failure; R2RMLTC0018a is a limit of the database engines, and RMLTC0027b and
   RMLSTC0009a are defects in the test suites.
   **Not implemented:** RML-FNML (new-vocabulary functions; the legacy
   `fnml:functionValue` with this store's own functions works), RML-CC
   (collections and containers), RML-LV (logical views), RML-star and RML-IO
   logical targets — a mapping that uses their terms is refused naming the
   module — and remote sources (never fetched; the file is given to the run).
10. **OWL 2 RL / EL / QL.**
    - **RL** runs all 78 RL/RDF rules (OWL 2 Profiles §4.3), lists of any
      length and inverse property expressions included. The Table 8 rules with
      literal subjects (`dt-type2`, `dt-eq`, `dt-diff`) are applied to data
      values through the 32-type RL datatype map, so `hasValue`, keys and
      negative data assertions match by value, data values type
      `someValuesFrom` subjects, and out-of-range or conflicting values are
      inconsistencies. `eq-ref` is off by default and on with `eq_ref: true`
      (decision D2); without it the closure lacks only the reflexive
      `owl:sameAs` triples. Not simulated: conclusions reached through a
      literal-subject triple when a data property is used as an object
      property, which OWL 2's typing rules out. Checked by a differential test
      against a generalized-triple reference evaluator and by the approved W3C
      OWL 2 RL-profile and SPARQL entailment-regime cases (no score
      published); every known failure there is outside Theorem PR1's scope
      ([conformance/owl2-rl.md](conformance/owl2-rl.md)).
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
11. **Full-text search** is graded as a feature: no W3C or OGC standard
    defines it. *Full* means the index covers every literal of the store and
    follows **every** write. Since 2026-10-03 each store write records what it
    touches (its exact quads, else its graphs) in the store's search journal,
    and the index catches up on the journal before it answers a text query —
    so LDP, RDF Patch, RML runs, SHACL rule output, entailment
    materialisation, replication, LDES sync and repair writes are searchable
    on the next query, as SPARQL Update, Graph Store and import writes already
    were. Only a write that cannot bound what it changed (a `CLEAR ALL`, a
    variable-graph update) costs a whole-index rebuild. Pinned by
    `tests/text_search_integration.rs`.

12. **SKOS** — a vocabulary, so *support* means SKOS-aware inferencing and
    integrity checking, not storage. The SKOS Reference's semantic conditions
    are, almost all, axioms of the SKOS RDF schema (inverses, symmetric and
    transitive properties, sub-properties, disjoint classes), and the `skos`
    dataset entailment regime materialises them: OWL 2 RL over the dataset
    with the bundled W3C schema as a premise, the schema's own closure pruned
    from the result ([reasoning.md](reasoning.md#the-skos-regime)). All seven
    integrity conditions — S9, S13, S14, S27, S36, S37 and S46 — are checked by
    the built-in *SKOS integrity* shape graph; S9 and S37 are also found by the
    regime as OWL 2 RL inconsistencies. Not checked: that the values of the
    labelling properties are plain literals (the range the reference gives
    them), which OWL 2 RL cannot express. Pinned by `tests/entailment_http.rs`
    and the shape tests in `src/shacl_studio/seed.rs`; there is no published
    SKOS test suite.

13. **JSON-LD 1.1** — every JSON-LD parse resolves a remote `@context`
    through the server's document loader (since 2026-10-03; until then any
    context named by IRI failed): the bundled W3C contexts of ActivityStreams,
    CSVW, LDP and ODRL offline, other URLs only when `OTS_REMOTE_ALLOWLIST`
    covers them, size-capped and cached ([formats.md](formats.md#json-ld-remote-contexts)).
    Graded *Partial* on the W3C json-ld-api `toRdf` and `fromRdf` sections,
    which run as a regression ratchet with no published score
    ([conformance/jsonld.md](conformance/jsonld.md)). Every `toRdf` entry
    the runner evaluates passes since the JSON-LD processor is patched in
    the vendored Oxigraph fork (`vendor/oxjsonld/`, 2026-10-03: base-IRI dot
    segments, an invalid `@base`, type-scoped contexts in type maps, the
    `rdfDirection` option). Three `fromRdf` entries fail on purpose, because
    the serialiser writes every stored quad as it is: it does not fold a
    list whose nodes carry `rdf:type rdf:List` into `@list` (the algorithm
    drops those quads) and does not refuse an `rdf:JSON` literal that is not
    valid JSON (the algorithm aborts the whole serialisation). A JSON-LD
    upload keeps `@direction` as an RDF 1.2 directional language-tagged
    string, where a JSON-LD 1.1 processor without `rdfDirection` drops it;
    the runner sets the option each test names. `expandContext`,
    `useNativeTypes` / `useRdfType`, generalized RDF and the JSON-LD 1.0
    processing mode are not offered.

14. **RDFS:** the RDF 1.1 Semantics patterns `rdfD2` and `rdfs1`–`rdfs13` with the RDF and RDFS
    axiomatic triples, in one fixed-point loop, and datatype clashes reported as
    inconsistencies ([RDFS Entailment](/docs/rdfs-entailment)). Exempt by decision D11, as the
    closure is infinite: the container-membership axioms are written for `rdf:_1` … `rdf:_n` up
    to the largest index the data uses, `rdfs1` declares the recognized datatypes that are in
    use, and `rdfD1` (a blank node per typed literal) is not materialised. Checked by
    `tests/rdfs_conformance.rs` and the W3C RDF 1.1 Semantics and SPARQL 1.1 entailment-regime
    cases (no score published; [conformance/entailment.md](conformance/entailment.md)).

15. **SWRL** is graded Full against the SWRL Member Submission: class, property,
    identity, data-range and built-in atoms; all 79 `swrlb:` built-ins of §8 with check,
    bind, split and enumerate modes, infinite binding patterns refused by name; lists as
    RDF lists with deterministic minted nodes for constructed ones; the §4 XML, §5 RDF,
    OWL/XML, functional and SWRLAPI syntaxes. Two things it depends on or leaves out:
    class-expression atoms are materialised by the entailment regime the rules run with
    (OWL 2 RL natively, beyond RL only with a DL backend), and rules are evaluated over
    the regime's materialisation, so conclusions that need reasoning by cases under
    disjunctive DL semantics are not derived. XPath-only regular-expression constructs
    (`\i`, `\c`, class subtraction) are refused. There is no W3C SWRL test suite; the
    semantics are pinned by `tests/swrl_conformance.rs` (one table-driven test per §8
    subsection) and `tests/entailment_http.rs`.

16. **ShEx 2.1** — graded against the ShEx 2.1 Final Community Group Report
    (2019-10-08). The engine passes the whole 2.1 part of the vendored
    shexTest suite (`tests/shextest_conformance.rs`): 1209 validation, 462
    representation (ShExC = ShExJ = ShExR), 105 negative-syntax and 19
    negative-structure tests, no known failures. Skipped: the 122 tests of
    ShEx 2.next (`EXTENDS`, `ABSTRACT`), which 2.1 does not include and the
    engine refuses. Where the specification leaves a choice to the
    implementation: `IMPORT` is resolved only from named graphs of the store
    the caller may read (ShExR), never over the network; semantic actions
    evaluate only the shexTest Test extension and never execute code;
    `EXTERNAL` shapes have no definition mechanism, so they are never
    satisfied; ShapeMap `SPARQL` selectors are not supported. Stored data: the
    store keeps typed literals as written, so every validation case answers
    the same through the store as on the parsed data (`STORE_DIVERGENCES` is
    empty).

17. **DCAT 3 / DCAT-AP / DCAT-AP-NL and VoID.** `tests/dcat_conformance.rs` builds the
    catalogue per profile over a fixture that takes every branch of the generator
    (organisation-, user- and group-owned datasets, geometry, an LDES stream, released and
    draft versions, a linkset and a private graph) and asserts no violation of SEMIC's
    DCAT-AP 3.0.1 shapes (`dcat-ap-SHACL.ttl` + `ranges.ttl`, vendored unmodified, CC BY
    4.0) and of Geonovum's DCAT-AP-NL 3 shapes (fetched at a pinned commit and
    sha256-checked in CI; the repository has no licence file, so they are not vendored).
    The shapes run on this repo's SHACL engine, itself graded Full for SHACL Core (6); the SEMIC
    files link five property shapes they never define, which the runner drops before
    validating (the engine refuses an ill-formed shapes graph whole). DCAT-AP-NL 3.0.1
    is still a working version upstream. What a profile requires and a registry entry does
    not hold — a theme, a contact point, a licence — is logged as a profile warning and
    never invented. `dcat:DatasetSeries` is not applicable: the product has no series
    concept. VoID's optional `void:uriLookupEndpoint` is not offered (there is no RDF
    lookup endpoint for arbitrary IRIs), and partitions are skipped on datasets larger than
    `OTS_VOID_PARTITION_MAX_TRIPLES`.

18. **SAML 2.0** is graded against the web-browser SSO profile (Profiles §4.1)
    and Single Logout (§4.4) for this store as the service provider, following
    the Kantara saml2int deployment profile: HTTP-Redirect requests, HTTP-POST
    responses, signed responses (RSA/ECDSA with SHA-256 or stronger only),
    encrypted assertions (AES-GCM/CBC with RSA-OAEP; RSA PKCS#1 v1.5 refused),
    our own entity ID, ACS, SLO endpoint, signing and encryption keys, NameID
    format and contact in the SP metadata. Out of scope: the Artifact binding,
    ECP, attribute queries, NameID management, federation metadata aggregates
    and MDQ. Single Logout revokes refresh tokens only; access tokens issued
    before it stay valid until they expire. Verified against a fake IdP in
    `tests/security_federated.rs` (signatures, audience, recipient, destination,
    expiry, replay, DOCTYPE, encryption with key rollover, both logout
    directions), not against a named IdP product in CI.

19. **OPM** is a 2018 W3C Linked Building Data community-group draft with no
    test suite, so *Full* is this project's own assessment against the
    specification text (classes, properties and the REST guidance for
    calculations), pinned by `tests/property_states_http.rs`. Two documented
    choices: property nodes are linked with `ots:propertyOf` /
    `ots:propertyPredicate` in storage (the data graph keeps the plain value;
    export and import use OPM's canonical `<item> <kind> <property>`, and every
    read accepts it), and calculations run only on an explicit POST or PUT, as
    the specification describes, never after a write. Arithmetic is on numeric
    XSD values; argument paths and expressions are restricted to an
    allow-listed subset of SPARQL (no `SERVICE`, `GRAPH`, `EXISTS`, subqueries
    or non-deterministic functions).

Related guides: [OWL Reasoning](/docs/reasoning), [SHACL Validation](/docs/shacl),
[ShEx Validation](/docs/shex),
[GeoSPARQL](/docs/geosparql), [Performance](/docs/performance),
[Triplestore comparison](/docs/triplestore-comparison),
[Authentication & API Tokens](/docs/auth).
