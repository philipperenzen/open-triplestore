//! GeoSPARQL function implementations.
//!
//! All functions are registered as custom SPARQL functions via Oxigraph's
//! `QueryOptions::with_custom_function` API. Each function takes oxrdf Terms
//! as arguments, parses geometry WKT literals, performs spatial operations
//! using the GEOS library, and returns result Terms.
//!
//! Implements:
//! - Simple Features (SF) topological relations
//! - Egenhofer topological relations
//! - RCC8 topological relations
//! - Non-topological / constructive functions
//! - Scalar measurement functions

use std::sync::Arc;

use geos::{Geom, Geometry as GeosGeometry};
use oxrdf::{NamedNode, Term};

use super::datatypes::*;
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
    ]
}

/// Helper to construct a (NamedNode, Arc<Fn>) pair.
fn make_fn(iri: &str, f: fn(&[Term]) -> Option<Term>) -> (NamedNode, FnHandler) {
    (NamedNode::new_unchecked(iri), Arc::new(f))
}

// ─── Argument parsing helpers ───

/// Parse two geometry arguments from the term slice.
fn parse_two_geoms(args: &[Term]) -> Option<(GeosGeometry, GeosGeometry)> {
    if args.len() < 2 {
        return None;
    }
    let g1 = parse_wkt_literal(&args[0])?;
    let g2 = parse_wkt_literal(&args[1])?;
    Some((g1, g2))
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
    let g1 = parse_wkt_literal(&args[0])?;
    let g2 = parse_wkt_literal(&args[1])?;
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
    // Use GEOS native covered_by() which handles all geometry type combinations
    // correctly, including polygons with shared boundary edges.
    let result = g1.covered_by(&g2).ok()?;
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
    let g = parse_one_geom(args)?;
    let result = g.boundary().ok()?;
    geometry_to_wkt_literal(&result)
}

fn fn_buffer(args: &[Term]) -> Option<Term> {
    let g = parse_one_geom(args)?;

    // Second arg: radius (xsd:double)
    let radius = match args.get(1) {
        Some(Term::Literal(lit)) => lit.value().parse::<f64>().ok()?,
        _ => return None,
    };

    // Third arg (optional): units IRI — for now we use the raw radius
    // A full implementation would convert units based on the CRS
    let _unit_scale = args.get(2).and_then(parse_uom).unwrap_or(1.0);

    let result = g.buffer(radius, 16).ok()?;
    geometry_to_wkt_literal(&result)
}

fn fn_convex_hull(args: &[Term]) -> Option<Term> {
    let g = parse_one_geom(args)?;
    let result = g.convex_hull().ok()?;
    geometry_to_wkt_literal(&result)
}

fn fn_difference(args: &[Term]) -> Option<Term> {
    let (g1, g2) = parse_two_geoms(args)?;
    let result = g1.difference(&g2).ok()?;
    geometry_to_wkt_literal(&result)
}

fn fn_envelope(args: &[Term]) -> Option<Term> {
    let g = parse_one_geom(args)?;
    let result = g.envelope().ok()?;
    geometry_to_wkt_literal(&result)
}

fn fn_intersection(args: &[Term]) -> Option<Term> {
    let (g1, g2) = parse_two_geoms(args)?;
    let result = g1.intersection(&g2).ok()?;
    geometry_to_wkt_literal(&result)
}

fn fn_sym_difference(args: &[Term]) -> Option<Term> {
    let (g1, g2) = parse_two_geoms(args)?;
    let result = g1.sym_difference(&g2).ok()?;
    geometry_to_wkt_literal(&result)
}

fn fn_union(args: &[Term]) -> Option<Term> {
    let (g1, g2) = parse_two_geoms(args)?;
    let result = g1.union(&g2).ok()?;
    geometry_to_wkt_literal(&result)
}

// ═══════════════════════════════════════════════════════════════
// Scalar Measurement Functions
// ═══════════════════════════════════════════════════════════════

fn fn_distance(args: &[Term]) -> Option<Term> {
    let (g1, g2) = parse_two_geoms(args)?;

    // Third arg (optional): units IRI
    let _unit_scale = args.get(2).and_then(parse_uom).unwrap_or(1.0);

    let dist = g1.distance(&g2).ok()?;
    Some(double_literal(dist))
}

fn fn_area(args: &[Term]) -> Option<Term> {
    let g = parse_one_geom(args)?;
    let area = g.area().ok()?;
    Some(double_literal(area))
}

fn fn_get_srid(args: &[Term]) -> Option<Term> {
    // Extract CRS URI from the WKT literal
    if let Some(Term::Literal(lit)) = args.first() {
        let value = lit.value();
        if let Some(crs) = extract_crs(value) {
            Some(Term::NamedNode(NamedNode::new_unchecked(crs)))
        } else {
            // Default CRS is CRS84
            Some(Term::NamedNode(NamedNode::new_unchecked(vocab::CRS84)))
        }
    } else {
        None
    }
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
    }
}
