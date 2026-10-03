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

The RDFS Entailment Extension (the GeoSPARQL ontology with the Simple Features and GML
3.2.1 class hierarchies as premises) and the Query Rewrite Extension (all 24 relations,
every rule shape and serialisation, set semantics, `GRAPH` scope, update `WHERE` clauses,
every query path, the switch) are tested too; `tests/standards_conformance.rs` runs both
over HTTP. The other GeoSPARQL 1.1 query functions (geometry properties, `centroid`,
`boundingCircle`, `concaveHull`, `length`, `perimeter`, `geometryN`, the min/max ordinates)
are tested on WKT, GML, GeoJSON and KML operands, and the five other aggregates with
`aggUnion`'s rules (empty group, non-geometry, order independence, CRS, query paths). What
GeoSPARQL 1.1 still lacks here is the optional DGGS conformance class; section 3 below
maps every requirement to its tests.

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

## 3. Requirement matrix

Every requirement of GeoSPARQL 1.0 (OGC 11-052r4) and 1.1 (OGC 22-047r1), its status in
this build and the tests in [`tests/geosparql_conformance.rs`](../../tests/geosparql_conformance.rs)
that pin it (`standards_conformance.rs` for the HTTP smoke tests). Status is the project's
own reading, not an OGC test result: **Met** — implemented and tested; **Partial** — the
note says what is missing; **Missing**. A requirement any SPARQL store satisfies (a
vocabulary term usable in a graph pattern) is Met once a test uses it. Matrix as of
2026-10-03.

### GeoSPARQL 1.0 (11-052r4)

The requirement IDs are `http://www.opengis.net/spec/geosparql/1.0/req/…`; the R-numbers
are the standard's own order.

| R | Requirement | Status | Tests |
|---|---|---|---|
| R1 | core/sparql-protocol | Met | SPARQL 1.1 Query/Protocol suites ([standards.md](../standards.md)) |
| R2 | core/spatial-object-class | Met | `ogc10_r02_r03_spatial_object_and_feature_classes` |
| R3 | core/feature-class | Met | `ogc10_r02_r03_spatial_object_and_feature_classes` |
| R4 | topology-vocab-extension/sf-spatial-relations | Met | `ogc10_r04_r05_r06_topology_vocabulary_properties`, `query_rewrite_req50_sf_relations` |
| R5 | topology-vocab-extension/eh-spatial-relations | Met | `ogc10_r04_r05_r06_topology_vocabulary_properties`, `query_rewrite_req51_eh_relations` |
| R6 | topology-vocab-extension/rcc8-spatial-relations | Met | `ogc10_r04_r05_r06_topology_vocabulary_properties`, `query_rewrite_req52_rcc8_relations` |
| R7 | geometry-extension/geometry-class | Met | `ogc10_r07_r08_r09_geometry_class_and_properties` |
| R8 | geometry-extension/feature-properties | Met | `ogc10_r07_r08_r09_geometry_class_and_properties`, `geo_data_model_*` |
| R9 | geometry-extension/geometry-properties | Met | `ogc10_r07_r08_r09_geometry_class_and_properties` |
| R10 | geometry-extension/wkt-literal | Met | `geo_req01_wkt_literal_*` |
| R11 | geometry-extension/wkt-literal-default-srs | Met | `geo_req30_get_srid_default_crs84`, `geos_cx_axis_order_crs84_within` |
| R12 | geometry-extension/wkt-axis-order | Met | `geos_cx_axis_order_crs84_within`, `ogc_req20_gml_srs_name_is_harmonised_against_wkt` |
| R13 | geometry-extension/wkt-literal-empty | Met | `ogc_req17_wkt_literal_empty_is_the_empty_geometry`, `geo_req01_wkt_empty_geometry` |
| R14 | geometry-extension/geometry-as-wkt-literal | Met | `geo_data_model_feature_geometry_pattern` |
| R15 | geometry-extension/gml-literal | Met — the documented GML profile; arcs, curved patches and solids are outside it and unbound | `geold_gml_literal_supported`, `ogc_req20_gml_profile_elements`, `ogc_req20_gml_outside_the_profile_is_unbound`, `ogc_req20_gml_srs_dimension_3_keeps_z`, `ogc_req20_gml_srs_name_is_harmonised_against_wkt`, `ogc_req41_*` |
| R16 | geometry-extension/gml-literal-empty | Met | `ogc_req21_gml_literal_empty_is_the_empty_geometry` |
| R17 | geometry-extension/gml-profile | Met — [Supported GML profile](../geosparql.md#supported-gml-profile) | `ogc_req22_gml_profile_is_documented` |
| R18 | geometry-extension/geometry-as-gml-literal | Met | `query_rewrite_reads_every_serialisation`, `geold_gml_literal_supported` |
| R19 | geometry-extension/query-functions | Met | `geo_req27_*`, `geo_req28_*`, `geo_req29_*`, `ogc_req39_*` |
| R20 | geometry-extension/srid-function | Met | `geo_req30_*`, `ogc_req41_get_srid_of_a_gml_literal` |
| R21 | geometry-topology-extension/relate-query-function | Met | `geos_cx_relate_de9im`, `ogc_req43_relate_harmonises_operand_crs` |
| R22 | geometry-topology-extension/sf-query-functions | Met | `geo_req03_*` … `geo_req10_*` |
| R23 | geometry-topology-extension/eh-query-functions | Met | `geo_req11_*` … `geo_req18_*`, `ogc_req45_eh_covered_by_uses_the_spec_mask` |
| R24 | geometry-topology-extension/rcc8-query-functions | Met | `geo_req19_*` … `geo_req26_*` |
| R25 | rdfs-entailment-extension/bgp-rdfs-ent | Met | `rdfs_ent_req47_*`, `geosparql_rdfs_entailment_over_http` |
| R26 | rdfs-entailment-extension/wkt-geometry-types | Met | `rdfs_ent_req48_simple_features_geometry_types` |
| R27 | rdfs-entailment-extension/gml-geometry-types | Met | `rdfs_ent_req49_gml_geometry_types` |
| R28 | query-rewrite-extension/sf-query-rewrite | Met | `query_rewrite_req50_sf_relations`, `query_rewrite_*`, `geosparql_query_rewrite_over_http` |
| R29 | query-rewrite-extension/eh-query-rewrite | Met | `query_rewrite_req51_eh_relations`, `query_rewrite_every_relation_uses_its_own_function` |
| R30 | query-rewrite-extension/rcc8-query-rewrite | Met | `query_rewrite_req52_rcc8_relations`, `query_rewrite_every_relation_uses_its_own_function` |

All 30 are met, and GeoSPARQL 1.0 is graded **Full** in [standards.md](../standards.md) —
the project's own grade, never an OGC certification.

### GeoSPARQL 1.1 (22-047r1)

The requirement IDs are `http://www.opengis.net/spec/geosparql/1.1/req/…`, numbered as in
the standard.

| Req | Requirement | Status | Tests |
|---|---|---|---|
| 1 | core/sparql-protocol | Met | as R1 above |
| 2, 3 | core/spatial-object-class, feature-class | Met | `ogc10_r02_r03_spatial_object_and_feature_classes` |
| 4, 5 | core/spatial-object-collection-class, feature-collection-class | Met | `ogc_req04_05_spatial_object_and_feature_collections` |
| 6 | core/spatial-object-properties | Met | `ogc_req06_spatial_object_properties` |
| 7 | core/feature-properties | Met | `ogc_req07_feature_properties`, `ogc10_r07_r08_r09_geometry_class_and_properties` |
| 8–10 | topology-vocab-extension/sf, eh, rcc8-spatial-relations | Met | as R4–R6 above |
| 11 | geometry-extension/geometry-class | Met | `ogc10_r07_r08_r09_geometry_class_and_properties` |
| 12 | geometry-extension/geometry-collection-class | Met | `ogc_req12_geometry_collection_class` |
| 13 | geometry-extension/geometry-properties | Met | `ogc10_r07_r08_r09_geometry_class_and_properties` |
| 14–18 | wkt-literal, wkt-literal-default-srs, wkt-axis-order, wkt-literal-empty, geometry-as-wkt-literal | Met | as R10–R14 above |
| 19 | geometry-extension/asWKT-function | Met | `ogc_req19_as_wkt_keeps_the_srs_and_z` |
| 20 | geometry-extension/gml-literal | Met (as R15) | as R15 |
| 21 | geometry-extension/gml-literal-empty | Met | `ogc_req21_gml_literal_empty_is_the_empty_geometry` |
| 22 | geometry-extension/gml-profile | Met (as R17) | `ogc_req22_gml_profile_is_documented` |
| 23 | geometry-extension/geometry-as-gml-literal | Met | as R18 |
| 24 | geometry-extension/asGML-function | Met | `ogc_req24_as_gml_round_trips` |
| 25 | geometry-extension/geojson-literal | Met — altitude kept as Z | `ogc_req25_geojson_literal_keeps_altitude`, `geojson_*`, `malformed_geojson_literal_is_unbound_not_a_panic` |
| 26 | geometry-extension/geojson-literal-srs | Met | `geof_as_geojson_reprojects_to_crs84`, `geojson_literal_harmonises_with_a_projected_operand` |
| 27 | geometry-extension/geojson-literal-empty | Met | `ogc_req27_geojson_literal_empty_is_the_empty_geometry` |
| 28 | geometry-extension/geometry-as-geojson-literal | Met | `query_rewrite_reads_every_serialisation`, `geos_cx_geojson_literal_sfwithin` |
| 29 | geometry-extension/asGeoJSON-function | Met | `geof_as_geojson_round_trips` |
| 30 | geometry-extension/kml-literal | Met | `ogc_req30_kml_literal_is_a_geometry` |
| 31 | geometry-extension/kml-literal-srs | Met — always CRS84 | `ogc_req31_kml_literal_is_crs84` |
| 32 | geometry-extension/kml-literal-empty | Met | `ogc_req32_kml_literal_empty_is_the_empty_geometry` |
| 33 | geometry-extension/geometry-as-kml-literal | Met | `ogc_req33_geometry_as_kml_literal_is_queryable` |
| 34 | geometry-extension/asKML-function | Met | `ogc_req34_as_kml_reprojects_and_round_trips` |
| 35–38 | geometry-extension-dggs (DGGS literals, `asDGGS`) | Missing (owner decision) | — |
| 39 | geometry-extension/query-functions | Met — all 23 functions; results follow the first operand's serialisation and SRS (§10.9.1); `geometryType` returns an IRI; `concaveHull`'s default target is 0.5 | `geo_req27_*` … `geo_req29_*`, `ogc_req39_*` (`ogc_req39_geometry_property_functions`, `ogc_req39_geometry_type_is_an_iri`, `ogc_req39_centroid_bounding_circle_and_concave_hull`, `ogc_req39_geometry_results_follow_the_first_operand`, `ogc_req39_40_functions_over_a_non_geometry_are_unbound`) |
| 40 | geometry-extension/query-functions-non-sf | Met — all 14 functions | `geos_cx_geosparql11_metric_functions`, `ogc_req40_*` (`ogc_req40_length_and_perimeter_with_units`, `ogc_req40_area_on_every_serialisation`, `ogc_req40_area_with_area_units`, `ogc_req40_num_geometries_and_geometry_n`, `ogc_req40_min_and_max_ordinates`) |
| 41 | geometry-extension/srid-function | Met — the result is the CRS as an IRI term (owner decision: IRIs, not `xsd:anyURI` literals) | `ogc_req41_get_srid_of_a_gml_literal`, `geos_cx_get_srid` |
| 42 | geometry-extension/sa-functions | Met — all six aggregates; `aggConcaveHull` takes one argument (the parser allows one expression per custom aggregate) with the default target 0.5 | `agg_union_*`, `ogc_req42_*` |
| 43 | geometry-topology-extension/relate-query-function | Met | as R21 |
| 44–46 | geometry-topology-extension/sf, eh, rcc8-query-functions | Met | as R22–R24 |
| 47–49 | rdfs-entailment-extension/bgp-rdfs-ent, wkt-geometry-types, gml-geometry-types | Met | as R25–R27 |
| 50–52 | query-rewrite-extension/sf, eh, rcc8-query-rewrite | Met | as R28–R30 |
| DGGS 39–42 | the DGGS versions of query-functions, query-functions-non-sf, srid-function, sa-functions | Missing (owner decision) | — |

Every requirement outside the DGGS conformance class (`/conf/geometry-extension-dggs`:
Req 35–38 and the DGGS versions of Req 39–42) is met, and GeoSPARQL 1.1 is graded **Full**
in [standards.md](../standards.md) with that scope stated. DGGS is a separate, optional
conformance class of OGC 22-047r1; the project's own grade, never an OGC certification.
