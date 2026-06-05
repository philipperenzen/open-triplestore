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
| SPARQL 1.1 Federated Query (`SERVICE`) | Remote query | **Disabled by design³** |
| SPARQL 1.1 Service Description | Capability advertisement | Full |
| SPARQL 1.2 (WD) | Triple terms, accessor functions | Partial¹ |
| RDFS | subClass/subProperty/domain/range inference | Full |
| OWL 2 QL / EL / RL | Profile reasoning (materialised) | Full |
| OWL 2 DL | Description-logic expressivity | Partial⁴ |
| GeoSPARQL 1.1 | Spatial RDF, relation/metric functions | Partial⁵ |
| SHACL Core | Structural constraint validation | Full⁶ |
| SHACL Advanced (AF / SPARQL) | SPARQL constraints, rules, targets | Full⁷ |
| SHACL-C | Compact-syntax parser/serializer | Full⁸ |
| LDP (Linked Data Platform) 1.0 | Basic/Direct/Indirect Containers; NonRDFSource | Full |
| DCAT 2 | Dataset catalogue description | Full |
| RML / R2RML | CSV/JSON/XML → RDF mapping | Full⁹ |
| JWT / OAuth 2.0 / OIDC / SAML 2.0 | Authentication | Full |
| ShEx / SWRL | Shape Expressions / Horn-clause rules | Full |

## Conformance test suites

Golden-standard conformance and high-complexity stress tests live in `tests/`.
Each suite encodes spec-verified expected results (adversarially fact-checked);
intentional non-conformances are encoded as documented, flip-when-fixed tests.

| Suite | Covers |
|---|---|
| `tests/w3c_sparql11_conformance.rs` | SPARQL 1.1 (incl. `cx01`–`cx15` high-complexity: MINUS antijoin, property paths, empty-group aggregates, subquery scope, WITH/USING, non-well-designed OPTIONAL, federation-disabled, dateTime type errors, COPY, FROM/FROM NAMED) |
| `tests/sparql12_conformance.rs` | RDF-star quoting semantics, accessor functions, opacity, named-graph isolation |
| `tests/geosparql_conformance.rs` | WKT, all sf/eh/rcc8 relations, CRS/axis order, getSRID, distance/area; geo-LD edge cases; documented function gaps |
| `tests/owl2_dl_conformance.rs` | DL extension rules + RL entailments (chains, inverse-functional, symmetric); documented tableau/profile gaps |
| `tests/shacl_conformance.rs` | Core components, languageIn, xone, not, deactivated, targetNode, qualified shapes; blank-node + sh:sparql gaps |
| `tests/shaclc_conformance.rs` | SHACL-C parse + round-trip (counts, closed, message) |
| `tests/rml_conformance.rs` | CSV/JSON/XML sources, template/reference/constant, datatype/language, class, dedup; join + inline-blank gaps |
| `tests/ldp_conformance.rs` | Container types, membership, NonRDFSource, ETag |

Run them in the Docker builder image (native build needs GEOS/pkg-config):

```bash
docker run --rm -v "$PWD:/app" -v ots_target:/app/target -w /app ots-builder \
  cargo test --all-features --locked --test '*conformance*'
```

## Known limitations & conformance findings

These were surfaced by the conformance suites above. Tracked tests pin current
behavior and will flip green when the limitation is resolved.

1. **RDF-star vs RDF 1.2.** The engine (oxigraph 0.4) implements the RDF-star CG
   model — quoted triples `<< s p o >>` usable in subject **and** object position.
   The newer RDF 1.2 / SPARQL 1.2 *triple-term* surface syntax `<<( s p o )>>`
   with `rdf:reifies` and `{| |}` annotations is **not** supported.
2. **Zero-length property paths.** `:x :p* ?y` does not yield a constant start node
   `:x` when `:x` is absent from the data (oxigraph behavior; the ALP algebra would
   include it). Tracked as an ignored test.
3. **Federation/`SERVICE` is intentionally disabled** as an SSRF mitigation
   (`without_service_handler()`); a `SERVICE` clause errors rather than reaching
   the network.
4. **OWL 2 DL** reasoning is RL-based forward-chaining plus DL-syntax extension
   rules (hasSelf, disjointUnion, NegativePropertyAssertion, hasKey, cardinality).
   Full DL tableau (consistency detection, profile validation, nominal/datatype
   reasoning) requires the external reasoner bridge (e.g. Konclude).
5. **GeoSPARQL 1.1:** WKT geometries; the full topology family (sf/eh/rcc8);
   `geof:relate` with DE-9IM patterns; distance, area, buffer, getSRID, and the
   constructive functions are supported. **Not yet implemented (feature gaps):**
   `geof:metricDistance` / `metricArea` (need a geodesic library), `geof:transform`
   (needs CRS reprojection / PROJ), `geof:aggUnion` (needs SPARQL aggregate support),
   and **GML** / **GeoJSON** geometry literals (WKT only). `geof:distance` is planar
   (CRS units), not geodetic.
6. **SHACL Core** — *fixed.* Blank-node property shapes (`sh:property [ … ]`, the
   standard idiom) are now enforced: the loader dereferences blank nodes through the
   raw quad index rather than via invalid `<_:bn>` SPARQL. Applies to SHACL-on-write
   too.
7. **SHACL Advanced** — *fixed.* `sh:qualifiedValueShape` counting is correct
   (class checks are scoped to the data graphs), and aggregate `sh:sparql` node
   constraints fire (`$this` is pre-bound via `VALUES` + `FROM <data-graph>` rather
   than textual IRI substitution).
8. **SHACL-C** is a pragmatic subset: `[min..max]` counts, `closed`, and `// "msg"`
   messages; the parser is lenient on unrecognized trailing input.
9. **RML** — *fixed.* Inline blank-node term maps (`rr:subjectMap [ … ]`, multiple
   `rr:predicateObjectMap [ … ]`) now parse correctly; `rr:class` is read from the
   subjectMap (R2RML); object `rr:constant` IRIs infer the IRI term type. Remaining
   gap: referencing object maps (`rr:parentTriplesMap` joins).
10. **Zero-length property paths:** `:x :p* ?y` includes start nodes present in the
    data; the pure ALP edge of a *constant* start node absent from the graph is an
    oxigraph-evaluator divergence.

Related guides: [OWL Reasoning](/docs/reasoning), [SHACL Validation](/docs/shacl),
[GeoSPARQL](/docs/geosparql), [Performance](/docs/performance),
[Triplestore comparison](/docs/triplestore-comparison),
[Authentication & API Tokens](/docs/auth).
