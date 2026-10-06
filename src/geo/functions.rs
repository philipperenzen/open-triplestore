//! GeoSPARQL function implementations.
//!
//! All functions are registered as custom SPARQL functions via Oxigraph's
//! `QueryOptions::with_custom_function` API. Each function takes oxrdf Terms
//! as arguments, parses geometry literals (WKT, GML or GeoJSON — see
//! [`super::datatypes::literal_wkt`]), performs spatial operations using the
//! GEOS library, and returns result Terms.
//!
//! Implements:
//! - Simple Features (SF) topological relations
//! - Egenhofer topological relations
//! - RCC8 topological relations
//! - Non-topological / constructive functions
//! - Scalar measurement functions, and the geometry properties and other
//!   non-topological functions of GeoSPARQL 1.1 (`dimension`, `centroid`,
//!   `boundingCircle`, `concaveHull`, `length`, `geometryN`, `minX`, …)
//! - Metric functions — metres on the WGS84 ellipsoid (see [`super::geodesic`])
//! - Serialisation (`geof:asWKT`, `geof:asGML`, `geof:asGeoJSON`, `geof:asKML`)
//!
//! A geometry result is a literal of the first operand's serialisation, in its
//! CRS ([`geometry_to_literal_like`]).
//!
//! Units of measure (`geof:distance`, `geof:buffer`, `geof:area`; see
//! [`parse_uom`] for the OGC, QUDT and EPSG IRIs understood): a linear unit on a
//! geographic CRS (CRS84, EPSG:4326) is geodesic metres, the same as the metric
//! functions; an angular unit there scales the planar degrees. A projected CRS
//! (RD New, Web Mercator) computes planar in its own metres, converted between
//! linear units only. Without a unit (or with `uom:unity`) every CRS is planar
//! in its own units. Any other combination — an unknown unit, an angular unit
//! on a projected CRS, a unit on a CRS this build does not know, an area unit
//! for a distance — is unbound rather than a number in units nobody asked for.

use std::sync::Arc;

use geos::{Geom, Geometry as GeosGeometry};
use oxrdf::{NamedNode, Term};

use super::crs::Crs;
use super::datatypes::*;
use super::geodesic;
use super::vocabulary as vocab;

/// Type alias for the custom function handler that Oxigraph expects.
type FnHandler = Arc<dyn Fn(&[Term]) -> Option<Term> + Send + Sync>;

/// Returns all GeoSPARQL functions as (IRI, handler) pairs for registration.
pub fn all_functions() -> Vec<(NamedNode, FnHandler)> {
    vec![
        // ─── Simple Features topological relations ───
        make_fn(vocab::SF_CONTAINS, sf_contains),
        make_fn(vocab::SF_CROSSES, sf_crosses),
        make_fn(vocab::SF_DISJOINT, sf_disjoint),
        make_fn(vocab::SF_EQUALS, sf_equals),
        make_fn(vocab::SF_INTERSECTS, sf_intersects),
        make_fn(vocab::SF_OVERLAPS, sf_overlaps),
        make_fn(vocab::SF_TOUCHES, sf_touches),
        make_fn(vocab::SF_WITHIN, sf_within),
        // ─── Egenhofer topological relations ───
        make_fn(vocab::EH_CONTAINS, eh_contains),
        make_fn(vocab::EH_COVERED_BY, eh_covered_by),
        make_fn(vocab::EH_COVERS, eh_covers),
        make_fn(vocab::EH_DISJOINT, eh_disjoint),
        make_fn(vocab::EH_EQUALS, eh_equals),
        make_fn(vocab::EH_INSIDE, eh_inside),
        make_fn(vocab::EH_MEET, eh_meet),
        make_fn(vocab::EH_OVERLAP, eh_overlap),
        // ─── RCC8 topological relations ───
        make_fn(vocab::RCC8_DC, rcc8_dc),
        make_fn(vocab::RCC8_EC, rcc8_ec),
        make_fn(vocab::RCC8_PO, rcc8_po),
        make_fn(vocab::RCC8_TPPI, rcc8_tppi),
        make_fn(vocab::RCC8_TPP, rcc8_tpp),
        make_fn(vocab::RCC8_NTPP, rcc8_ntpp),
        make_fn(vocab::RCC8_NTPPI, rcc8_ntppi),
        make_fn(vocab::RCC8_EQ, rcc8_eq),
        // ─── Non-topological / constructive functions ───
        make_fn(vocab::BOUNDARY, fn_boundary),
        make_fn(vocab::BUFFER, fn_buffer),
        make_fn(vocab::CONVEX_HULL, fn_convex_hull),
        make_fn(vocab::DIFFERENCE, fn_difference),
        make_fn(vocab::ENVELOPE, fn_envelope),
        make_fn(vocab::INTERSECTION, fn_intersection),
        make_fn(vocab::SYM_DIFFERENCE, fn_sym_difference),
        make_fn(vocab::UNION, fn_union),
        // ─── Scalar measurement functions ───
        make_fn(vocab::DISTANCE, fn_distance),
        make_fn(vocab::AREA, fn_area),
        make_fn(vocab::GET_SRID, fn_get_srid),
        make_fn(vocab::RELATE, fn_relate),
        // ─── Metric functions (GeoSPARQL 1.1): metres on the WGS84 ellipsoid ───
        make_fn(vocab::METRIC_DISTANCE, fn_metric_distance),
        make_fn(vocab::METRIC_AREA, fn_metric_area),
        make_fn(vocab::METRIC_LENGTH, fn_metric_length),
        make_fn(vocab::METRIC_PERIMETER, fn_metric_perimeter),
        make_fn(vocab::METRIC_BUFFER, fn_metric_buffer),
        // ─── CRS transform ───
        make_fn(vocab::TRANSFORM, fn_transform),
        // ─── Serialisation ───
        make_fn(vocab::AS_GEOJSON, fn_as_geojson),
        make_fn(vocab::AS_WKT, fn_as_wkt),
        make_fn(vocab::AS_GML, fn_as_gml),
        make_fn(vocab::AS_KML, fn_as_kml),
        // ─── Geometry properties and the other non-topological functions ───
        make_fn(vocab::BOUNDING_CIRCLE, fn_bounding_circle),
        make_fn(vocab::CENTROID, fn_centroid),
        make_fn(vocab::CONCAVE_HULL, fn_concave_hull),
        make_fn(vocab::COORDINATE_DIMENSION, fn_coordinate_dimension),
        make_fn(vocab::DIMENSION, fn_dimension),
        make_fn(vocab::GEOMETRY_TYPE, fn_geometry_type),
        make_fn(vocab::IS_3D, fn_is_3d),
        make_fn(vocab::IS_EMPTY, fn_is_empty),
        make_fn(vocab::IS_MEASURED, fn_is_measured),
        make_fn(vocab::IS_SIMPLE, fn_is_simple),
        make_fn(vocab::SPATIAL_DIMENSION, fn_spatial_dimension),
        make_fn(vocab::LENGTH, fn_length),
        make_fn(vocab::PERIMETER, fn_perimeter),
        make_fn(vocab::GEOMETRY_N, fn_geometry_n),
        make_fn(vocab::NUM_GEOMETRIES, fn_num_geometries),
        make_fn(vocab::MAX_X, fn_max_x),
        make_fn(vocab::MAX_Y, fn_max_y),
        make_fn(vocab::MAX_Z, fn_max_z),
        make_fn(vocab::MIN_X, fn_min_x),
        make_fn(vocab::MIN_Y, fn_min_y),
        make_fn(vocab::MIN_Z, fn_min_z),
    ]
}

/// Helper to construct a (NamedNode, Arc<Fn>) pair.
fn make_fn(iri: &str, f: fn(&[Term]) -> Option<Term>) -> (NamedNode, FnHandler) {
    (NamedNode::new_unchecked(iri), Arc::new(f))
}

// ─── Argument parsing helpers ───

/// Parse two geometry arguments, harmonising their coordinate reference systems.
///
/// GeoSPARQL requires the operands of a topological or metric function to be in
/// a common CRS. Both `<crs>` prefixes used to be stripped and discarded, so
/// `geof:sfIntersects("<…/28992> POINT(187420 428470)", "POINT(5.86 51.85)")`
/// compared metres against degrees and returned `false` with no error — a
/// silently wrong answer, which is the worst kind for a spatial filter.
///
/// The second operand is transformed into the first's CRS. When the two name
/// different CRS and either is one this build cannot reproject, the result is
/// unbound rather than a comparison of incompatible numbers.
///
/// The CRS is [`literal_crs_uri`]'s — a WKT prefix or a GML `srsName` — so a
/// GML literal in RD New meets a CRS84 WKT literal in the same place.
fn parse_two_geoms(args: &[Term]) -> Option<(GeosGeometry, GeosGeometry)> {
    let [a, b, ..] = args else {
        return None;
    };
    let (crs1, crs2) = (literal_crs_uri(a), literal_crs_uri(b));
    let (g1, g2) = (parse_wkt_literal(a)?, parse_wkt_literal(b)?);
    // Identical CRS strings (including both absent — GeoSPARQL's CRS84 default)
    // need no transform, and this also covers a CRS this build does not know:
    // comparing two geometries in the SAME unknown CRS is still meaningful.
    if crs1 == crs2 {
        return Some((g1, g2));
    }

    let to = crs1.as_deref().map_or(Some(Crs::Wgs84), Crs::from_uri)?;
    let from = crs2.as_deref().map_or(Some(Crs::Wgs84), Crs::from_uri)?;
    // Different spellings of one CRS (`/4326` vs `:4326`) reproject as a copy.
    Some((g1, reproject_geometry(&g2, from, to)?))
}

/// Parse a single geometry argument.
fn parse_one_geom(args: &[Term]) -> Option<GeosGeometry> {
    if args.is_empty() {
        return None;
    }
    parse_wkt_literal(&args[0])
}

// ═══════════════════════════════════════════════════════════════
// Simple Features (SF) Topological Relations
// Based on OGC Simple Features Access (ISO 19125-1)
// ═══════════════════════════════════════════════════════════════

fn sf_contains(args: &[Term]) -> Option<Term> {
    let (g1, g2) = parse_two_geoms(args)?;
    let result = g1.contains(&g2).ok()?;
    Some(boolean_literal(result))
}

fn sf_crosses(args: &[Term]) -> Option<Term> {
    let (g1, g2) = parse_two_geoms(args)?;
    let result = g1.crosses(&g2).ok()?;
    Some(boolean_literal(result))
}

fn sf_disjoint(args: &[Term]) -> Option<Term> {
    let (g1, g2) = parse_two_geoms(args)?;
    let result = g1.disjoint(&g2).ok()?;
    Some(boolean_literal(result))
}

fn sf_equals(args: &[Term]) -> Option<Term> {
    let (g1, g2) = parse_two_geoms(args)?;
    let result = g1.equals(&g2).ok()?;
    Some(boolean_literal(result))
}

fn sf_intersects(args: &[Term]) -> Option<Term> {
    let (g1, g2) = parse_two_geoms(args)?;
    let result = g1.intersects(&g2).ok()?;
    Some(boolean_literal(result))
}

fn sf_overlaps(args: &[Term]) -> Option<Term> {
    let (g1, g2) = parse_two_geoms(args)?;
    let result = g1.overlaps(&g2).ok()?;
    Some(boolean_literal(result))
}

fn sf_touches(args: &[Term]) -> Option<Term> {
    let (g1, g2) = parse_two_geoms(args)?;
    let result = g1.touches(&g2).ok()?;
    Some(boolean_literal(result))
}

fn sf_within(args: &[Term]) -> Option<Term> {
    let (g1, g2) = parse_two_geoms(args)?;
    let result = g1.within(&g2).ok()?;
    Some(boolean_literal(result))
}

// ═══════════════════════════════════════════════════════════════
// Egenhofer Topological Relations
// Implemented using DE-9IM intersection matrix patterns
// ═══════════════════════════════════════════════════════════════

/// Check a DE-9IM relationship pattern.
fn relates_pattern(g1: &GeosGeometry, g2: &GeosGeometry, pattern: &str) -> Option<bool> {
    g1.relate_pattern(g2, pattern).ok()
}

/// geof:relate(g1, g2, pattern) — true iff the DE-9IM intersection matrix of g1
/// and g2 matches the 9-character `pattern` (chars from the set T/F/*/0/1/2).
/// OGC GeoSPARQL Requirement 44 (Geometry Extension).
fn fn_relate(args: &[Term]) -> Option<Term> {
    if args.len() < 3 {
        return None;
    }
    // Harmonised like every other binary function: a DE-9IM matrix of RD New
    // metres against CRS84 degrees means nothing.
    let (g1, g2) = parse_two_geoms(args)?;
    let pattern = match &args[2] {
        Term::Literal(l) => l.value().to_string(),
        _ => return None,
    };
    let result = g1.relate_pattern(&g2, &pattern).ok()?;
    Some(boolean_literal(result))
}

fn eh_contains(args: &[Term]) -> Option<Term> {
    let (g1, g2) = parse_two_geoms(args)?;
    // Egenhofer contains: T*TFF*FF*
    let result = relates_pattern(&g1, &g2, "T*TFF*FF*")?;
    Some(boolean_literal(result))
}

fn eh_covered_by(args: &[Term]) -> Option<Term> {
    let (g1, g2) = parse_two_geoms(args)?;
    // Egenhofer coveredBy: TFF*TFT** — the transpose of ehCovers's mask. A
    // geometry strictly inside the other (empty boundary contact) is ehInside,
    // not ehCoveredBy; a line on a polygon's boundary is neither.
    let result = relates_pattern(&g1, &g2, "TFF*TFT**")?;
    Some(boolean_literal(result))
}

fn eh_covers(args: &[Term]) -> Option<Term> {
    let (g1, g2) = parse_two_geoms(args)?;
    // Egenhofer covers: T*TFT*FF*
    let result = relates_pattern(&g1, &g2, "T*TFT*FF*")?;
    Some(boolean_literal(result))
}

fn eh_disjoint(args: &[Term]) -> Option<Term> {
    let (g1, g2) = parse_two_geoms(args)?;
    // Egenhofer disjoint: FF*FF****
    let result = relates_pattern(&g1, &g2, "FF*FF****")?;
    Some(boolean_literal(result))
}

fn eh_equals(args: &[Term]) -> Option<Term> {
    let (g1, g2) = parse_two_geoms(args)?;
    // Use GEOS native equals() which works for all geometry types including
    // points (which have empty boundaries, so DE-9IM pattern TFFFTFFFT fails
    // for them because it requires non-empty boundary intersection at position 4).
    let result = g1.equals(&g2).ok()?;
    Some(boolean_literal(result))
}

fn eh_inside(args: &[Term]) -> Option<Term> {
    let (g1, g2) = parse_two_geoms(args)?;
    // Egenhofer inside: TFF*FFT**
    let result = relates_pattern(&g1, &g2, "TFF*FFT**")?;
    Some(boolean_literal(result))
}

fn eh_meet(args: &[Term]) -> Option<Term> {
    let (g1, g2) = parse_two_geoms(args)?;
    // Egenhofer meet: boundaries intersect but interiors don't
    // FT*******  OR  F**T*****  OR  F***T****
    let r1 = relates_pattern(&g1, &g2, "FT*******")?;
    let r2 = relates_pattern(&g1, &g2, "F**T*****")?;
    let r3 = relates_pattern(&g1, &g2, "F***T****")?;
    Some(boolean_literal(r1 || r2 || r3))
}

fn eh_overlap(args: &[Term]) -> Option<Term> {
    let (g1, g2) = parse_two_geoms(args)?;
    // Egenhofer overlap: T*T***T**
    let result = relates_pattern(&g1, &g2, "T*T***T**")?;
    Some(boolean_literal(result))
}

// ═══════════════════════════════════════════════════════════════
// RCC8 Topological Relations
// Region Connection Calculus with 8 base relations
// ═══════════════════════════════════════════════════════════════

fn rcc8_dc(args: &[Term]) -> Option<Term> {
    // Disconnected = Egenhofer disjoint
    eh_disjoint(args)
}

fn rcc8_ec(args: &[Term]) -> Option<Term> {
    // Externally connected = Egenhofer meet
    eh_meet(args)
}

fn rcc8_po(args: &[Term]) -> Option<Term> {
    // Partial overlap = Egenhofer overlap
    eh_overlap(args)
}

fn rcc8_tppi(args: &[Term]) -> Option<Term> {
    // Tangential proper part inverse = (covers ∧ ¬equals) ∧ ¬(non-tangential inverse).
    // Without the ¬ntppi guard, TPPi would also match the non-tangential case (NTPPi),
    // violating RCC8's requirement that its eight base relations be mutually exclusive.
    let (g1, g2) = parse_two_geoms(args)?;
    let covers = relates_pattern(&g1, &g2, "T*TFT*FF*")?;
    let equals = relates_pattern(&g1, &g2, "TFFFTFFFT")?;
    // NTPPi uses the Egenhofer "contains" mask.
    let ntppi = relates_pattern(&g1, &g2, "T*TFF*FF*")?;
    Some(boolean_literal(covers && !equals && !ntppi))
}

fn rcc8_tpp(args: &[Term]) -> Option<Term> {
    // Tangential proper part = (coveredBy ∧ ¬equals) ∧ ¬(non-tangential proper part).
    // Without the ¬ntpp guard, TPP would also match the non-tangential case (NTPP),
    // violating RCC8's requirement that its eight base relations be mutually exclusive.
    // Native GEOS covered_by/equals handle boundary-sharing and mixed types correctly.
    let (g1, g2) = parse_two_geoms(args)?;
    let covered_by = g1.covered_by(&g2).ok()?;
    let equals = g1.equals(&g2).ok()?;
    // NTPP uses the Egenhofer "inside" mask.
    let ntpp = relates_pattern(&g1, &g2, "TFF*FFT**")?;
    Some(boolean_literal(covered_by && !equals && !ntpp))
}

fn rcc8_ntpp(args: &[Term]) -> Option<Term> {
    // Non-tangential proper part = Egenhofer inside
    eh_inside(args)
}

fn rcc8_ntppi(args: &[Term]) -> Option<Term> {
    // Non-tangential proper part inverse = Egenhofer contains
    eh_contains(args)
}

fn rcc8_eq(args: &[Term]) -> Option<Term> {
    // Equal = Egenhofer equals
    eh_equals(args)
}

// ═══════════════════════════════════════════════════════════════
// Non-topological (Constructive) Functions
// Return new geometry literals
// ═══════════════════════════════════════════════════════════════

fn fn_boundary(args: &[Term]) -> Option<Term> {
    let result = parse_one_geom(args)?.boundary().ok()?;
    geometry_to_literal_like(&result, args.first()?)
}

/// Whether a CRS is geographic (degrees of longitude and latitude).
fn is_geographic(crs: Crs) -> bool {
    matches!(crs, Crs::Wgs84 | Crs::Epsg4326)
}

/// An optional unit-of-measure argument: `Some(None)` when there is none,
/// `None` (the function is unbound) when it names no unit [`parse_uom`] knows.
fn units_arg(term: Option<&Term>) -> Option<Option<Uom>> {
    match term {
        None => Some(None),
        Some(t) => parse_uom(t).map(Some),
    }
}

/// The radius argument of a buffer: any numeric literal.
fn radius_arg(term: Option<&Term>) -> Option<f64> {
    match term {
        Some(Term::Literal(lit)) => lit.value().parse::<f64>().ok(),
        _ => None,
    }
}

/// `geof:buffer(geom, radius, units)` in the operand's CRS. A linear unit on a
/// geographic CRS buffers by geodesic metres (as `geof:metricBuffer`); an
/// angular unit there buffers by that many planar degrees. A projected CRS
/// buffers planar in its own metres, a linear radius converted. No unit (or
/// `uom:unity`): the radius is in the CRS's own units. Anything else is
/// unbound (see the module docs).
fn fn_buffer(args: &[Term]) -> Option<Term> {
    let term = args.first()?;
    let radius = radius_arg(args.get(1))?;
    let units = units_arg(args.get(2))?;
    let crs = literal_crs(term);
    let geographic = crs.is_some_and(is_geographic);
    let native_radius = match units {
        None | Some(Uom::Unity) => radius,
        Some(Uom::Linear(metres)) if geographic => {
            return metric_buffer_literal(term, crs?, radius * metres)
        }
        Some(Uom::Linear(metres)) if crs.is_some() => radius * metres,
        Some(Uom::Angular(degrees)) if geographic => radius * degrees,
        _ => return None,
    };
    let result = parse_one_geom(args)?.buffer(native_radius, 16).ok()?;
    geometry_to_literal_like(&result, term)
}

/// A geodesic buffer of `radius_m` metres around a geometry literal in `crs`,
/// as a literal of the same serialisation in the same CRS (and with the same
/// prefix or `srsName` spelling).
fn metric_buffer_literal(term: &Term, crs: Crs, radius_m: f64) -> Option<Term> {
    use wkt::ToWkt;
    let buffered = geodesic::metric_buffer(&geodesic::literal_to_crs84(term)?, radius_m)?;
    let out = geodesic::reproject(&buffered, Crs::Wgs84, crs)?;
    let geos = GeosGeometry::new_from_wkt(&out.wkt_string()).ok()?;
    geometry_to_literal_like(&geos, term)
}

fn fn_convex_hull(args: &[Term]) -> Option<Term> {
    let result = parse_one_geom(args)?.convex_hull().ok()?;
    geometry_to_literal_like(&result, args.first()?)
}

fn fn_difference(args: &[Term]) -> Option<Term> {
    // parse_two_geoms harmonises into the FIRST operand's CRS, so the result is
    // in that CRS — and in that operand's serialisation.
    let (g1, g2) = parse_two_geoms(args)?;
    let result = g1.difference(&g2).ok()?;
    geometry_to_literal_like(&result, args.first()?)
}

fn fn_envelope(args: &[Term]) -> Option<Term> {
    let result = parse_one_geom(args)?.envelope().ok()?;
    geometry_to_literal_like(&result, args.first()?)
}

fn fn_intersection(args: &[Term]) -> Option<Term> {
    // parse_two_geoms harmonises into the FIRST operand's CRS, so the result is
    // in that CRS — and in that operand's serialisation.
    let (g1, g2) = parse_two_geoms(args)?;
    let result = g1.intersection(&g2).ok()?;
    geometry_to_literal_like(&result, args.first()?)
}

fn fn_sym_difference(args: &[Term]) -> Option<Term> {
    // parse_two_geoms harmonises into the FIRST operand's CRS, so the result is
    // in that CRS — and in that operand's serialisation.
    let (g1, g2) = parse_two_geoms(args)?;
    let result = g1.sym_difference(&g2).ok()?;
    geometry_to_literal_like(&result, args.first()?)
}

fn fn_union(args: &[Term]) -> Option<Term> {
    // parse_two_geoms harmonises into the FIRST operand's CRS, so the result is
    // in that CRS — and in that operand's serialisation.
    let (g1, g2) = parse_two_geoms(args)?;
    let result = g1.union(&g2).ok()?;
    geometry_to_literal_like(&result, args.first()?)
}

// ═══════════════════════════════════════════════════════════════
// Scalar Measurement Functions
// ═══════════════════════════════════════════════════════════════

/// `geof:distance(geom1, geom2, units)`, calculated in the CRS of `geom1`.
///
/// A linear unit on a geographic CRS is the geodesic distance on the WGS84
/// ellipsoid (as `geof:metricDistance`) in that unit — it used to be the planar
/// distance in *degrees*, whatever the unit said. An angular unit there converts
/// the planar degree distance. On a projected CRS the distance is planar in the
/// CRS's own metres, converted between linear units only. No unit (or
/// `uom:unity`): planar, in the CRS's own units. Anything else is unbound (see
/// the module docs).
fn fn_distance(args: &[Term]) -> Option<Term> {
    let units = units_arg(args.get(2))?;
    let crs = literal_crs(args.first()?);
    let geographic = crs.is_some_and(is_geographic);
    if let (Some(Uom::Linear(metres)), true) = (units, geographic) {
        let metric = geodesic::literal_distance(args.first()?, args.get(1)?)?;
        return Some(double_literal(metric / metres));
    }
    let per_unit = match units {
        None | Some(Uom::Unity) => 1.0,
        Some(Uom::Linear(metres)) if crs.is_some() => metres,
        Some(Uom::Angular(degrees)) if geographic => degrees,
        _ => return None,
    };
    let (g1, g2) = parse_two_geoms(args)?;
    Some(double_literal(g1.distance(&g2).ok()? / per_unit))
}

// ═══════════════════════════════════════════════════════════════
// Metric functions (GeoSPARQL 1.1) — metres on the WGS84 ellipsoid,
// whatever the operand's CRS; unbound for a CRS this build cannot
// reproject. See `super::geodesic`.
// ═══════════════════════════════════════════════════════════════

/// `geof:metricDistance(geom1, geom2)` — shortest geodesic distance in metres.
fn fn_metric_distance(args: &[Term]) -> Option<Term> {
    geodesic::literal_distance(args.first()?, args.get(1)?).map(double_literal)
}

/// `geof:metricArea(geom)` — geodesic area in square metres; zero for anything
/// but (multi)polygons.
fn fn_metric_area(args: &[Term]) -> Option<Term> {
    let g = geodesic::literal_to_crs84(args.first()?)?;
    Some(double_literal(geodesic::metric_area(&g)))
}

/// `geof:metricLength(geom)` — geodesic length in metres of the lines, and of a
/// polygon's rings; zero for points.
fn fn_metric_length(args: &[Term]) -> Option<Term> {
    let g = geodesic::literal_to_crs84(args.first()?)?;
    Some(double_literal(geodesic::metric_length(&g)))
}

/// `geof:metricPerimeter(geom)` — geodesic length in metres of a polygon's
/// rings, holes included. GeoSPARQL 1.1 makes the perimeter of a non-areal
/// geometry its length, so a line's perimeter is its length (a point's zero) —
/// the same number as `geof:metricLength` for every geometry.
fn fn_metric_perimeter(args: &[Term]) -> Option<Term> {
    let g = geodesic::literal_to_crs84(args.first()?)?;
    Some(double_literal(geodesic::metric_length(&g)))
}

/// `geof:metricBuffer(geom, radius)` — a geodesic buffer of `radius` metres,
/// returned in the operand's CRS.
fn fn_metric_buffer(args: &[Term]) -> Option<Term> {
    let term = args.first()?;
    let radius = radius_arg(args.get(1))?;
    metric_buffer_literal(term, geodesic::literal_crs(term)?, radius)
}

/// `geof:transform(geom, targetCrsIri)` — reproject a geometry literal to the target
/// CRS (OGC GeoSPARQL Geometry Extension). The source CRS is the literal's
/// ([`literal_crs_uri`]: a WKT prefix or a GML `srsName`, else CRS84); the target an
/// IRI or an `xsd:anyURI` literal. Supported CRS are CRS84 and EPSG:28992 / 4326 /
/// 3857 (see [`super::crs`]). Returns a literal of the operand's serialisation in
/// the target CRS — a WKT literal prefixed with it, a GML literal with it as its
/// `srsName`, GeoJSON or KML when the target is CRS84 (a WKT literal otherwise) —
/// Z kept, or `None` if either CRS is unsupported, the geometry does not parse,
/// or any coordinate lies outside a CRS's domain — a coordinate that does not
/// transform is never copied through.
fn fn_transform(args: &[Term]) -> Option<Term> {
    let term = args.first()?;
    let source = literal_crs(term)?;
    let target = Crs::from_uri(&super::crs::normalise_crs_uri(iri_arg(args.get(1)?)?))?;
    let out = reproject_geometry(&parse_one_geom(args)?, source, target)?;
    let target_uri =
        (target != Crs::Wgs84 || !Serialisation::of(term).is_crs84_only()).then(|| target.to_uri());
    geometry_to_literal(&out, Serialisation::of(term), target_uri)
}

/// `geof:asWKT(geom)` — the geometry as a `geo:wktLiteral` in its own CRS (a
/// GML `srsName` becomes the WKT prefix, normalised), Z kept.
fn fn_as_wkt(args: &[Term]) -> Option<Term> {
    let term = args.first()?;
    geometry_to_wkt_literal_in(&parse_wkt_literal(term)?, literal_crs_uri(term).as_deref())
}

/// `geof:asGML(geom, gmlProfile)` — the geometry as a `geo:gmlLiteral` in its
/// own CRS, which becomes the `srsName` (CRS84 written out when the operand
/// names none), with `srsDimension="3"` when it has Z. The profile argument is
/// optional and every value is accepted: the output is always the GML 3.2
/// profile this store reads (see [`super::gml`]).
fn fn_as_gml(args: &[Term]) -> Option<Term> {
    let term = args.first()?;
    if args
        .get(1)
        .is_some_and(|profile| !matches!(profile, Term::Literal(_)))
    {
        return None;
    }
    let crs = literal_crs_uri(term);
    geometry_to_literal(
        &parse_wkt_literal(term)?,
        Serialisation::Gml,
        Some(crs.as_deref().unwrap_or(vocab::CRS84)),
    )
}

/// `geof:asKML(geom)` — the geometry as a `geo:kmlLiteral`. KML is CRS84 by
/// definition, so the geometry is reprojected from its literal's CRS on the
/// way out; `None` for a CRS this build cannot reproject.
fn fn_as_kml(args: &[Term]) -> Option<Term> {
    let term = args.first()?;
    let source = literal_crs(term)?;
    let geom = parse_wkt_literal(term)?;
    let to_crs84 = |x: f64, y: f64| super::crs::transform_xy(source, Crs::Wgs84, x, y);
    Some(Term::Literal(oxrdf::Literal::new_typed_literal(
        super::kml::geometry_to_kml(&geom, &to_crs84)?,
        NamedNode::new_unchecked(vocab::KML_LITERAL),
    )))
}

/// `geof:asGeoJSON(geom)` — the geometry as a `geo:geoJSONLiteral` (GeoSPARQL 1.1
/// Geometry Extension). GeoJSON is CRS84 by definition (RFC 7946), so the
/// geometry is reprojected from its literal's CRS (a GML literal's `srsName`
/// included) on the way out; `None` for a CRS this build cannot reproject, or a
/// geometry that does not parse.
fn fn_as_geojson(args: &[Term]) -> Option<Term> {
    let term = args.first()?;
    let source = geodesic::literal_crs(term)?;
    let geom = parse_wkt_literal(term)?;
    let to_crs84 = |x: f64, y: f64| super::crs::transform_xy(source, Crs::Wgs84, x, y);
    let json = super::geojson::geometry_to_geojson(&geom, &to_crs84)?;
    Some(Term::Literal(oxrdf::Literal::new_typed_literal(
        json.to_string(),
        NamedNode::new_unchecked(vocab::GEOJSON_LITERAL),
    )))
}

/// `geof:area(geom, units)`. An area unit on a geographic CRS is the geodesic
/// area (as `geof:metricArea`) in that unit; on a projected CRS the planar area
/// in its own square metres, converted. No unit (or `uom:unity`): the planar
/// area in the CRS's own units. Anything else is unbound.
fn fn_area(args: &[Term]) -> Option<Term> {
    let term = args.first()?;
    let units = units_arg(args.get(1))?;
    let crs = literal_crs(term);
    let per_unit = match units {
        None | Some(Uom::Unity) => 1.0,
        Some(Uom::Area(square_metres)) if crs.is_some_and(is_geographic) => {
            let g = geodesic::literal_to_crs84(term)?;
            return Some(double_literal(geodesic::metric_area(&g) / square_metres));
        }
        Some(Uom::Area(square_metres)) if crs.is_some() => square_metres,
        _ => return None,
    };
    Some(double_literal(
        parse_one_geom(args)?.area().ok()? / per_unit,
    ))
}

fn fn_get_srid(args: &[Term]) -> Option<Term> {
    // The literal's CRS — a WKT `<crs>` prefix or a GML `srsName`, normalised —
    // and CRS84 by default, which is also what a GeoJSON literal always has.
    match args.first()? {
        term @ Term::Literal(_) => {
            let crs = literal_crs_uri(term);
            NamedNode::new(crs.as_deref().unwrap_or(vocab::CRS84))
                .ok()
                .map(Term::NamedNode)
        }
        _ => None,
    }
}

// ═══════════════════════════════════════════════════════════════
// Geometry properties and the other non-topological functions
// (GeoSPARQL 1.1 Req 39 and 40)
// ═══════════════════════════════════════════════════════════════

/// Create an `xsd:integer` literal.
fn integer_literal(value: i64) -> Term {
    Term::Literal(oxrdf::Literal::new_typed_literal(
        value.to_string(),
        NamedNode::new_unchecked("http://www.w3.org/2001/XMLSchema#integer"),
    ))
}

/// An integer argument: an integer-valued numeric literal.
fn integer_arg(term: Option<&Term>) -> Option<i64> {
    let Some(Term::Literal(l)) = term else {
        return None;
    };
    let v = l.value().trim();
    v.parse::<i64>().ok().or_else(|| {
        let f = v.parse::<f64>().ok()?;
        (f.fract() == 0.0 && f.abs() < 9.0e15).then_some(f as i64)
    })
}

/// `geof:dimension(geom)` — the topological dimension: 0 for points, 1 for
/// curves, 2 for surfaces, the largest of a collection's members. Unbound for
/// an empty collection, which has none.
fn fn_dimension(args: &[Term]) -> Option<Term> {
    let d = match parse_one_geom(args)?.get_dimension().ok()? {
        geos::DimensionType::Point => 0,
        geos::DimensionType::Curve => 1,
        geos::DimensionType::Surface => 2,
    };
    Some(integer_literal(d))
}

/// `geof:coordinateDimension(geom)` — numbers per position: 2, 3 with Z or M,
/// 4 with both.
fn fn_coordinate_dimension(args: &[Term]) -> Option<Term> {
    let (z, m) = literal_dimensions(args.first()?)?;
    Some(integer_literal(2 + i64::from(z) + i64::from(m)))
}

/// `geof:spatialDimension(geom)` — spatial ordinates per position: 2, or 3 with Z.
fn fn_spatial_dimension(args: &[Term]) -> Option<Term> {
    let (z, _) = literal_dimensions(args.first()?)?;
    Some(integer_literal(2 + i64::from(z)))
}

/// `geof:is3D(geom)` — whether its positions have Z.
fn fn_is_3d(args: &[Term]) -> Option<Term> {
    let (z, _) = literal_dimensions(args.first()?)?;
    Some(boolean_literal(z))
}

/// `geof:isMeasured(geom)` — whether its positions have M. Read from the
/// literal (`POINT M`, `POINT ZM`); GML, GeoJSON and KML have no M.
fn fn_is_measured(args: &[Term]) -> Option<Term> {
    let (_, m) = literal_dimensions(args.first()?)?;
    Some(boolean_literal(m))
}

/// `geof:isEmpty(geom)`.
fn fn_is_empty(args: &[Term]) -> Option<Term> {
    Some(boolean_literal(parse_one_geom(args)?.is_empty().ok()?))
}

/// `geof:isSimple(geom)` — no anomalous points (self-intersection, self-tangency).
fn fn_is_simple(args: &[Term]) -> Option<Term> {
    Some(boolean_literal(parse_one_geom(args)?.is_simple().ok()?))
}

/// `geof:geometryType(geom)` — the geometry's class, as an IRI: the GML element
/// of a GML literal (`gml:Surface`, `gml:Envelope`, …, in the GeoSPARQL GML
/// namespace), the Simple Features class otherwise (`sf:Point`,
/// `sf:MultiPolygon`, …).
fn fn_geometry_type(args: &[Term]) -> Option<Term> {
    let term = args.first()?;
    let g = parse_wkt_literal(term)?;
    if let (Serialisation::Gml, Term::Literal(l)) = (Serialisation::of(term), term) {
        if let Some(name) = super::gml::gml_root_name(l.value()) {
            return Some(Term::NamedNode(NamedNode::new_unchecked(format!(
                "{}{name}",
                vocab::GML_NS
            ))));
        }
    }
    use geos::GeometryTypes as T;
    let class = match g.geometry_type().ok()? {
        T::Point => "Point",
        T::LineString => "LineString",
        T::LinearRing => "LinearRing",
        T::Polygon => "Polygon",
        T::MultiPoint => "MultiPoint",
        T::MultiLineString => "MultiLineString",
        T::MultiPolygon => "MultiPolygon",
        T::GeometryCollection => "GeometryCollection",
        #[allow(unreachable_patterns)] // curve types, with GEOS >= 3.13 features
        _ => return None,
    };
    Some(Term::NamedNode(NamedNode::new_unchecked(format!(
        "{}{class}",
        vocab::SF_NS
    ))))
}

/// `geof:numGeometries(geom)` — the members of a collection; 1 for a
/// geometry that is not one.
fn fn_num_geometries(args: &[Term]) -> Option<Term> {
    let n = parse_one_geom(args)?.get_num_geometries().ok()?;
    Some(integer_literal(i64::try_from(n).ok()?))
}

/// `geof:geometryN(geom, n)` — the `n`th member of a collection, counting from
/// 1 as Simple Features Access does; a geometry that is not a collection is its
/// own first member. Unbound out of range. The member keeps the operand's
/// serialisation and CRS.
fn fn_geometry_n(args: &[Term]) -> Option<Term> {
    let g = parse_one_geom(args)?;
    let n = integer_arg(args.get(1))?;
    let count = g.get_num_geometries().ok()?;
    let index = usize::try_from(n.checked_sub(1)?).ok()?;
    if index >= count {
        return None;
    }
    let member: GeosGeometry = match g.geometry_type().ok()? {
        geos::GeometryTypes::MultiPoint
        | geos::GeometryTypes::MultiLineString
        | geos::GeometryTypes::MultiPolygon
        | geos::GeometryTypes::GeometryCollection => {
            Geom::clone(&g.get_geometry_n(index).ok()?).ok()?
        }
        _ => g,
    };
    geometry_to_literal_like(&member, args.first()?)
}

/// Every position of a geometry as `(x, y, z)`, `z` `None` without Z.
fn positions(geom: &impl Geom, out: &mut Vec<(f64, f64, Option<f64>)>) -> Option<()> {
    use geos::GeometryTypes as T;
    if geom.is_empty().ok()? {
        return Some(());
    }
    match geom.geometry_type().ok()? {
        T::Point | T::LineString | T::LinearRing => {
            let cs = geom.get_coord_seq().ok()?;
            let z = geom.has_z().ok()?;
            for i in 0..cs.size().ok()? {
                let pz = if z {
                    cs.get_z(i).ok().filter(|v| v.is_finite())
                } else {
                    None
                };
                out.push((cs.get_x(i).ok()?, cs.get_y(i).ok()?, pz));
            }
        }
        T::Polygon => {
            positions(&geom.get_exterior_ring().ok()?, out)?;
            for i in 0..geom.get_num_interior_rings().ok()? {
                positions(&geom.get_interior_ring_n(i).ok()?, out)?;
            }
        }
        _ => {
            for i in 0..geom.get_num_geometries().ok()? {
                positions(&geom.get_geometry_n(i).ok()?, out)?;
            }
        }
    }
    Some(())
}

/// The minimum or maximum of one ordinate over a geometry's positions; unbound
/// for the empty geometry, and for Z when a position has none.
fn ordinate_extreme(args: &[Term], ordinate: usize, max: bool) -> Option<Term> {
    let mut ps = Vec::new();
    positions(&parse_one_geom(args)?, &mut ps)?;
    let values = ps
        .iter()
        .map(|&(x, y, z)| match ordinate {
            0 => Some(x),
            1 => Some(y),
            _ => z,
        })
        .collect::<Option<Vec<f64>>>()?;
    let pick = |a: f64, b: f64| if max { a.max(b) } else { a.min(b) };
    let v = values.into_iter().reduce(pick)?;
    Some(double_literal(v))
}

fn fn_min_x(args: &[Term]) -> Option<Term> {
    ordinate_extreme(args, 0, false)
}
fn fn_max_x(args: &[Term]) -> Option<Term> {
    ordinate_extreme(args, 0, true)
}
fn fn_min_y(args: &[Term]) -> Option<Term> {
    ordinate_extreme(args, 1, false)
}
fn fn_max_y(args: &[Term]) -> Option<Term> {
    ordinate_extreme(args, 1, true)
}
fn fn_min_z(args: &[Term]) -> Option<Term> {
    ordinate_extreme(args, 2, false)
}
fn fn_max_z(args: &[Term]) -> Option<Term> {
    ordinate_extreme(args, 2, true)
}

/// `geof:centroid(geom)` — the centroid, as a point in the operand's
/// serialisation and CRS (the empty point for the empty geometry).
fn fn_centroid(args: &[Term]) -> Option<Term> {
    let c = parse_one_geom(args)?.get_centroid().ok()?;
    geometry_to_literal_like(&c, args.first()?)
}

/// GEOS buffer segments per quarter circle for a bounding circle, as `geof:buffer`.
const CIRCLE_QUADRANT_SEGMENTS: i32 = 16;

/// The minimum bounding circle of a geometry's positions (Welzl's algorithm)
/// as a polygon of 64 segments that *circumscribes* that circle, so every
/// position lies inside or on it (a polygon through points on the circle would
/// cut corners off it). A single position is its own bounding circle, a point;
/// the empty geometry's is the empty polygon. Z is not considered.
pub(super) fn minimum_bounding_circle(geom: &impl Geom) -> Option<GeosGeometry> {
    let mut ps = Vec::new();
    positions(geom, &mut ps)?;
    let mut pts: Vec<(f64, f64)> = ps.into_iter().map(|(x, y, _)| (x, y)).collect();
    pts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    pts.dedup();
    if pts.is_empty() {
        return GeosGeometry::create_empty_polygon().ok();
    }
    let (cx, cy, r) = welzl(&mut pts);
    let centre = GeosGeometry::new_from_wkt(&format!("POINT({cx} {cy})")).ok()?;
    if r == 0.0 {
        return Some(centre);
    }
    let segments = f64::from(4 * CIRCLE_QUADRANT_SEGMENTS);
    let circumscribed = r / (std::f64::consts::PI / segments).cos();
    centre.buffer(circumscribed, CIRCLE_QUADRANT_SEGMENTS).ok()
}

/// Welzl's minimum enclosing circle, iteratively, over a deterministic shuffle
/// of the points (expected linear time). `(centre x, centre y, radius)`.
fn welzl(pts: &mut [(f64, f64)]) -> (f64, f64, f64) {
    // A fixed-seed LCG shuffle: deterministic results, no adversarial order.
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    for i in (1..pts.len()).rev() {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let j = (seed >> 33) as usize % (i + 1);
        pts.swap(i, j);
    }
    let scale = pts
        .iter()
        .fold(1.0f64, |m, &(x, y)| m.max(x.abs()).max(y.abs()));
    let eps = 1e-12 * scale;
    let inside = |c: (f64, f64, f64), p: (f64, f64)| (p.0 - c.0).hypot(p.1 - c.1) <= c.2 + eps;
    let two = |a: (f64, f64), b: (f64, f64)| {
        (
            (a.0 + b.0) / 2.0,
            (a.1 + b.1) / 2.0,
            (a.0 - b.0).hypot(a.1 - b.1) / 2.0,
        )
    };
    let three = |a: (f64, f64), b: (f64, f64), c: (f64, f64)| {
        let (bx, by) = (b.0 - a.0, b.1 - a.1);
        let (cx, cy) = (c.0 - a.0, c.1 - a.1);
        let d = 2.0 * (bx * cy - by * cx);
        if d.abs() <= f64::EPSILON * scale * scale {
            // Collinear: the circle on the two farthest apart.
            let candidates = [two(a, b), two(a, c), two(b, c)];
            return candidates
                .into_iter()
                .fold((0.0, 0.0, -1.0), |m, c| if c.2 > m.2 { c } else { m });
        }
        let (b2, c2) = (bx * bx + by * by, cx * cx + cy * cy);
        let ux = (cy * b2 - by * c2) / d;
        let uy = (bx * c2 - cx * b2) / d;
        (a.0 + ux, a.1 + uy, ux.hypot(uy))
    };
    let mut c = (pts[0].0, pts[0].1, 0.0);
    for (i, &pi) in pts.iter().enumerate().skip(1) {
        if inside(c, pi) {
            continue;
        }
        c = (pi.0, pi.1, 0.0);
        for (j, &pj) in pts[..i].iter().enumerate() {
            if inside(c, pj) {
                continue;
            }
            c = two(pi, pj);
            for &pk in &pts[..j] {
                if !inside(c, pk) {
                    c = three(pi, pj, pk);
                }
            }
        }
    }
    c
}

/// `geof:boundingCircle(geom)` — see [`minimum_bounding_circle`].
fn fn_bounding_circle(args: &[Term]) -> Option<Term> {
    let circle = minimum_bounding_circle(&parse_one_geom(args)?)?;
    geometry_to_literal_like(&circle, args.first()?)
}

/// The target ratio `geof:concaveHull` and `geof:aggConcaveHull` use when none
/// is given: halfway between the most concave hull (0) and the convex hull (1).
pub const DEFAULT_CONCAVE_HULL_RATIO: f64 = 0.5;

/// A concave hull (GEOS 3.11's `GEOSConcaveHull`, no holes): `ratio` 1 is the
/// convex hull, 0 the most concave hull of the positions.
pub(super) fn concave_hull(geom: &impl Geom, ratio: f64) -> Option<GeosGeometry> {
    geom.concave_hull(ratio, false).ok()
}

/// `geof:concaveHull(geom, targetPercent)` — a concave hull enclosing the
/// geometry. `targetPercent` is a number from 0 (the most concave hull, every
/// edge as short as it can be) to 1 (the convex hull), as in PostGIS 3.3+ and
/// GEOS; outside that range the result is unbound. Without it the ratio is
/// [`DEFAULT_CONCAVE_HULL_RATIO`]. Holes are not allowed.
fn fn_concave_hull(args: &[Term]) -> Option<Term> {
    let ratio = match args.get(1) {
        None => DEFAULT_CONCAVE_HULL_RATIO,
        Some(t) => radius_arg(Some(t)).filter(|r| (0.0..=1.0).contains(r))?,
    };
    let hull = concave_hull(&parse_one_geom(args)?, ratio)?;
    geometry_to_literal_like(&hull, args.first()?)
}

/// A planar measure (length or perimeter) in `units`, following `geof:distance`'s
/// unit rules; `metric` is the geodesic measure in metres for a linear unit on a
/// geographic CRS.
fn planar_measure(
    args: &[Term],
    planar: fn(&GeosGeometry) -> Option<f64>,
    metric: fn(&geo::Geometry<f64>) -> f64,
) -> Option<Term> {
    let term = args.first()?;
    let units = units_arg(args.get(1))?;
    let crs = literal_crs(term);
    let geographic = crs.is_some_and(is_geographic);
    if let (Some(Uom::Linear(metres)), true) = (units, geographic) {
        let g = geodesic::literal_to_crs84(term)?;
        return Some(double_literal(metric(&g) / metres));
    }
    let per_unit = match units {
        None | Some(Uom::Unity) => 1.0,
        Some(Uom::Linear(metres)) if crs.is_some() => metres,
        Some(Uom::Angular(degrees)) if geographic => degrees,
        _ => return None,
    };
    Some(double_literal(planar(&parse_one_geom(args)?)? / per_unit))
}

/// `geof:length(geom, units)` — the length of the lines and of a polygon's
/// rings (0 for points), in `units` as `geof:distance` reads them: geodesic for
/// a linear unit on a geographic CRS, planar on a projected one.
fn fn_length(args: &[Term]) -> Option<Term> {
    planar_measure(args, |g| g.length().ok(), geodesic::metric_length)
}

/// `geof:perimeter(geom, units)` — the length of a polygon's rings, holes
/// included; for a non-areal geometry its length (GeoSPARQL 1.1), so the same
/// number as `geof:length`, as `geof:metricPerimeter` is `geof:metricLength`'s.
fn fn_perimeter(args: &[Term]) -> Option<Term> {
    planar_measure(args, |g| g.length().ok(), geodesic::metric_length)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxrdf::{Literal, NamedNode};

    fn wkt_term(wkt: &str) -> Term {
        Term::Literal(Literal::new_typed_literal(
            wkt,
            NamedNode::new_unchecked(vocab::WKT_LITERAL),
        ))
    }

    fn double_term(val: f64) -> Term {
        Term::Literal(Literal::new_typed_literal(
            val.to_string(),
            NamedNode::new_unchecked("http://www.w3.org/2001/XMLSchema#double"),
        ))
    }

    #[test]
    fn test_sf_contains_point_in_polygon() {
        let poly = wkt_term("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))");
        let point = wkt_term("POINT(5 5)");
        let result = sf_contains(&[poly, point]);
        assert_eq!(result, Some(boolean_literal(true)));
    }

    #[test]
    fn test_sf_contains_point_outside() {
        let poly = wkt_term("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))");
        let point = wkt_term("POINT(15 15)");
        let result = sf_contains(&[poly, point]);
        assert_eq!(result, Some(boolean_literal(false)));
    }

    #[test]
    fn test_sf_intersects() {
        let poly1 = wkt_term("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))");
        let poly2 = wkt_term("POLYGON((5 5, 15 5, 15 15, 5 15, 5 5))");
        let result = sf_intersects(&[poly1, poly2]);
        assert_eq!(result, Some(boolean_literal(true)));
    }

    #[test]
    fn test_sf_disjoint() {
        let poly1 = wkt_term("POLYGON((0 0, 1 0, 1 1, 0 1, 0 0))");
        let poly2 = wkt_term("POLYGON((5 5, 6 5, 6 6, 5 6, 5 5))");
        let result = sf_disjoint(&[poly1, poly2]);
        assert_eq!(result, Some(boolean_literal(true)));
    }

    #[test]
    fn test_sf_equals() {
        let g1 = wkt_term("POINT(1 2)");
        let g2 = wkt_term("POINT(1 2)");
        let result = sf_equals(&[g1, g2]);
        assert_eq!(result, Some(boolean_literal(true)));
    }

    #[test]
    fn test_sf_within() {
        let point = wkt_term("POINT(5 5)");
        let poly = wkt_term("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))");
        let result = sf_within(&[point, poly]);
        assert_eq!(result, Some(boolean_literal(true)));
    }

    #[test]
    fn test_sf_touches() {
        let line = wkt_term("LINESTRING(0 0, 1 1)");
        let point = wkt_term("POINT(0 0)");
        let result = sf_touches(&[point, line]);
        assert_eq!(result, Some(boolean_literal(true)));
    }

    #[test]
    fn test_distance() {
        let p1 = wkt_term("POINT(0 0)");
        let p2 = wkt_term("POINT(3 4)");
        let result = fn_distance(&[p1, p2]);
        if let Some(Term::Literal(lit)) = result {
            let dist: f64 = lit.value().parse().unwrap();
            assert!((dist - 5.0).abs() < 1e-10);
        } else {
            panic!("Expected double literal");
        }
    }

    #[test]
    fn test_area() {
        let poly = wkt_term("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))");
        let result = fn_area(&[poly]);
        if let Some(Term::Literal(lit)) = result {
            let area: f64 = lit.value().parse().unwrap();
            assert!((area - 100.0).abs() < 1e-10);
        } else {
            panic!("Expected double literal");
        }
    }

    #[test]
    fn test_convex_hull() {
        let points = wkt_term("MULTIPOINT((0 0), (10 0), (5 10))");
        let result = fn_convex_hull(&[points]);
        assert!(result.is_some());
        if let Some(Term::Literal(lit)) = result {
            assert!(lit.value().contains("POLYGON"));
        }
    }

    #[test]
    fn test_buffer() {
        let point = wkt_term("POINT(0 0)");
        let radius = double_term(1.0);
        let result = fn_buffer(&[point, radius]);
        assert!(result.is_some());
        if let Some(Term::Literal(lit)) = result {
            assert!(lit.value().contains("POLYGON"));
        }
    }

    #[test]
    fn test_intersection() {
        let poly1 = wkt_term("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))");
        let poly2 = wkt_term("POLYGON((5 5, 15 5, 15 15, 5 15, 5 5))");
        let result = fn_intersection(&[poly1, poly2]);
        assert!(result.is_some());
    }

    #[test]
    fn test_union_geom() {
        let poly1 = wkt_term("POLYGON((0 0, 5 0, 5 5, 0 5, 0 0))");
        let poly2 = wkt_term("POLYGON((3 3, 8 3, 8 8, 3 8, 3 3))");
        let result = fn_union(&[poly1, poly2]);
        assert!(result.is_some());
    }

    #[test]
    fn test_envelope() {
        let line = wkt_term("LINESTRING(0 0, 5 10, 10 0)");
        let result = fn_envelope(&[line]);
        assert!(result.is_some());
    }

    #[test]
    fn test_eh_disjoint() {
        let poly1 = wkt_term("POLYGON((0 0, 1 0, 1 1, 0 1, 0 0))");
        let poly2 = wkt_term("POLYGON((5 5, 6 5, 6 6, 5 6, 5 5))");
        let result = eh_disjoint(&[poly1, poly2]);
        assert_eq!(result, Some(boolean_literal(true)));
    }

    #[test]
    fn test_rcc8_dc() {
        let poly1 = wkt_term("POLYGON((0 0, 1 0, 1 1, 0 1, 0 0))");
        let poly2 = wkt_term("POLYGON((5 5, 6 5, 6 6, 5 6, 5 5))");
        let result = rcc8_dc(&[poly1, poly2]);
        assert_eq!(result, Some(boolean_literal(true)));
    }

    #[test]
    fn test_get_srid_with_crs() {
        let term = Term::Literal(Literal::new_typed_literal(
            "<http://www.opengis.net/def/crs/EPSG/0/4326> POINT(1 2)",
            NamedNode::new_unchecked(vocab::WKT_LITERAL),
        ));
        let result = fn_get_srid(&[term]);
        if let Some(Term::NamedNode(nn)) = result {
            assert_eq!(nn.as_str(), "http://www.opengis.net/def/crs/EPSG/0/4326");
        } else {
            panic!("Expected NamedNode");
        }
    }

    #[test]
    fn test_get_srid_of_gml_and_geojson() {
        // A GML literal reports its srsName, in the canonical IRI form.
        let gml = Term::Literal(Literal::new_typed_literal(
            "<gml:Point srsName='urn:ogc:def:crs:EPSG::4326'><gml:pos>1 2</gml:pos></gml:Point>",
            NamedNode::new_unchecked(vocab::GML_LITERAL),
        ));
        assert_eq!(
            fn_get_srid(&[gml]),
            Some(Term::NamedNode(NamedNode::new_unchecked(
                "http://www.opengis.net/def/crs/EPSG/0/4326"
            )))
        );
        // GML without one, and GeoJSON always, are CRS84.
        let bare_gml = Term::Literal(Literal::new_typed_literal(
            "<gml:Point><gml:pos>1 2</gml:pos></gml:Point>",
            NamedNode::new_unchecked(vocab::GML_LITERAL),
        ));
        let json = Term::Literal(Literal::new_typed_literal(
            r#"{"type":"Point","coordinates":[1,2]}"#,
            NamedNode::new_unchecked(vocab::GEOJSON_LITERAL),
        ));
        for term in [bare_gml, json] {
            assert_eq!(
                fn_get_srid(&[term]),
                Some(Term::NamedNode(NamedNode::new_unchecked(vocab::CRS84)))
            );
        }
    }

    #[test]
    fn test_as_geojson_reprojects_to_crs84() {
        let rd = Term::Literal(Literal::new_typed_literal(
            "<http://www.opengis.net/def/crs/EPSG/0/28992> POINT(187420 428470)",
            NamedNode::new_unchecked(vocab::WKT_LITERAL),
        ));
        let Some(Term::Literal(out)) = fn_as_geojson(&[rd]) else {
            panic!("a literal")
        };
        assert_eq!(out.datatype().as_str(), vocab::GEOJSON_LITERAL);
        let json: serde_json::Value = serde_json::from_str(out.value()).unwrap();
        assert_eq!(json["type"], "Point");
        let (lon, lat) = (
            json["coordinates"][0].as_f64().unwrap(),
            json["coordinates"][1].as_f64().unwrap(),
        );
        assert!(
            (lon - 5.86).abs() < 0.05 && (lat - 51.85).abs() < 0.05,
            "{json}"
        );
    }

    #[test]
    fn test_get_srid_default() {
        let term = wkt_term("POINT(1 2)");
        let result = fn_get_srid(&[term]);
        if let Some(Term::NamedNode(nn)) = result {
            assert_eq!(nn.as_str(), vocab::CRS84);
        } else {
            panic!("Expected NamedNode");
        }
    }

    #[test]
    fn test_all_functions_registered() {
        let fns = all_functions();
        // We should have at least 35 functions registered
        assert!(
            fns.len() >= 35,
            "Expected >= 35 functions, got {}",
            fns.len()
        );

        // Verify some key function IRIs are present
        let iris: Vec<String> = fns
            .iter()
            .map(|(iri, _)| iri.as_str().to_string())
            .collect();
        assert!(iris.contains(&vocab::SF_CONTAINS.to_string()));
        assert!(iris.contains(&vocab::SF_INTERSECTS.to_string()));
        assert!(iris.contains(&vocab::DISTANCE.to_string()));
        assert!(iris.contains(&vocab::BUFFER.to_string()));
        assert!(iris.contains(&vocab::CONVEX_HULL.to_string()));
        assert!(iris.contains(&vocab::RCC8_DC.to_string()));
        assert!(iris.contains(&vocab::EH_CONTAINS.to_string()));
        for metric in [
            vocab::METRIC_DISTANCE,
            vocab::METRIC_AREA,
            vocab::METRIC_LENGTH,
            vocab::METRIC_PERIMETER,
            vocab::METRIC_BUFFER,
        ] {
            assert!(iris.contains(&metric.to_string()), "{metric}");
        }
    }

    fn uom(unit: &str) -> Term {
        Term::NamedNode(NamedNode::new_unchecked(unit))
    }

    fn num(t: Option<Term>) -> f64 {
        match t {
            Some(Term::Literal(l)) => l.value().parse().unwrap(),
            other => panic!("expected a number, got {other:?}"),
        }
    }

    #[test]
    fn distance_in_metres_on_crs84_is_geodesic() {
        // JFK–LHR, GeographicLib's example: 5 551 759.400 m.
        let (a, b) = (wkt_term("POINT(-73.8 40.6)"), wkt_term("POINT(-0.5 51.6)"));
        let m = num(fn_distance(&[a.clone(), b.clone(), uom(vocab::METRE)]));
        assert!((m - 5_551_759.400).abs() < 0.001, "{m}");
        let km = num(fn_distance(&[a.clone(), b.clone(), uom(vocab::KILOMETRE)]));
        assert!((km - m / 1000.0).abs() < 1e-9);
        // No unit, or an angular one: planar degrees, as before.
        let planar = num(fn_distance(&[a.clone(), b.clone()]));
        let degrees = num(fn_distance(&[a.clone(), b.clone(), uom(vocab::DEGREE)]));
        assert!((planar - 73.3f64.hypot(11.0)).abs() < 1e-9);
        assert_eq!(planar, degrees);
        let radians = num(fn_distance(&[a, b, uom(vocab::RADIAN)]));
        assert!((radians - planar.to_radians()).abs() < 1e-12);
    }

    #[test]
    fn buffer_radius_units() {
        let rd = Term::Literal(Literal::new_typed_literal(
            "<http://www.opengis.net/def/crs/EPSG/0/28992> POINT(155000 463000)",
            NamedNode::new_unchecked(vocab::WKT_LITERAL),
        ));
        let area = |t: Option<Term>| parse_wkt_literal(&t.unwrap()).unwrap().area().unwrap();
        // A projected CRS: a kilometre is 1000 of its metres.
        let km = area(fn_buffer(&[
            rd.clone(),
            double_term(1.0),
            uom(vocab::KILOMETRE),
        ]));
        let m = area(fn_buffer(&[rd, double_term(1000.0), uom(vocab::METRE)]));
        assert!((km - m).abs() < 1e-6, "{km} vs {m}");
        // CRS84 with metres: a geodesic buffer — an ellipse of ~0.029° × 0.018°
        // at 52°N, ~4.1e-4 square degrees — not a disc of 1000 degrees.
        let geo = area(fn_buffer(&[
            wkt_term("POINT(5 52)"),
            double_term(1000.0),
            uom(vocab::METRE),
        ]));
        assert!(geo > 3.5e-4 && geo < 4.5e-4, "{geo}");
    }
}
