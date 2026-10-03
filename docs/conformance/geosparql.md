# GeoSPARQL — test results

Two complementary layers, both in CI.

## 1. Functional coverage (in-house, spec-derived)

[`tests/geosparql_conformance.rs`](../../tests/geosparql_conformance.rs) — hand-written
tests derived from the OGC GeoSPARQL 1.1 standard (count in the
[generated table](../standards.md#conformance-test-suites)), grouped by the test file's
own 30-item requirement list, not the OGC conformance classes: Simple Features /
Egenhofer / RCC8 relation families, constructive and metric functions, `geo:wktLiteral`,
`geo:gmlLiteral` (the documented GML profile), `geo:geoJSONLiteral` (every RFC 7946
geometry type, malformed input unbound rather than a panic) and `geo:kmlLiteral` parsing
with `geof:asWKT`, `geof:asGML`, `geof:asGeoJSON` and `geof:asKML` round trips, `geof:getSRID`, and
`geof:transform` (EPSG:28992 ↔ 4326 ↔ 3857, pure-Rust closed-form). The GeoSPARQL 1.1
metric functions (`metricDistance`, `metricLength`, `metricPerimeter`, `metricArea`,
`metricBuffer`) are checked against published WGS84 geodesic values — GeographicLib's
JFK–LHR (5 551 759.400 m) and nearly antipodal Wellington–Salamanca (19 959 679.267 m),
the equatorial degree and the meridian arc — and the `uom:` units of `geof:distance` /
`geof:buffer` on geographic and projected CRSs. The `geof:aggUnion` aggregate is tested
for overlapping polygons (area), duplicates, `GROUP BY`/`HAVING`, the empty group, mixed
WKT/GML/GeoJSON and mixed-CRS groups, SPARQL error semantics, and on every path a query
can take — the subject shards and the columnar copy (both decline it), the full
in-memory copy, the engine, the result cache, scoped queries and updates — with
byte-identical answers across them; `tests/standards_conformance.rs` runs it over HTTP.

No functional gap is tracked in this suite any more. What GeoSPARQL 1.1 still lacks
here — DGGS literals, the Query Rewrite Extension, the other aggregates and several
non-metric functions — is listed in [standards.md](../standards.md#known-limitations--conformance-findings).

### Requirement matrix (OGC 22-047r1)

Requirements of GeoSPARQL 1.1 and the tests in `tests/geosparql_conformance.rs` that
check them. The tests are named by requirement: `ogc_req24_…` checks Req 24. This is the
project's own mapping, not an OGC test result.

| Req | ID | Tests |
|---|---|---|
| 19 | `/req/geometry-extension/asWKT-function` | `ogc_req19_as_wkt_keeps_the_srs_and_z` |
| 20 | `/req/geometry-extension/gml-literal` | `ogc_req20_gml_profile_elements`, `ogc_req20_gml_outside_the_profile_is_unbound`, `ogc_req20_gml_srs_dimension_3_keeps_z`, `ogc_req20_gml_srs_name_is_harmonised_against_wkt` |
| 22 | `/req/geometry-extension/gml-profile` | `ogc_req22_gml_profile_is_documented` (the profile: [geosparql.md](../geosparql.md#supported-gml-profile)) |
| 24 | `/req/geometry-extension/asGML-function` | `ogc_req24_as_gml_round_trips` |
| 25 | `/req/geometry-extension/geojson-literal` | `ogc_req25_geojson_literal_keeps_altitude`, `geojson_multi_geometries_and_collections`, `malformed_geojson_literal_is_unbound_not_a_panic` |
| 30 | `/req/geometry-extension/kml-literal` | `ogc_req30_kml_literal_is_a_geometry` |
| 31 | `/req/geometry-extension/kml-literal-default-srs` | `ogc_req31_kml_literal_is_crs84` |
| 32 | `/req/geometry-extension/kml-literal-empty` | `ogc_req32_kml_literal_empty_is_the_empty_geometry` |
| 33 | `/req/geometry-extension/geometry-as-kml-literal` | `ogc_req33_geometry_as_kml_literal_is_queryable` |
| 34 | `/req/geometry-extension/asKML-function` | `ogc_req34_as_kml_reprojects_and_round_trips` |
| 39 | `/req/geometry-extension/query-functions` (§10.9.1 result serialisation) | `ogc_req39_geometry_results_follow_the_first_operand` |

## 2. OGC GeoSPARQL 1.1 SHACL validator (vendored) — the round-trip

The OGC's own **GeoSPARQL 1.1 validator shapes** (54 shapes; informative, not normative,
as of GeoSPARQL 1.1) and its **valid/invalid example corpus** (48 files) are vendored
unmodified under
[`tests/fixtures/ogc-geosparql/`](../../tests/fixtures/ogc-geosparql/PROVENANCE.md) and run
via [`tests/ogc_geosparql_shacl_roundtrip.rs`](../../tests/ogc_geosparql_shacl_roundtrip.rs)
— **using this repo's native SHACL engine**, which closes the loop:
*GeoSPARQL data, validated by GeoSPARQL's own SHACL shapes, by our own
validator.*

These are development results, not an OGC compliance certification — only the OGC can
authorise compliance marks for its standards. The files are © Open Geospatial Consortium
and redistributed under the Apache License 2.0 — see
[`LICENSE.md`](../../tests/fixtures/ogc-geosparql/LICENSE.md) there.

### Results (2026-06-11)

| | count |
|---|---|
| **Examples matching the OGC oracle** | **46** |
| Known deviations (ratcheted) | 2 |
| Total examples | 48 |
| **Reference-example round-trip** ([`example-bridge`](../../tests/fixtures/example-bridge/)) | **conforms ✓** |

Known deviations (same two-way ratchet as the W3C suite): two validator
`sh:sparql` subtleties. The two node-level lexical-form/datatype deviations were
fixed by the typed focus-node engine refactor (see `docs/conformance/shacl.md`).

### Why this suite mattered

Running the OGC validator surfaced the same **value-node semantics** bug as the W3C
suite (the validator leans heavily on `sh:or`-over-datatypes in property context — before
the fix, *every* geometry failed it), plus the node-level `sh:nodeKind sh:Literal` gap
(the validator targets serialization literals via `sh:targetObjectsOf`).
