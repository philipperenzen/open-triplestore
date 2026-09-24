//! GeoSPARQL datatype handling: parsing and serialization of geometry literals.
//!
//! Three serialisations are geometries here: `geo:wktLiteral` (with its
//! optional CRS URI prefix, `<http://...crs...> POINT(0 0)`), `geo:gmlLiteral`
//! and `geo:geoJSONLiteral` (RFC 7946 — always CRS84). A plain string is read as
//! WKT for convenience. GML and GeoJSON are translated to WKT, so every one of
//! them reaches GEOS by the same path ([`literal_wkt`]).

use std::borrow::Cow;

use dashmap::DashMap;
use geos::{Geom, Geometry as GeosGeometry};
use oxrdf::{Literal, NamedNode, Term};
use std::sync::OnceLock;
use tracing::trace;

use super::vocabulary;

// Process-wide WKT → WKB cache. Profiling (callgrind) shows GeoSPARQL relation
// queries are dominated by **WKT parsing** (`strtod` per coordinate +
// `geos::io::StringTokenizer`), not the geometric computation — and a relation
// query with a constant geometry re-parses that same literal on every candidate
// binding. We memoise the parse as **WKB bytes**, not a `geos::Geometry`: a
// `Vec<u8>` carries no GEOS context, so (unlike caching the geometry) it drops
// safely on any thread at teardown and can be shared process-wide. Decoding WKB
// skips the `strtod`/tokeniser hot path; misses pay one `to_wkb`, amortised across
// repeated bindings and queries over the same geometry.
fn wkb_cache() -> &'static DashMap<String, Vec<u8>> {
    static CACHE: OnceLock<DashMap<String, Vec<u8>>> = OnceLock::new();
    CACHE.get_or_init(DashMap::new)
}
/// Cap the cache; once full, new geometries simply aren't memoised (still parsed).
const WKB_CACHE_CAP: usize = 200_000;

/// Parse a WKT string into a geometry, memoised process-wide as WKB.
fn parse_wkt_cached(wkt_str: &str) -> Option<GeosGeometry> {
    let cache = wkb_cache();
    // Clone the bytes out of the map so the shard lock isn't held during decode.
    if let Some(wkb) = cache.get(wkt_str).map(|r| r.value().clone()) {
        if let Ok(g) = GeosGeometry::new_from_wkb(&wkb) {
            return Some(g);
        }
        // Decode failure: fall through and re-parse from WKT.
    }
    let g = GeosGeometry::new_from_wkt(wkt_str).ok()?;
    if cache.len() < WKB_CACHE_CAP {
        if let Ok(wkb) = g.to_wkb() {
            cache.insert(wkt_str.to_string(), wkb);
        }
    }
    Some(g)
}

const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";

/// Parse a geometry literal — `geo:wktLiteral` (optionally CRS-prefixed,
/// `<http://www.opengis.net/def/crs/EPSG/0/4326> POINT(1.0 2.0)`),
/// `geo:gmlLiteral`, `geo:geoJSONLiteral`, or a plain string read as WKT —
/// into a GEOS Geometry. `None` for anything else, or a malformed literal.
pub fn parse_wkt_literal(term: &Term) -> Option<GeosGeometry> {
    let wkt = literal_wkt(term)?;
    trace!("Parsing WKT: {}", wkt);
    parse_wkt_cached(&wkt)
}

/// The WKT of a geometry literal, whatever its serialisation: a WKT literal
/// (or plain string) without its CRS prefix, a GML literal translated by
/// [`super::gml`], a GeoJSON literal translated by [`super::geojson`]. `None`
/// for a term that is not a geometry literal or does not translate.
pub fn literal_wkt(term: &Term) -> Option<Cow<'_, str>> {
    let Term::Literal(literal) = term else {
        return None;
    };
    let value = literal.value();
    match literal.datatype().as_str() {
        vocabulary::WKT_LITERAL | XSD_STRING => Some(Cow::Borrowed(extract_wkt(value))),
        vocabulary::GML_LITERAL => super::gml::gml_to_wkt(value).map(Cow::Owned),
        vocabulary::GEOJSON_LITERAL => super::geojson::geojson_to_wkt(value).map(Cow::Owned),
        _ => None,
    }
}

/// The CRS URI a geometry literal carries as its `<crs>` prefix, if any —
/// `None` meaning GeoSPARQL's default, CRS84.
///
/// Only `geo:wktLiteral` (and the plain strings accepted for convenience) use
/// the prefix form. A `geo:gmlLiteral` value also starts with `<` — its opening
/// tag — so it is excluded, or the tag would be mistaken for a CRS URI; GML
/// carries its CRS in `srsName`, which this build does not yet read, so it is
/// treated as unspecified. A `geo:geoJSONLiteral` is CRS84 by definition
/// (RFC 7946 has no CRS member).
pub fn literal_crs_uri(term: &Term) -> Option<&str> {
    match term {
        Term::Literal(l)
            if matches!(l.datatype().as_str(), vocabulary::WKT_LITERAL | XSD_STRING) =>
        {
            extract_crs(l.value())
        }
        _ => None,
    }
}

/// Extract the WKT portion from a geo:wktLiteral value,
/// stripping any CRS URI prefix.
///
/// Input: `<http://www.opengis.net/def/crs/EPSG/0/4326> POINT(1 2)`
/// Output: `POINT(1 2)`
pub fn extract_wkt(value: &str) -> &str {
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

/// Serialize a GEOS Geometry back to a `geo:wktLiteral` Term, carrying `crs_uri`
/// as the literal's CRS prefix when one is given.
///
/// The CRS must be threaded through: emitting a bare WKT literal dropped the
/// operand's CRS, so `geof:getSRID(geof:buffer("<…/28992> POINT(…)", 10))`
/// reported CRS84 — silently relabelling RD New metres as degrees, and making
/// the result unusable as an operand for anything else.
pub fn geometry_to_wkt_literal_in(geom: &GeosGeometry, crs_uri: Option<&str>) -> Option<Term> {
    let wkt = geom.to_wkt().ok()?;
    let lexical = match crs_uri {
        Some(uri) => format!("<{uri}> {wkt}"),
        None => wkt,
    };
    let literal =
        Literal::new_typed_literal(lexical, NamedNode::new_unchecked(vocabulary::WKT_LITERAL));
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

    fn typed(value: &str, datatype: &str) -> Term {
        Term::Literal(Literal::new_typed_literal(
            value,
            NamedNode::new_unchecked(datatype),
        ))
    }

    #[test]
    fn geojson_literal_parses_like_its_wkt() {
        let json = typed(
            r#"{"type":"Polygon","coordinates":[[[0,0],[2,0],[2,2],[0,2],[0,0]]]}"#,
            vocabulary::GEOJSON_LITERAL,
        );
        let from_json = parse_wkt_literal(&json).expect("GeoJSON polygon parses");
        let from_wkt = parse_wkt_literal(&typed(
            "POLYGON((0 0, 2 0, 2 2, 0 2, 0 0))",
            vocabulary::WKT_LITERAL,
        ))
        .unwrap();
        assert!(from_json.equals(&from_wkt).unwrap());
        // Malformed GeoJSON is not a geometry.
        assert!(parse_wkt_literal(&typed("{\"type\":", vocabulary::GEOJSON_LITERAL)).is_none());
    }

    #[test]
    fn only_a_wkt_literal_carries_a_crs_prefix() {
        let rd = "<http://www.opengis.net/def/crs/EPSG/0/28992> POINT(1 2)";
        assert_eq!(
            literal_crs_uri(&typed(rd, vocabulary::WKT_LITERAL)),
            Some("http://www.opengis.net/def/crs/EPSG/0/28992")
        );
        // A GML literal's opening tag is not a CRS, and GeoJSON has none.
        let gml = "<gml:Point srsName='EPSG:28992'><gml:pos>1 2</gml:pos></gml:Point>";
        assert_eq!(literal_crs_uri(&typed(gml, vocabulary::GML_LITERAL)), None);
        let json = r#"{"type":"Point","coordinates":[1,2]}"#;
        assert_eq!(
            literal_crs_uri(&typed(json, vocabulary::GEOJSON_LITERAL)),
            None
        );
        assert_eq!(
            literal_wkt(&typed(json, vocabulary::GEOJSON_LITERAL)).as_deref(),
            Some("POINT(1 2)")
        );
    }

    #[test]
    fn test_roundtrip_wkt() {
        let term = Term::Literal(Literal::new_typed_literal(
            "POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))",
            NamedNode::new_unchecked(vocabulary::WKT_LITERAL),
        ));
        let geom = parse_wkt_literal(&term).expect("Should parse POLYGON");
        let output = geometry_to_wkt_literal_in(&geom, None).expect("Should serialize");
        // The output should still be a wktLiteral
        if let Term::Literal(lit) = &output {
            assert_eq!(lit.datatype().as_str(), vocabulary::WKT_LITERAL);
            assert!(lit.value().contains("POLYGON"));
        } else {
            panic!("Expected literal");
        }
    }
}
