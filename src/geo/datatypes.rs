//! GeoSPARQL datatype handling: parsing and serialization of WKT/GML geometry literals.
//!
//! Supports the `geo:wktLiteral` datatype as defined in GeoSPARQL 1.1,
//! including optional CRS URI prefix: `<http://...crs...> POINT(0 0)`

use geos::{Geom, Geometry as GeosGeometry};
use oxrdf::{Literal, NamedNode, Term};
use tracing::trace;

use super::vocabulary;

/// Parse a `geo:wktLiteral` from an oxrdf Term into a GEOS Geometry.
///
/// The WKT literal may optionally have a CRS URI prefix:
///   `<http://www.opengis.net/def/crs/EPSG/0/4326> POINT(1.0 2.0)`
/// or just plain WKT:
///   `POINT(1.0 2.0)`
pub fn parse_wkt_literal(term: &Term) -> Option<GeosGeometry> {
    let literal = match term {
        Term::Literal(lit) => lit,
        _ => return None,
    };

    // Check the datatype
    let datatype = literal.datatype();
    let is_wkt = datatype.as_str() == vocabulary::WKT_LITERAL;
    let is_string = datatype.as_str() == "http://www.w3.org/2001/XMLSchema#string";

    // Accept geo:wktLiteral or plain string (for convenience)
    if !is_wkt && !is_string {
        return None;
    }

    let value = literal.value();
    let wkt_str = extract_wkt(value);

    trace!("Parsing WKT: {}", wkt_str);

    GeosGeometry::new_from_wkt(wkt_str).ok()
}

/// Extract the WKT portion from a geo:wktLiteral value,
/// stripping any CRS URI prefix.
///
/// Input: `<http://www.opengis.net/def/crs/EPSG/0/4326> POINT(1 2)`
/// Output: `POINT(1 2)`
fn extract_wkt(value: &str) -> &str {
    let trimmed = value.trim();
    if trimmed.starts_with('<') {
        // Find the closing '>' and skip past it
        if let Some(end) = trimmed.find('>') {
            trimmed[end + 1..].trim()
        } else {
            trimmed
        }
    } else {
        trimmed
    }
}

/// Extract the CRS URI from a geo:wktLiteral value, if present.
///
/// Input: `<http://www.opengis.net/def/crs/EPSG/0/4326> POINT(1 2)`
/// Output: `Some("http://www.opengis.net/def/crs/EPSG/0/4326")`
pub fn extract_crs(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.starts_with('<') {
        let end = trimmed.find('>')?;
        Some(&trimmed[1..end])
    } else {
        None
    }
}

/// Serialize a GEOS Geometry back to a `geo:wktLiteral` Term.
pub fn geometry_to_wkt_literal(geom: &GeosGeometry) -> Option<Term> {
    let wkt = geom.to_wkt().ok()?;
    // Clean up GEOS WKT output (sometimes has extra precision)
    let literal = Literal::new_typed_literal(wkt, NamedNode::new_unchecked(vocabulary::WKT_LITERAL));
    Some(Term::Literal(literal))
}

/// Create an xsd:boolean literal Term.
pub fn boolean_literal(value: bool) -> Term {
    Term::Literal(Literal::new_typed_literal(
        if value { "true" } else { "false" },
        NamedNode::new_unchecked("http://www.w3.org/2001/XMLSchema#boolean"),
    ))
}

/// Create an xsd:double literal Term.
pub fn double_literal(value: f64) -> Term {
    Term::Literal(Literal::new_typed_literal(
        value.to_string(),
        NamedNode::new_unchecked("http://www.w3.org/2001/XMLSchema#double"),
    ))
}

/// Parse a units-of-measure IRI from a Term, returning a scale factor
/// relative to the geometry's native units.
pub fn parse_uom(term: &Term) -> Option<f64> {
    match term {
        Term::NamedNode(nn) => {
            match nn.as_str() {
                s if s == vocabulary::METRE => Some(1.0),
                s if s == vocabulary::DEGREE => Some(1.0), // assume CRS84 in degrees
                s if s == vocabulary::RADIAN => Some(std::f64::consts::PI / 180.0),
                s if s == vocabulary::UNITY => Some(1.0),
                _ => Some(1.0), // default: pass through
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_wkt_plain() {
        assert_eq!(extract_wkt("POINT(1 2)"), "POINT(1 2)");
    }

    #[test]
    fn test_extract_wkt_with_crs() {
        let input = "<http://www.opengis.net/def/crs/EPSG/0/4326> POINT(1 2)";
        assert_eq!(extract_wkt(input), "POINT(1 2)");
    }

    #[test]
    fn test_extract_crs() {
        let input = "<http://www.opengis.net/def/crs/EPSG/0/4326> POINT(1 2)";
        assert_eq!(
            extract_crs(input),
            Some("http://www.opengis.net/def/crs/EPSG/0/4326")
        );
    }

    #[test]
    fn test_extract_crs_none() {
        assert_eq!(extract_crs("POINT(1 2)"), None);
    }

    #[test]
    fn test_parse_wkt_literal() {
        let term = Term::Literal(Literal::new_typed_literal(
            "POINT(1.0 2.0)",
            NamedNode::new_unchecked(vocabulary::WKT_LITERAL),
        ));
        let geom = parse_wkt_literal(&term).expect("Should parse POINT");
        assert!(!geom.is_empty().unwrap());
    }

    #[test]
    fn test_roundtrip_wkt() {
        let term = Term::Literal(Literal::new_typed_literal(
            "POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))",
            NamedNode::new_unchecked(vocabulary::WKT_LITERAL),
        ));
        let geom = parse_wkt_literal(&term).expect("Should parse POLYGON");
        let output = geometry_to_wkt_literal(&geom).expect("Should serialize");
        // The output should still be a wktLiteral
        if let Term::Literal(lit) = &output {
            assert_eq!(lit.datatype().as_str(), vocabulary::WKT_LITERAL);
            assert!(lit.value().contains("POLYGON"));
        } else {
            panic!("Expected literal");
        }
    }
}
