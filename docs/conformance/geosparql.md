# GeoSPARQL conformance

Two complementary layers, both in CI.

## 1. Functional coverage (in-house, OGC-requirement-mapped)

[`tests/geosparql_conformance.rs`](../../tests/geosparql_conformance.rs) — **101 tests**
mapping OGC GeoSPARQL 1.1 requirements 1–30: Simple Features / Egenhofer / RCC8 relation
families, constructive and metric functions, `geo:wktLiteral` + `geo:gmlLiteral` parsing,
`geof:getSRID`, and `geof:transform` (EPSG:28992 ↔ 4326 ↔ 3857, pure-Rust closed-form).

Tracked functional gaps (encoded as tests that flip when implemented):
`geof:metricDistance`/`geof:metricArea` (need geodesic math), `geof:aggUnion` (needs
SPARQL aggregate extension hooks), `geo:geoJSONLiteral` parsing.

## 2. Official OGC SHACL validator (vendored) — the round-trip

The OGC's own **GeoSPARQL 1.1 validator shapes** (54 shapes) and its **valid/invalid
example corpus** (48 files) are vendored under
[`tests/fixtures/ogc-geosparql/`](../../tests/fixtures/ogc-geosparql/PROVENANCE.md) and run
via [`tests/ogc_geosparql_shacl_roundtrip.rs`](../../tests/ogc_geosparql_shacl_roundtrip.rs)
— **using this repo's native SHACL engine**, which closes the loop the implementation
brief asked for: *GeoSPARQL data, validated by GeoSPARQL's own SHACL shapes, by our own
validator.*

### Scorecard (2026-06-11)

| | count |
|---|---|
| **Examples matching the OGC oracle** | **46** |
| Known deviations (ratcheted) | 2 |
| Total examples | 48 |
| **Waalbrug dataset round-trip** | **conforms ✓** |

Known deviations (same two-way ratchet as the W3C suite): two validator
`sh:sparql` subtleties. The two node-level lexical-form/datatype deviations were
fixed by the typed focus-node engine refactor (see `docs/conformance/shacl.md`).

### Why this suite mattered

Running the official validator surfaced the same **value-node semantics** bug as the W3C
suite (the validator leans heavily on `sh:or`-over-datatypes in property context — before
the fix, *every* geometry failed it), plus the node-level `sh:nodeKind sh:Literal` gap
(the validator targets serialization literals via `sh:targetObjectsOf`).
