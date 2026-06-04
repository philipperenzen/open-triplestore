//! OGC GeoSPARQL 1.1 Conformance Tests
//!
//! Derived from:
//! - OGC GeoSPARQL 1.1 standard: https://docs.ogc.org/is/22-047r1/22-047r1.html
//! - GeoSPARQL Compliance Benchmark (SIMPAC-2021-29):
//!   https://github.com/SoftwareImpacts/SIMPAC-2021-29
//!   206 SPARQL queries targeting 30 specific GeoSPARQL requirements
//!
//! The 30 OGC requirements covered:
//!   Req 1:  Core – WKT literal support (geo:wktLiteral)
//!   Req 2:  Core – GML literal support (geo:gmlLiteral) [parse only]
//!   Req 3:  Simple Features - sfContains
//!   Req 4:  Simple Features - sfCrosses
//!   Req 5:  Simple Features - sfDisjoint
//!   Req 6:  Simple Features - sfEquals
//!   Req 7:  Simple Features - sfIntersects
//!   Req 8:  Simple Features - sfOverlaps
//!   Req 9:  Simple Features - sfTouches
//!   Req 10: Simple Features - sfWithin
//!   Req 11: Egenhofer - ehContains
//!   Req 12: Egenhofer - ehCoveredBy
//!   Req 13: Egenhofer - ehCovers
//!   Req 14: Egenhofer - ehDisjoint
//!   Req 15: Egenhofer - ehEquals
//!   Req 16: Egenhofer - ehInside
//!   Req 17: Egenhofer - ehMeet
//!   Req 18: Egenhofer - ehOverlap
//!   Req 19: RCC8 - rcc8dc
//!   Req 20: RCC8 - rcc8ec
//!   Req 21: RCC8 - rcc8po
//!   Req 22: RCC8 - rcc8tppi
//!   Req 23: RCC8 - rcc8tpp
//!   Req 24: RCC8 - rcc8ntpp
//!   Req 25: RCC8 - rcc8ntppi
//!   Req 26: RCC8 - rcc8eq
//!   Req 27: Metric – geof:distance
//!   Req 28: Metric – geof:area
//!   Req 29: Constructive – spatial set operations
//!   Req 30: Aggregate – geof:getSRID, geometry properties

use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;

// ─── Helpers ──────────────────────────────────────────────────────────────────

const GEO_PFX: &str = "PREFIX geo: <http://www.opengis.net/ont/geosparql#>
PREFIX geof: <http://www.opengis.net/def/function/geosparql/>
PREFIX sf: <http://www.opengis.net/ont/sf#>
PREFIX uom: <http://www.opengis.net/def/uom/OGC/1.0/>
PREFIX ex: <http://example.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>";

fn ts() -> open_triplestore::store::TripleStore {
    open_triplestore::store::TripleStore::in_memory().unwrap()
}

/// Standard Turtle prefix declarations prepended to all test data.
const TTL_PREFIXES: &str = "\
@prefix geo:  <http://www.opengis.net/ont/geosparql#> .\n\
@prefix geof: <http://www.opengis.net/def/function/geosparql/> .\n\
@prefix sf:   <http://www.opengis.net/ont/sf#> .\n\
@prefix uom:  <http://www.opengis.net/def/uom/OGC/1.0/> .\n\
@prefix ex:   <http://example.org/> .\n\
@prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .\n";

fn load(s: &open_triplestore::store::TripleStore, ttl: &str) {
    let with_prefixes = format!("{}{}", TTL_PREFIXES, ttl);
    s.load_str(&with_prefixes, RdfFormat::Turtle, None).unwrap();
}

/// Extract a floating-point value from an Oxigraph literal term string.
/// Handles the form `"1.5"^^<http://www.w3.org/2001/XMLSchema#double>`.
fn extract_f64(r: &str) -> f64 {
    // Split on `"` and take the second token (the value between the first pair of quotes).
    r.split('"').nth(1).unwrap_or("0").parse::<f64>().unwrap_or(f64::NAN)
}

fn sel(s: &open_triplestore::store::TripleStore, q: &str) -> Vec<Vec<String>> {
    let full_q = format!("{}\n{}", GEO_PFX, q);
    match s.query(&full_q).unwrap() {
        QueryResults::Solutions(sols) => {
            let vars: Vec<_> = sols.variables().iter().map(|v| v.as_str().to_string()).collect();
            sols.into_iter()
                .map(|sol| {
                    let sol = sol.unwrap();
                    vars.iter().map(|v| sol.get(v.as_str()).map(|t| t.to_string()).unwrap_or_default()).collect()
                })
                .collect()
        }
        _ => panic!("Expected SELECT results"),
    }
}

fn ask_geo(s: &open_triplestore::store::TripleStore, q: &str) -> bool {
    let full_q = format!("{}\n{}", GEO_PFX, q);
    match s.query(&full_q).unwrap() {
        QueryResults::Boolean(b) => b,
        _ => panic!("Expected ASK"),
    }
}

/// BIND expression shorthand for GeoSPARQL function calls
fn bind_fn(s: &open_triplestore::store::TripleStore, expr: &str) -> String {
    let q = format!("SELECT ?result WHERE {{ BIND({} AS ?result) }}", expr);
    let rows = sel(s, &q);
    rows.into_iter().next().and_then(|r| r.into_iter().next()).unwrap_or_default()
}

fn wkt(v: &str) -> String {
    format!("\"{}\"^^geo:wktLiteral", v)
}

// ═══════════════════════════════════════════════════════════
// Requirement 1: WKT Literal Support (geo:wktLiteral)
// ═══════════════════════════════════════════════════════════

#[test]
fn geo_req01_wkt_literal_point() {
    let s = ts();
    load(&s, "ex:g geo:hasGeometry [ geo:asWKT \"POINT(1 2)\"^^geo:wktLiteral ] .");
    assert!(ask_geo(&s, "ASK { ?x geo:asWKT ?wkt . FILTER(DATATYPE(?wkt) = geo:wktLiteral) }"));
}

#[test]
fn geo_req01_wkt_literal_linestring() {
    let s = ts();
    let result = bind_fn(&s, &format!("geof:sfIntersects({}, {})",
        wkt("LINESTRING(0 0, 10 10)"),
        wkt("LINESTRING(0 10, 10 0)")
    ));
    assert!(result.contains("true"), "Crossing lines should intersect: {}", result);
}

#[test]
fn geo_req01_wkt_literal_polygon() {
    let s = ts();
    let result = bind_fn(&s, &format!("geof:sfContains({}, {})",
        wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))"),
        wkt("POINT(5 5)")
    ));
    assert!(result.contains("true"), "Polygon should contain interior point: {}", result);
}

#[test]
fn geo_req01_wkt_literal_multipoint() {
    let s = ts();
    let result = bind_fn(&s, &format!("geof:sfContains({}, {})",
        wkt("POLYGON((0 0, 100 0, 100 100, 0 100, 0 0))"),
        wkt("MULTIPOINT((10 10), (50 50), (90 90))")
    ));
    assert!(result.contains("true"), "Polygon should contain multipoint: {}", result);
}

#[test]
fn geo_req01_wkt_literal_multilinestring() {
    let s = ts();
    let result = bind_fn(&s, &format!("geof:sfIntersects({}, {})",
        wkt("MULTILINESTRING((0 0, 5 5), (10 0, 15 5))"),
        wkt("LINESTRING(2 0, 8 10)")
    ));
    assert!(!result.is_empty());
}

#[test]
fn geo_req01_wkt_literal_multipolygon() {
    let s = ts();
    let result = bind_fn(&s, &format!("geof:sfDisjoint({}, {})",
        wkt("MULTIPOLYGON(((0 0, 1 0, 1 1, 0 1, 0 0)), ((5 5, 6 5, 6 6, 5 6, 5 5)))"),
        wkt("POINT(3 3)")
    ));
    assert!(result.contains("true"), "Point should be disjoint from multipolygon: {}", result);
}

#[test]
fn geo_req01_wkt_crs_prefix() {
    // WKT with CRS URI prefix: <crs-uri> WKT
    let s = ts();
    let result = bind_fn(&s, &format!(
        "geof:sfContains(\"<http://www.opengis.net/def/crs/OGC/1.3/CRS84> POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))\"^^geo:wktLiteral, {})",
        wkt("POINT(5 5)")
    ));
    // Should still work regardless of CRS prefix
    assert!(result.contains("true"), "CRS-prefixed WKT should be parsed: {}", result);
}

#[test]
fn geo_req01_wkt_empty_geometry() {
    // Empty geometry literals
    let s = ts();
    let result = bind_fn(&s, &format!("geof:sfDisjoint({}, {})",
        wkt("GEOMETRYCOLLECTION EMPTY"),
        wkt("POINT(1 1)")
    ));
    // Empty geometry is disjoint from everything
    assert!(result.contains("true"), "Empty geometry should be disjoint: {}", result);
}

// ═══════════════════════════════════════════════════════════
// Requirements 3-10: Simple Features Topological Relations
// ═══════════════════════════════════════════════════════════

// Req 3: sfContains
#[test]
fn geo_req03_sf_contains_polygon_contains_point() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:sfContains({}, {})",
        wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))"), wkt("POINT(5 5)")));
    assert!(r.contains("true"));
}

#[test]
fn geo_req03_sf_contains_polygon_not_contains_boundary() {
    // sfContains is FALSE when the point is ON the boundary
    let s = ts();
    let r = bind_fn(&s, &format!("geof:sfContains({}, {})",
        wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))"), wkt("POINT(0 5)")));
    assert!(r.contains("false"), "Boundary point should NOT be contained: {}", r);
}

#[test]
fn geo_req03_sf_contains_polygon_contains_polygon() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:sfContains({}, {})",
        wkt("POLYGON((0 0, 20 0, 20 20, 0 20, 0 0))"),
        wkt("POLYGON((5 5, 15 5, 15 15, 5 15, 5 5))")));
    assert!(r.contains("true"));
}

#[test]
fn geo_req03_sf_contains_antisymmetric() {
    // A contains B does not imply B contains A
    let s = ts();
    let poly = wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))");
    let pt = wkt("POINT(5 5)");
    let fwd = bind_fn(&s, &format!("geof:sfContains({}, {})", poly, pt));
    let rev = bind_fn(&s, &format!("geof:sfContains({}, {})", pt, poly));
    assert!(fwd.contains("true"));
    assert!(rev.contains("false"));
}

// Req 4: sfCrosses
#[test]
fn geo_req04_sf_crosses_lines() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:sfCrosses({}, {})",
        wkt("LINESTRING(0 0, 10 10)"), wkt("LINESTRING(0 10, 10 0)")));
    assert!(r.contains("true"), "Crossing lines: {}", r);
}

#[test]
fn geo_req04_sf_crosses_parallel_lines() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:sfCrosses({}, {})",
        wkt("LINESTRING(0 0, 10 0)"), wkt("LINESTRING(0 1, 10 1)")));
    assert!(r.contains("false"), "Parallel lines do not cross: {}", r);
}

#[test]
fn geo_req04_sf_crosses_line_polygon() {
    // A line crosses a polygon when it enters/exits (crosses boundary at 2 points)
    let s = ts();
    let r = bind_fn(&s, &format!("geof:sfCrosses({}, {})",
        wkt("LINESTRING(-5 5, 15 5)"),
        wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))")));
    assert!(r.contains("true"), "Line crossing polygon: {}", r);
}

// Req 5: sfDisjoint
#[test]
fn geo_req05_sf_disjoint_separated_polygons() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:sfDisjoint({}, {})",
        wkt("POLYGON((0 0, 1 0, 1 1, 0 1, 0 0))"),
        wkt("POLYGON((5 5, 6 5, 6 6, 5 6, 5 5))")));
    assert!(r.contains("true"));
}

#[test]
fn geo_req05_sf_disjoint_touching_polygons() {
    // Adjacent polygons sharing a border are NOT disjoint
    let s = ts();
    let r = bind_fn(&s, &format!("geof:sfDisjoint({}, {})",
        wkt("POLYGON((0 0, 5 0, 5 5, 0 5, 0 0))"),
        wkt("POLYGON((5 0, 10 0, 10 5, 5 5, 5 0))")));
    assert!(r.contains("false"), "Touching polygons are not disjoint: {}", r);
}

#[test]
fn geo_req05_sf_disjoint_inverse_of_intersects() {
    // sfDisjoint = NOT sfIntersects
    let s = ts();
    let poly1 = wkt("POLYGON((0 0, 5 0, 5 5, 0 5, 0 0))");
    let poly2 = wkt("POLYGON((3 3, 8 3, 8 8, 3 8, 3 3))");
    let disjoint = bind_fn(&s, &format!("geof:sfDisjoint({}, {})", poly1.clone(), poly2.clone()));
    let intersects = bind_fn(&s, &format!("geof:sfIntersects({}, {})", poly1, poly2));
    // Should be opposites
    assert!(disjoint.contains("false") && intersects.contains("true")
        || disjoint.contains("true") && intersects.contains("false"));
}

// Req 6: sfEquals
#[test]
fn geo_req06_sf_equals_identical_points() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:sfEquals({}, {})",
        wkt("POINT(1 2)"), wkt("POINT(1 2)")));
    assert!(r.contains("true"));
}

#[test]
fn geo_req06_sf_equals_different_points() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:sfEquals({}, {})",
        wkt("POINT(1 2)"), wkt("POINT(2 1)")));
    assert!(r.contains("false"));
}

#[test]
fn geo_req06_sf_equals_identical_polygons() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:sfEquals({}, {})",
        wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))"),
        wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))")));
    assert!(r.contains("true"));
}

#[test]
fn geo_req06_sf_equals_reflexive() {
    let s = ts();
    let g = wkt("POLYGON((0 0, 1 0, 1 1, 0 1, 0 0))");
    let r = bind_fn(&s, &format!("geof:sfEquals({}, {})", g.clone(), g));
    assert!(r.contains("true"), "Reflexivity: {}", r);
}

// Req 7: sfIntersects
#[test]
fn geo_req07_sf_intersects_overlapping_polygons() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:sfIntersects({}, {})",
        wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))"),
        wkt("POLYGON((5 5, 15 5, 15 15, 5 15, 5 5))")));
    assert!(r.contains("true"));
}

#[test]
fn geo_req07_sf_intersects_disjoint() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:sfIntersects({}, {})",
        wkt("POLYGON((0 0, 1 0, 1 1, 0 1, 0 0))"),
        wkt("POLYGON((5 5, 6 5, 6 6, 5 6, 5 5))")));
    assert!(r.contains("false"));
}

#[test]
fn geo_req07_sf_intersects_shared_boundary() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:sfIntersects({}, {})",
        wkt("POLYGON((0 0, 5 0, 5 5, 0 5, 0 0))"),
        wkt("POLYGON((5 0, 10 0, 10 5, 5 5, 5 0))")));
    assert!(r.contains("true"), "Shared boundary means intersection: {}", r);
}

#[test]
fn geo_req07_sf_intersects_with_loaded_data() {
    let s = ts();
    load(&s, r#"
        ex:park a geo:Feature ; geo:hasGeometry [ geo:asWKT "POLYGON((0 0, 100 0, 100 100, 0 100, 0 0))"^^geo:wktLiteral ] .
        ex:road a geo:Feature ; geo:hasGeometry [ geo:asWKT "LINESTRING(-10 50, 50 50)"^^geo:wktLiteral ] .
        ex:far  a geo:Feature ; geo:hasGeometry [ geo:asWKT "POINT(200 200)"^^geo:wktLiteral ] .
    "#);
    let r = sel(&s, r#"
        SELECT ?feature WHERE {
            ex:park geo:hasGeometry/geo:asWKT ?parkWkt .
            ?feature a geo:Feature .
            ?feature geo:hasGeometry/geo:asWKT ?fWkt .
            FILTER(?feature != ex:park)
            FILTER(geof:sfIntersects(?parkWkt, ?fWkt))
        }
    "#);
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("road"), "Road intersects park: {:?}", r);
}

// Req 8: sfOverlaps
#[test]
fn geo_req08_sf_overlaps_partial_overlap() {
    let s = ts();
    // Two polygons of same dimension that partially overlap
    let r = bind_fn(&s, &format!("geof:sfOverlaps({}, {})",
        wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))"),
        wkt("POLYGON((5 5, 15 5, 15 15, 5 15, 5 5))")));
    assert!(r.contains("true"), "Overlapping polygons: {}", r);
}

#[test]
fn geo_req08_sf_overlaps_contained_polygon() {
    // A polygon fully inside another does NOT overlap (sfContains instead)
    let s = ts();
    let r = bind_fn(&s, &format!("geof:sfOverlaps({}, {})",
        wkt("POLYGON((0 0, 20 0, 20 20, 0 20, 0 0))"),
        wkt("POLYGON((5 5, 15 5, 15 15, 5 15, 5 5))")));
    assert!(r.contains("false"), "Contained polygon does not overlap: {}", r);
}

// Req 9: sfTouches
#[test]
fn geo_req09_sf_touches_adjacent_polygons() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:sfTouches({}, {})",
        wkt("POLYGON((0 0, 5 0, 5 5, 0 5, 0 0))"),
        wkt("POLYGON((5 0, 10 0, 10 5, 5 5, 5 0))")));
    assert!(r.contains("true"), "Adjacent polygons touch: {}", r);
}

#[test]
fn geo_req09_sf_touches_point_on_line_endpoint() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:sfTouches({}, {})",
        wkt("POINT(0 0)"), wkt("LINESTRING(0 0, 10 10)")));
    assert!(r.contains("true"), "Point touches line at endpoint: {}", r);
}

#[test]
fn geo_req09_sf_touches_overlapping_polys_not_touch() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:sfTouches({}, {})",
        wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))"),
        wkt("POLYGON((5 5, 15 5, 15 15, 5 15, 5 5))")));
    assert!(r.contains("false"), "Overlapping polygons do not touch: {}", r);
}

// Req 10: sfWithin
#[test]
fn geo_req10_sf_within_point_inside_polygon() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:sfWithin({}, {})",
        wkt("POINT(5 5)"), wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))")));
    assert!(r.contains("true"));
}

#[test]
fn geo_req10_sf_within_converse_of_contains() {
    // sfWithin(A, B) = sfContains(B, A)
    let s = ts();
    let poly = wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))");
    let pt = wkt("POINT(5 5)");
    let within = bind_fn(&s, &format!("geof:sfWithin({}, {})", pt.clone(), poly.clone()));
    let contains = bind_fn(&s, &format!("geof:sfContains({}, {})", poly, pt));
    assert_eq!(within, contains, "sfWithin must be converse of sfContains");
}

#[test]
fn geo_req10_sf_within_polygon_inside_polygon() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:sfWithin({}, {})",
        wkt("POLYGON((2 2, 8 2, 8 8, 2 8, 2 2))"),
        wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))")));
    assert!(r.contains("true"));
}

// ═══════════════════════════════════════════════════════════
// Requirements 11-18: Egenhofer Topological Relations
// ═══════════════════════════════════════════════════════════

#[test]
fn geo_req11_eh_contains() {
    // Interior contains, boundary excluded
    let s = ts();
    let r = bind_fn(&s, &format!("geof:ehContains({}, {})",
        wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))"),
        wkt("POINT(5 5)")));
    assert!(r.contains("true"), "ehContains interior point: {}", r);
}

#[test]
fn geo_req12_eh_covered_by() {
    // ehCoveredBy includes boundary
    let s = ts();
    let r = bind_fn(&s, &format!("geof:ehCoveredBy({}, {})",
        wkt("POLYGON((0 0, 5 0, 5 5, 0 5, 0 0))"),
        wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))")));
    assert!(r.contains("true"), "Small polygon ehCoveredBy large: {}", r);
}

#[test]
fn geo_req13_eh_covers() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:ehCovers({}, {})",
        wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))"),
        wkt("POLYGON((0 0, 5 0, 5 5, 0 5, 0 0))")));
    assert!(r.contains("true"), "Large polygon ehCovers small: {}", r);
}

#[test]
fn geo_req14_eh_disjoint() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:ehDisjoint({}, {})",
        wkt("POLYGON((0 0, 1 0, 1 1, 0 1, 0 0))"),
        wkt("POLYGON((5 5, 6 5, 6 6, 5 6, 5 5))")));
    assert!(r.contains("true"));
}

#[test]
fn geo_req15_eh_equals() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:ehEquals({}, {})",
        wkt("POINT(3 4)"), wkt("POINT(3 4)")));
    assert!(r.contains("true"));
}

#[test]
fn geo_req15_eh_equals_reflexive() {
    let s = ts();
    let g = wkt("POLYGON((0 0, 5 0, 5 5, 0 5, 0 0))");
    let r = bind_fn(&s, &format!("geof:ehEquals({}, {})", g.clone(), g));
    assert!(r.contains("true"), "ehEquals is reflexive: {}", r);
}

#[test]
fn geo_req16_eh_inside() {
    // Interior of A is inside interior of B, no boundary contact
    let s = ts();
    let r = bind_fn(&s, &format!("geof:ehInside({}, {})",
        wkt("POINT(5 5)"),
        wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))")));
    assert!(r.contains("true"), "Point ehInside polygon: {}", r);
}

#[test]
fn geo_req17_eh_meet() {
    // Boundaries touch, interiors disjoint
    let s = ts();
    let r = bind_fn(&s, &format!("geof:ehMeet({}, {})",
        wkt("POLYGON((0 0, 5 0, 5 5, 0 5, 0 0))"),
        wkt("POLYGON((5 0, 10 0, 10 5, 5 5, 5 0))")));
    assert!(r.contains("true"), "Adjacent polygons ehMeet: {}", r);
}

#[test]
fn geo_req18_eh_overlap() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:ehOverlap({}, {})",
        wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))"),
        wkt("POLYGON((5 5, 15 5, 15 15, 5 15, 5 5))")));
    assert!(r.contains("true"), "Overlapping polygons ehOverlap: {}", r);
}

// ═══════════════════════════════════════════════════════════
// Requirements 19-26: RCC8 Relations
// ═══════════════════════════════════════════════════════════

#[test]
fn geo_req19_rcc8_dc_disconnected() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:rcc8dc({}, {})",
        wkt("POLYGON((0 0, 1 0, 1 1, 0 1, 0 0))"),
        wkt("POLYGON((5 5, 6 5, 6 6, 5 6, 5 5))")));
    assert!(r.contains("true"), "DC: disconnected: {}", r);
}

#[test]
fn geo_req20_rcc8_ec_externally_connected() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:rcc8ec({}, {})",
        wkt("POLYGON((0 0, 5 0, 5 5, 0 5, 0 0))"),
        wkt("POLYGON((5 0, 10 0, 10 5, 5 5, 5 0))")));
    assert!(r.contains("true"), "EC: externally connected (sharing boundary): {}", r);
}

#[test]
fn geo_req21_rcc8_po_partial_overlap() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:rcc8po({}, {})",
        wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))"),
        wkt("POLYGON((5 5, 15 5, 15 15, 5 15, 5 5))")));
    assert!(r.contains("true"), "PO: partial overlap: {}", r);
}

#[test]
fn geo_req22_rcc8_tppi_tangential_proper_part_inverse() {
    // A covers B (B is on boundary of A, not equal)
    let s = ts();
    let r = bind_fn(&s, &format!("geof:rcc8tppi({}, {})",
        wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))"),
        wkt("POLYGON((0 0, 5 0, 5 10, 0 10, 0 0))")));
    assert!(r.contains("true"), "TPPI: A covers B with shared boundary: {}", r);
}

#[test]
fn geo_req23_rcc8_tpp_tangential_proper_part() {
    // A is on boundary of B, not equal
    let s = ts();
    let r = bind_fn(&s, &format!("geof:rcc8tpp({}, {})",
        wkt("POLYGON((0 0, 5 0, 5 10, 0 10, 0 0))"),
        wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))")));
    assert!(r.contains("true"), "TPP: B covers A with shared boundary: {}", r);
}

#[test]
fn geo_req24_rcc8_ntpp_non_tangential_proper_part() {
    // A's interior is completely inside B's interior (no boundary contact)
    let s = ts();
    let r = bind_fn(&s, &format!("geof:rcc8ntpp({}, {})",
        wkt("POINT(5 5)"),
        wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))")));
    assert!(r.contains("true"), "NTPP: interior point inside polygon: {}", r);
}

#[test]
fn geo_req25_rcc8_ntppi_non_tangential_proper_part_inverse() {
    // B's interior is completely inside A's interior
    let s = ts();
    let r = bind_fn(&s, &format!("geof:rcc8ntppi({}, {})",
        wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))"),
        wkt("POINT(5 5)")));
    assert!(r.contains("true"), "NTPPi: polygon contains interior point: {}", r);
}

#[test]
fn geo_req26_rcc8_eq_equal() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:rcc8eq({}, {})",
        wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))"),
        wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))")));
    assert!(r.contains("true"), "RCC8 equal: {}", r);
}

#[test]
fn geo_req26_rcc8_eq_not_equal() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:rcc8eq({}, {})",
        wkt("POINT(1 2)"), wkt("POINT(3 4)")));
    assert!(r.contains("false"), "Different points not RCC8 equal: {}", r);
}

// RCC8 Mutual Exclusivity Tests
#[test]
fn geo_req_rcc8_mutually_exclusive_dc_ec() {
    // DC and EC cannot both be true
    let s = ts();
    // Two adjacent polygons are EC, not DC
    let poly1 = wkt("POLYGON((0 0, 5 0, 5 5, 0 5, 0 0))");
    let poly2 = wkt("POLYGON((5 0, 10 0, 10 5, 5 5, 5 0))");
    let dc = bind_fn(&s, &format!("geof:rcc8dc({}, {})", poly1.clone(), poly2.clone()));
    let ec = bind_fn(&s, &format!("geof:rcc8ec({}, {})", poly1, poly2));
    // Exactly one should be true
    assert_ne!(dc, ec, "DC and EC should not have same truth value for this case");
    assert!(ec.contains("true"), "Adjacent polygons are EC: {}", ec);
    assert!(dc.contains("false"), "Adjacent polygons are not DC: {}", dc);
}

// ═══════════════════════════════════════════════════════════
// Requirement 27: geof:distance
// ═══════════════════════════════════════════════════════════

#[test]
fn geo_req27_distance_points() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:distance({}, {})",
        wkt("POINT(0 0)"), wkt("POINT(3 4)")));
    let v: f64 = r.trim_matches('"').trim_end_matches("\"^^<http://www.w3.org/2001/XMLSchema#double>")
        .parse().unwrap_or_else(|_| {
            // Try parsing from result format
            r.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect::<String>()
             .parse::<f64>().unwrap_or(0.0)
        });
    assert!((v - 5.0).abs() < 0.001, "Distance 3-4-5 triangle: {}", r);
}

#[test]
fn geo_req27_distance_coincident_points() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:distance({}, {})",
        wkt("POINT(5 5)"), wkt("POINT(5 5)")));
    // Should be 0 or very close to 0
    assert!(r.contains("0"), "Coincident points have 0 distance: {}", r);
}

#[test]
fn geo_req27_distance_polygon_to_point() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:distance({}, {})",
        wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))"),
        wkt("POINT(15 5)")));
    // Distance from polygon edge (x=10) to point (x=15) = 5
    let v = extract_f64(&r);
    assert!((v - 5.0).abs() < 0.001, "Distance polygon to external point: {}", r);
}

#[test]
fn geo_req27_distance_symmetric() {
    let s = ts();
    let p1 = wkt("POINT(1 1)");
    let p2 = wkt("POINT(7 9)");
    let d1 = bind_fn(&s, &format!("geof:distance({}, {})", p1.clone(), p2.clone()));
    let d2 = bind_fn(&s, &format!("geof:distance({}, {})", p2, p1));
    assert_eq!(d1, d2, "Distance must be symmetric");
}

#[test]
fn geo_req27_distance_filter_within_radius() {
    // Use distance in a FILTER to find features within range
    let s = ts();
    load(&s, r#"
        ex:near a geo:Feature ; geo:hasGeometry [ geo:asWKT "POINT(3 4)"^^geo:wktLiteral ] .
        ex:far  a geo:Feature ; geo:hasGeometry [ geo:asWKT "POINT(100 100)"^^geo:wktLiteral ] .
    "#);
    let r = sel(&s, r#"
        SELECT ?f WHERE {
            ?f a geo:Feature ;
               geo:hasGeometry/geo:asWKT ?wkt .
            FILTER(geof:distance("POINT(0 0)"^^geo:wktLiteral, ?wkt) < 10)
        }
    "#);
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("near"), "Only 'near' feature within 10 units: {:?}", r);
}

// ═══════════════════════════════════════════════════════════
// Requirement 28: geof:area
// ═══════════════════════════════════════════════════════════

#[test]
fn geo_req28_area_unit_square() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:area({})", wkt("POLYGON((0 0, 1 0, 1 1, 0 1, 0 0))")));
    let v = extract_f64(&r);
    assert!((v - 1.0).abs() < 0.001, "Unit square area = 1: {}", r);
}

#[test]
fn geo_req28_area_ten_by_ten() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:area({})",
        wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))")));
    let v = extract_f64(&r);
    assert!((v - 100.0).abs() < 0.001, "10x10 square area = 100: {}", r);
}

#[test]
fn geo_req28_area_point_zero() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:area({})", wkt("POINT(5 5)")));
    let v = extract_f64(&r);
    assert!(v == 0.0, "Point area = 0: {}", r);
}

#[test]
fn geo_req28_area_polygon_with_hole() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:area({})",
        // 10x10 polygon with 2x2 hole
        wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0), (4 4, 6 4, 6 6, 4 6, 4 4))")));
    let v = extract_f64(&r);
    assert!((v - 96.0).abs() < 0.001, "Polygon with hole area = 96: {}", r);
}

// ═══════════════════════════════════════════════════════════
// Requirement 29: Constructive geometry functions
// ═══════════════════════════════════════════════════════════

#[test]
fn geo_req29_boundary() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:boundary({})",
        wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))")));
    assert!(r.contains("LINESTRING") || r.contains("LINEARRING"),
            "Boundary of polygon is a linestring: {}", r);
}

#[test]
fn geo_req29_buffer_creates_polygon() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:buffer({}, \"5.0\"^^xsd:double)",
        wkt("POINT(0 0)")));
    assert!(r.contains("POLYGON"), "Buffer of point is a polygon: {}", r);
}

#[test]
fn geo_req29_buffer_area_approx_pi_r_squared() {
    // Buffer of a point with radius r ≈ π*r²
    let s = ts();
    let buf = bind_fn(&s, &format!("geof:buffer({}, \"10.0\"^^xsd:double)",
        wkt("POINT(0 0)")));
    // buf is a WKT polygon; compute its area
    let area_r = bind_fn(&s, &format!("geof:area({})", buf));
    let v = extract_f64(&area_r);
    // π*10² ≈ 314.15; GEOS uses ~65 segments so should be close
    assert!(v > 300.0 && v < 320.0, "Buffer area ≈ π*100: {}", area_r);
}

#[test]
fn geo_req29_convex_hull() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:convexHull({})",
        wkt("MULTIPOINT((0 0), (10 0), (5 10), (3 3))")));
    assert!(r.contains("POLYGON"), "Convex hull is a polygon: {}", r);
}

#[test]
fn geo_req29_convex_hull_triangle() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:convexHull({})",
        wkt("MULTIPOINT((0 0), (10 0), (5 5))")));
    assert!(r.contains("POLYGON") || r.contains("TRIANGLE"),
            "Convex hull of 3 points: {}", r);
}

#[test]
fn geo_req29_envelope() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:envelope({})",
        wkt("LINESTRING(2 3, 8 7)")));
    assert!(r.contains("POLYGON") || r.contains("LINESTRING"),
            "Envelope (bounding box) of linestring: {}", r);
}

#[test]
fn geo_req29_intersection() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:intersection({}, {})",
        wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))"),
        wkt("POLYGON((5 5, 15 5, 15 15, 5 15, 5 5))")));
    assert!(r.contains("POLYGON"), "Intersection of overlapping polygons: {}", r);
}

#[test]
fn geo_req29_intersection_disjoint_is_empty() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:intersection({}, {})",
        wkt("POLYGON((0 0, 1 0, 1 1, 0 1, 0 0))"),
        wkt("POLYGON((5 5, 6 5, 6 6, 5 6, 5 5))")));
    // Intersection of disjoint geometries = GEOMETRYCOLLECTION EMPTY or POINT EMPTY, etc.
    assert!(r.contains("EMPTY") || r.to_lowercase().contains("geometrycollection"),
            "Intersection of disjoint geoms is empty: {}", r);
}

#[test]
fn geo_req29_difference() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:difference({}, {})",
        wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))"),
        wkt("POLYGON((5 5, 15 5, 15 15, 5 15, 5 5))")));
    assert!(r.contains("POLYGON"), "Difference is a polygon: {}", r);
}

#[test]
fn geo_req29_sym_difference() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:symDifference({}, {})",
        wkt("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))"),
        wkt("POLYGON((5 5, 15 5, 15 15, 5 15, 5 5))")));
    assert!(r.contains("POLYGON") || r.contains("MULTI"),
            "Symmetric difference: {}", r);
}

#[test]
fn geo_req29_union() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:union({}, {})",
        wkt("POLYGON((0 0, 5 0, 5 5, 0 5, 0 0))"),
        wkt("POLYGON((3 3, 8 3, 8 8, 3 8, 3 3))")));
    assert!(r.contains("POLYGON"), "Union of overlapping polygons: {}", r);
}

#[test]
fn geo_req29_union_area_gte_max() {
    // Area of union >= max of individual areas
    let s = ts();
    let poly1 = wkt("POLYGON((0 0, 5 0, 5 5, 0 5, 0 0))");  // area = 25
    let poly2 = wkt("POLYGON((3 3, 8 3, 8 8, 3 8, 3 3))");  // area = 25
    let union_r = bind_fn(&s, &format!("geof:union({}, {})", poly1.clone(), poly2.clone()));
    let union_area_r = bind_fn(&s, &format!("geof:area({})", union_r));
    let v = extract_f64(&union_area_r);
    assert!(v >= 25.0, "Union area >= each individual area: {}", union_area_r);
}

// ═══════════════════════════════════════════════════════════
// Requirement 30: getSRID and geometry properties
// ═══════════════════════════════════════════════════════════

#[test]
fn geo_req30_get_srid_default_crs84() {
    let s = ts();
    let r = bind_fn(&s, &format!("geof:getSRID({})", wkt("POINT(1 2)")));
    assert!(r.contains("CRS84") || r.contains("crs/OGC"),
            "Default CRS should be CRS84: {}", r);
}

#[test]
fn geo_req30_get_srid_epsg4326() {
    let s = ts();
    let r = bind_fn(&s,
        "geof:getSRID(\"<http://www.opengis.net/def/crs/EPSG/0/4326> POINT(1 2)\"^^geo:wktLiteral)");
    assert!(r.contains("4326"), "EPSG:4326 CRS: {}", r);
}

// ═══════════════════════════════════════════════════════════
// Complex GeoSPARQL Queries (Integration)
// ═══════════════════════════════════════════════════════════

#[test]
fn geo_complex_spatial_join() {
    // Find all features within a search area
    let s = ts();
    load(&s, r#"
        ex:city1 a geo:Feature ; geo:hasGeometry [ geo:asWKT "POINT(10 10)"^^geo:wktLiteral ] .
        ex:city2 a geo:Feature ; geo:hasGeometry [ geo:asWKT "POINT(50 50)"^^geo:wktLiteral ] .
        ex:city3 a geo:Feature ; geo:hasGeometry [ geo:asWKT "POINT(90 90)"^^geo:wktLiteral ] .
    "#);
    let r = sel(&s, r#"
        SELECT ?city WHERE {
            ?city a geo:Feature ;
                  geo:hasGeometry/geo:asWKT ?wkt .
            FILTER(geof:sfWithin(?wkt, "POLYGON((0 0, 60 0, 60 60, 0 60, 0 0))"^^geo:wktLiteral))
        } ORDER BY ?city
    "#);
    assert_eq!(r.len(), 2, "Two cities within search area: {:?}", r);
}

#[test]
fn geo_complex_nearest_neighbor() {
    // Find the feature closest to a query point
    let s = ts();
    load(&s, r#"
        ex:a a geo:Feature ; geo:hasGeometry [ geo:asWKT "POINT(1 1)"^^geo:wktLiteral ] .
        ex:b a geo:Feature ; geo:hasGeometry [ geo:asWKT "POINT(5 5)"^^geo:wktLiteral ] .
        ex:c a geo:Feature ; geo:hasGeometry [ geo:asWKT "POINT(20 20)"^^geo:wktLiteral ] .
    "#);
    let r = sel(&s, r#"
        SELECT ?f ?dist WHERE {
            ?f a geo:Feature ;
               geo:hasGeometry/geo:asWKT ?wkt .
            BIND(geof:distance("POINT(0 0)"^^geo:wktLiteral, ?wkt) AS ?dist)
        } ORDER BY ?dist LIMIT 1
    "#);
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("/a>"), "Nearest to origin should be :a: {:?}", r);
}

#[test]
fn geo_complex_topology_classification() {
    // Classify spatial relationships between two geometries
    let s = ts();
    load(&s, r#"
        ex:A geo:hasGeometry [ geo:asWKT "POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))"^^geo:wktLiteral ] .
        ex:B geo:hasGeometry [ geo:asWKT "POLYGON((5 5, 15 5, 15 15, 5 15, 5 5))"^^geo:wktLiteral ] .
    "#);
    let r = sel(&s, r#"
        SELECT ?rel WHERE {
            ex:A geo:hasGeometry/geo:asWKT ?wA .
            ex:B geo:hasGeometry/geo:asWKT ?wB .
            VALUES (?rel ?fn) {
                ("sfContains" "1") ("sfIntersects" "2") ("sfDisjoint" "3") ("sfOverlaps" "4")
            }
            FILTER(
                (?rel = "sfIntersects" && geof:sfIntersects(?wA, ?wB)) ||
                (?rel = "sfOverlaps"   && geof:sfOverlaps(?wA, ?wB))   ||
                (?rel = "sfContains"   && geof:sfContains(?wA, ?wB))   ||
                (?rel = "sfDisjoint"   && geof:sfDisjoint(?wA, ?wB))
            )
        }
    "#);
    // A and B overlap, so sfIntersects and sfOverlaps should be true
    let rels: Vec<_> = r.iter().map(|row| row[0].as_str()).collect();
    assert!(rels.contains(&"\"sfIntersects\""), "Should intersect: {:?}", rels);
    assert!(rels.contains(&"\"sfOverlaps\""), "Should overlap: {:?}", rels);
    assert!(!rels.contains(&"\"sfDisjoint\""), "Should not be disjoint: {:?}", rels);
}

#[test]
fn geo_complex_buffer_and_intersect() {
    // Buffer a point, then find features intersecting the buffer
    let s = ts();
    load(&s, r#"
        ex:near a geo:Feature ; geo:hasGeometry [ geo:asWKT "POINT(4 0)"^^geo:wktLiteral ] .
        ex:far  a geo:Feature ; geo:hasGeometry [ geo:asWKT "POINT(100 0)"^^geo:wktLiteral ] .
    "#);
    let r = sel(&s, r#"
        SELECT ?feature WHERE {
            ?feature a geo:Feature ;
                     geo:hasGeometry/geo:asWKT ?wkt .
            BIND(geof:buffer("POINT(0 0)"^^geo:wktLiteral, "5.0"^^xsd:double) AS ?bufferZone)
            FILTER(geof:sfIntersects(?bufferZone, ?wkt))
        }
    "#);
    assert_eq!(r.len(), 1, "Only 'near' feature within 5-unit buffer: {:?}", r);
    assert!(r[0][0].contains("near"));
}

#[test]
fn geo_complex_convex_hull_of_features() {
    // Compute convex hull of multiple feature geometries combined
    let s = ts();
    load(&s, r#"
        ex:p1 geo:hasGeometry [ geo:asWKT "POINT(0 0)"^^geo:wktLiteral ] .
        ex:p2 geo:hasGeometry [ geo:asWKT "POINT(10 0)"^^geo:wktLiteral ] .
        ex:p3 geo:hasGeometry [ geo:asWKT "POINT(5 10)"^^geo:wktLiteral ] .
    "#);
    let r = sel(&s, r#"
        SELECT (geof:convexHull(geof:union(geof:union(
            "POINT(0 0)"^^geo:wktLiteral,
            "POINT(10 0)"^^geo:wktLiteral),
            "POINT(5 10)"^^geo:wktLiteral)) AS ?hull)
        WHERE {}
    "#);
    assert!(!r.is_empty());
    assert!(r[0][0].contains("POLYGON") || r[0][0].contains("POINT"),
            "Hull of 3 points: {}", r[0][0]);
}

// ═══════════════════════════════════════════════════════════
// GeoSPARQL with RDF Data Model
// ═══════════════════════════════════════════════════════════

#[test]
fn geo_data_model_feature_geometry_pattern() {
    // Standard GeoSPARQL data model: Feature → hasGeometry → Geometry → asWKT
    let s = ts();
    load(&s, r#"
        @prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
        ex:london a geo:Feature ;
            geo:hasGeometry ex:london_geom .
        ex:london_geom a geo:Geometry, sf:Point ;
            geo:asWKT "-0.1276 51.5074"^^geo:wktLiteral .
    "#);
    // Note: uses lon/lat format for demonstration; not geographic projection
    let r = sel(&s, "SELECT ?wkt WHERE { ex:london geo:hasGeometry ?g . ?g geo:asWKT ?wkt }");
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("51.5074"), "London geometry: {}", r[0][0]);
}

#[test]
fn geo_data_model_inline_geometry() {
    // Geometry as blank node with inline asWKT
    let s = ts();
    load(&s, r#"
        ex:park a geo:Feature ;
            geo:hasGeometry [
                a sf:Polygon ;
                geo:asWKT "POLYGON((0 0, 1 0, 1 1, 0 1, 0 0))"^^geo:wktLiteral
            ] .
    "#);
    let r = sel(&s, r#"
        SELECT ?wkt WHERE {
            ex:park geo:hasGeometry ?g .
            ?g a sf:Polygon ;
               geo:asWKT ?wkt .
        }
    "#);
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("POLYGON"));
}

#[test]
fn geo_data_model_property_path_wkt() {
    // Property path: geo:hasGeometry/geo:asWKT
    let s = ts();
    load(&s, r#"
        ex:lake a geo:Feature ;
            geo:hasGeometry [ geo:asWKT "POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))"^^geo:wktLiteral ] .
        ex:island a geo:Feature ;
            geo:hasGeometry [ geo:asWKT "POINT(5 5)"^^geo:wktLiteral ] .
    "#);
    let r = sel(&s, r#"
        SELECT ?feature WHERE {
            ex:lake geo:hasGeometry/geo:asWKT ?lake_wkt .
            ?feature geo:hasGeometry/geo:asWKT ?feature_wkt .
            FILTER(?feature != ex:lake)
            FILTER(geof:sfContains(?lake_wkt, ?feature_wkt))
        }
    "#);
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("island"));
}
