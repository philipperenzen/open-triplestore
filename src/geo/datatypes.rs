//! GeoSPARQL datatype handling: parsing and serialization of geometry literals.
//!
//! Four serialisations are geometries here: `geo:wktLiteral` (with its
//! optional CRS URI prefix, `<http://...crs...> POINT(0 0)`), `geo:gmlLiteral`,
//! `geo:geoJSONLiteral` (RFC 7946 — always CRS84) and `geo:kmlLiteral` (KML 2.2
//! — always CRS84). A plain string is read as WKT for convenience. GML, GeoJSON
//! and KML are translated to WKT, so every one of them reaches GEOS by the same
//! path ([`literal_wkt`]); Z survives the translation.
//!
//! A geometry result is written back in its first operand's serialisation and
//! CRS ([`geometry_to_literal_like`], GeoSPARQL 1.1 §10.9.1): a buffer of a GML
//! literal is a GML literal with the same `srsName`.
//!
//! A literal's CRS comes from one place, [`literal_crs_uri`]: a WKT literal's
//! `<crs>` prefix or a GML literal's `srsName`, and CRS84 otherwise. Every
//! function — topology, constructive, `getSRID`, `transform`, the metric family
//! and `aggUnion` — reads it there, so they all agree about one literal.
//!
//! An empty `geo:wktLiteral`, `geo:gmlLiteral`, `geo:geoJSONLiteral` or
//! `geo:kmlLiteral` is the empty geometry (GeoSPARQL 1.1 Req 17, 21, 27 and 32).

use std::borrow::Cow;

use dashmap::DashMap;
use geos::{CoordDimensions, Geom, Geometry as GeosGeometry, WKBWriter, WKTWriter};
use oxrdf::{Literal, NamedNode, Term};
use std::sync::OnceLock;
use tracing::trace;

use super::crs::{normalise_crs_uri, Crs};
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
        if let Some(wkb) = geometry_wkb(&g) {
            cache.insert(wkt_str.to_string(), wkb);
        }
    }
    Some(g)
}

/// A geometry's WKB, Z kept on every supported GEOS: GEOS 3.11 writes two
/// dimensions unless told otherwise (3.12 keeps them), and a cached parse
/// must not lose a Z the literal has.
fn geometry_wkb(g: &GeosGeometry) -> Option<Vec<u8>> {
    let mut writer = WKBWriter::new().ok()?;
    if matches!(writer.get_out_dimension(), Ok(CoordDimensions::TwoD)) {
        writer.set_output_dimension(CoordDimensions::ThreeD);
    }
    writer.write_wkb(g).ok()
}

const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
const XSD_ANY_URI: &str = "http://www.w3.org/2001/XMLSchema#anyURI";

/// The WKT of the empty geometry.
const EMPTY_WKT: &str = "GEOMETRYCOLLECTION EMPTY";

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
///
/// An empty WKT, GML or GeoJSON literal (a WKT literal may still carry its
/// `<crs>`) is the empty geometry. An empty plain string is not a geometry.
pub fn literal_wkt(term: &Term) -> Option<Cow<'_, str>> {
    let Term::Literal(literal) = term else {
        return None;
    };
    let value = literal.value();
    match literal.datatype().as_str() {
        vocabulary::WKT_LITERAL => match extract_wkt(value) {
            "" => Some(Cow::Borrowed(EMPTY_WKT)),
            wkt => Some(Cow::Borrowed(wkt)),
        },
        XSD_STRING => Some(Cow::Borrowed(extract_wkt(value))),
        vocabulary::GML_LITERAL if value.trim().is_empty() => Some(Cow::Borrowed(EMPTY_WKT)),
        vocabulary::GML_LITERAL => super::gml::gml_to_wkt(value).map(Cow::Owned),
        vocabulary::GEOJSON_LITERAL if value.trim().is_empty() => Some(Cow::Borrowed(EMPTY_WKT)),
        vocabulary::GEOJSON_LITERAL => super::geojson::geojson_to_wkt(value).map(Cow::Owned),
        vocabulary::KML_LITERAL if value.trim().is_empty() => Some(Cow::Borrowed(EMPTY_WKT)),
        vocabulary::KML_LITERAL => super::kml::kml_to_wkt(value).map(Cow::Owned),
        _ => None,
    }
}

/// A geometry literal's serialisation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Serialisation {
    /// `geo:wktLiteral` (and a plain string read as WKT).
    Wkt,
    Gml,
    GeoJson,
    Kml,
}

impl Serialisation {
    /// The serialisation of a geometry literal term; WKT for anything else.
    pub fn of(term: &Term) -> Serialisation {
        match term {
            Term::Literal(l) => match l.datatype().as_str() {
                vocabulary::GML_LITERAL => Serialisation::Gml,
                vocabulary::GEOJSON_LITERAL => Serialisation::GeoJson,
                vocabulary::KML_LITERAL => Serialisation::Kml,
                _ => Serialisation::Wkt,
            },
            _ => Serialisation::Wkt,
        }
    }

    /// Whether the serialisation is CRS84 by definition (GeoJSON, KML).
    pub fn is_crs84_only(self) -> bool {
        matches!(self, Serialisation::GeoJson | Serialisation::Kml)
    }
}

/// A geometry as a literal of `serialisation` in the CRS `crs_uri` (`None`:
/// CRS84). The geometry is already in that CRS: nothing is reprojected here.
/// A GeoJSON or KML result outside CRS84 cannot exist, so it is written as a
/// WKT literal with the CRS instead.
pub fn geometry_to_literal(
    geom: &GeosGeometry,
    serialisation: Serialisation,
    crs_uri: Option<&str>,
) -> Option<Term> {
    let crs84 = crs_uri.is_none_or(|uri| Crs::from_uri(uri) == Some(Crs::Wgs84));
    let typed = |lexical: String, datatype: &str| {
        Term::Literal(Literal::new_typed_literal(
            lexical,
            NamedNode::new_unchecked(datatype),
        ))
    };
    let identity = |x: f64, y: f64| Some((x, y));
    match serialisation {
        Serialisation::Gml => Some(typed(
            super::gml::geometry_to_gml(geom, crs_uri)?,
            vocabulary::GML_LITERAL,
        )),
        Serialisation::GeoJson if crs84 => Some(typed(
            super::geojson::geometry_to_geojson(geom, &identity)?.to_string(),
            vocabulary::GEOJSON_LITERAL,
        )),
        Serialisation::Kml if crs84 => Some(typed(
            super::kml::geometry_to_kml(geom, &identity)?,
            vocabulary::KML_LITERAL,
        )),
        _ => geometry_to_wkt_literal_in(geom, crs_uri),
    }
}

/// A geometry result as a literal like `template`, its first operand: the same
/// serialisation, and the same CRS spelled the same way (a WKT prefix, or a GML
/// `srsName`, as written). GeoSPARQL 1.1 §10.9.1: a function returning a
/// geometry returns it in the serialisation and SRS of its first argument.
pub fn geometry_to_literal_like(geom: &GeosGeometry, template: &Term) -> Option<Term> {
    let serialisation = Serialisation::of(template);
    let crs = match (serialisation, template) {
        (Serialisation::Gml, Term::Literal(l)) => super::gml::gml_srs_name(l.value()),
        (Serialisation::Wkt, Term::Literal(l)) => extract_crs(l.value()).map(str::to_string),
        _ => None,
    };
    geometry_to_literal(geom, serialisation, crs.as_deref())
}

/// The coordinate dimensions a geometry literal is written with: whether it has
/// Z and whether it has M. Read from the lexical form (the `wkt` crate sees a
/// `POINT M`/`POINT ZM` that GEOS 3.11 cannot), falling back to GEOS for WKT the
/// `wkt` crate does not read. GML, GeoJSON and KML have no M.
pub fn literal_dimensions(term: &Term) -> Option<(bool, bool)> {
    use std::str::FromStr;
    let wkt = literal_wkt(term)?;
    if let Ok(parsed) = wkt::Wkt::<f64>::from_str(&wkt) {
        return Some(match parsed.dimension() {
            wkt::types::Dimension::XY => (false, false),
            wkt::types::Dimension::XYZ => (true, false),
            wkt::types::Dimension::XYM => (false, true),
            wkt::types::Dimension::XYZM => (true, true),
        });
    }
    let g = parse_wkt_cached(&wkt)?;
    Some((g.has_z().ok()?, false))
}

/// The CRS IRI a geometry literal names — `None` meaning GeoSPARQL's default,
/// CRS84. This is the one place a literal's CRS is read.
///
/// * `geo:wktLiteral` (and the plain strings accepted for convenience): the
///   `<crs>` prefix, as written.
/// * `geo:gmlLiteral`: the `srsName` of its geometry, normalised to the
///   `http://www.opengis.net/def/crs/…` form (`EPSG:28992`,
///   `urn:ogc:def:crs:EPSG::28992` and the rest name the same CRS). Its axis
///   order is the CRS's, as for WKT: an EPSG:4326 `gml:pos` is latitude first.
/// * `geo:geoJSONLiteral`: none — RFC 7946 is CRS84 by definition.
pub fn literal_crs_uri(term: &Term) -> Option<Cow<'_, str>> {
    let Term::Literal(l) = term else {
        return None;
    };
    match l.datatype().as_str() {
        vocabulary::WKT_LITERAL | XSD_STRING => extract_crs(l.value()).map(Cow::Borrowed),
        vocabulary::GML_LITERAL => super::gml::gml_srs_name(l.value())
            .map(|name| Cow::Owned(normalise_crs_uri(&name).into_owned())),
        // GeoJSON and KML are CRS84 by definition.
        _ => None,
    }
}

/// The CRS a geometry literal is written in ([`literal_crs_uri`], CRS84 when it
/// names none), or `None` for a CRS this build cannot reproject.
pub fn literal_crs(term: &Term) -> Option<Crs> {
    match literal_crs_uri(term) {
        Some(uri) => Crs::from_uri(&uri),
        None => Some(Crs::Wgs84),
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
    let wkt = geometry_wkt(geom)?;
    let lexical = match crs_uri {
        Some(uri) => format!("<{uri}> {wkt}"),
        None => wkt,
    };
    let literal =
        Literal::new_typed_literal(lexical, NamedNode::new_unchecked(vocabulary::WKT_LITERAL));
    Some(Term::Literal(literal))
}

/// A geometry's WKT, the same on every supported GEOS: trailing zeros trimmed
/// and Z kept. GEOS 3.12 made both the default; GEOS 3.11 wrote two dimensions
/// and every digit, so set them when the writer starts out two-dimensional.
pub fn geometry_wkt(geom: &GeosGeometry) -> Option<String> {
    let mut writer = WKTWriter::new().ok()?;
    writer.set_trim(true);
    if matches!(writer.get_out_dimension(), Ok(CoordDimensions::TwoD)) {
        writer.set_output_dimension(CoordDimensions::ThreeD);
    }
    writer.write(geom).ok()
}

/// Reproject a geometry from `from` to `to` (Z kept), or `None` when any
/// coordinate does not transform — it is never copied through unchanged.
pub fn reproject_geometry(geom: &GeosGeometry, from: Crs, to: Crs) -> Option<GeosGeometry> {
    if from == to {
        return Some(Clone::clone(geom));
    }
    geom.transform_xy(|x, y| {
        super::crs::transform_xy(from, to, x, y)
            .ok_or_else(|| geos::Error::ImpossibleOperation("outside the CRS's domain".into()))
    })
    .ok()
}

/// A term naming an IRI: an IRI, or an `xsd:anyURI` literal (GeoSPARQL types
/// its CRS and unit arguments as `xsd:anyURI`).
pub fn iri_arg(term: &Term) -> Option<&str> {
    match term {
        Term::NamedNode(nn) => Some(nn.as_str()),
        Term::Literal(l) if l.datatype().as_str() == XSD_ANY_URI => Some(l.value().trim()),
        _ => None,
    }
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

/// A unit of measure as `geof:distance`, `geof:buffer` and `geof:area` use it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Uom {
    /// A length, as metres per unit (metre 1, kilometre 1000, …).
    Linear(f64),
    /// An angle, as degrees per unit (degree 1, radian 180/π).
    Angular(f64),
    /// An area, as square metres per unit (square metre 1, hectare 10 000, …).
    Area(f64),
    /// `uom:unity`: dimensionless — the geometry's own units stand.
    Unity,
}

/// Parse a unit-of-measure argument: an IRI or an `xsd:anyURI` literal naming
/// an OGC unit (`http://www.opengis.net/def/uom/OGC/1.0/…`), a QUDT unit
/// (`http://qudt.org/vocab/unit/…`, the vocabulary GeoSPARQL 1.1 recommends) or
/// an EPSG unit (`http://www.opengis.net/def/uom/EPSG/0/…`). `None` for any
/// other term: the functions then return unbound rather than a number in units
/// the caller did not ask for.
pub fn parse_uom(term: &Term) -> Option<Uom> {
    const QUDT: &str = "http://qudt.org/vocab/unit/";
    const QUDT_HTTPS: &str = "https://qudt.org/vocab/unit/";
    const EPSG: &str = "http://www.opengis.net/def/uom/EPSG/0/";
    const RADIAN_DEGREES: f64 = 180.0 / std::f64::consts::PI;
    let iri = iri_arg(term)?;
    if let Some(unit) = iri
        .strip_prefix(QUDT)
        .or_else(|| iri.strip_prefix(QUDT_HTTPS))
    {
        return Some(match unit {
            "M" => Uom::Linear(1.0),
            "KiloM" => Uom::Linear(1000.0),
            "CentiM" => Uom::Linear(0.01),
            "MilliM" => Uom::Linear(0.001),
            "FT" => Uom::Linear(0.3048),
            "MI" => Uom::Linear(1609.344),
            "MI_N" => Uom::Linear(1852.0),
            "DEG" => Uom::Angular(1.0),
            "RAD" => Uom::Angular(RADIAN_DEGREES),
            "M2" => Uom::Area(1.0),
            "KiloM2" => Uom::Area(1.0e6),
            "HA" => Uom::Area(1.0e4),
            "UNITLESS" => Uom::Unity,
            _ => return None,
        });
    }
    if let Some(code) = iri.strip_prefix(EPSG) {
        return Some(match code {
            "9001" => Uom::Linear(1.0),
            "9002" => Uom::Linear(0.3048),
            "9036" => Uom::Linear(1000.0),
            "9101" => Uom::Angular(RADIAN_DEGREES),
            "9102" => Uom::Angular(1.0),
            _ => return None,
        });
    }
    Some(match iri {
        vocabulary::METRE => Uom::Linear(1.0),
        vocabulary::KILOMETRE => Uom::Linear(1000.0),
        vocabulary::CENTIMETRE => Uom::Linear(0.01),
        vocabulary::MILLIMETRE => Uom::Linear(0.001),
        vocabulary::DEGREE => Uom::Angular(1.0),
        vocabulary::RADIAN => Uom::Angular(RADIAN_DEGREES),
        vocabulary::UNITY => Uom::Unity,
        _ => return None,
    })
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
    fn the_crs_comes_from_the_wkt_prefix_or_the_gml_srs_name() {
        let rd = "<http://www.opengis.net/def/crs/EPSG/0/28992> POINT(1 2)";
        assert_eq!(
            literal_crs_uri(&typed(rd, vocabulary::WKT_LITERAL)).as_deref(),
            Some("http://www.opengis.net/def/crs/EPSG/0/28992")
        );
        // A GML literal's opening tag is not a CRS: its srsName is, normalised.
        let gml = "<gml:Point srsName='EPSG:28992'><gml:pos>1 2</gml:pos></gml:Point>";
        assert_eq!(
            literal_crs_uri(&typed(gml, vocabulary::GML_LITERAL)).as_deref(),
            Some("http://www.opengis.net/def/crs/EPSG/0/28992")
        );
        assert_eq!(
            literal_crs(&typed(gml, vocabulary::GML_LITERAL)),
            Some(Crs::RdNew)
        );
        let bare = "<gml:Point><gml:pos>1 2</gml:pos></gml:Point>";
        assert_eq!(literal_crs_uri(&typed(bare, vocabulary::GML_LITERAL)), None);
        // GeoJSON has none.
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
    fn empty_geometry_literals_are_the_empty_geometry() {
        for (value, datatype) in [
            ("", vocabulary::WKT_LITERAL),
            ("  ", vocabulary::WKT_LITERAL),
            (
                "<http://www.opengis.net/def/crs/EPSG/0/28992>",
                vocabulary::WKT_LITERAL,
            ),
            ("", vocabulary::GML_LITERAL),
            ("", vocabulary::GEOJSON_LITERAL),
        ] {
            let g = parse_wkt_literal(&typed(value, datatype))
                .unwrap_or_else(|| panic!("{value:?}^^{datatype} is a geometry"));
            assert!(g.is_empty().unwrap(), "{value:?}^^{datatype}");
        }
        // An empty plain string is still not a geometry.
        assert!(parse_wkt_literal(&typed("", XSD_STRING)).is_none());
    }

    #[test]
    fn units_are_ogc_qudt_or_epsg_iris_and_nothing_else() {
        let iri = |s: &str| Term::NamedNode(NamedNode::new_unchecked(s));
        assert_eq!(parse_uom(&iri(vocabulary::METRE)), Some(Uom::Linear(1.0)));
        assert_eq!(
            parse_uom(&iri("http://qudt.org/vocab/unit/KiloM")),
            Some(Uom::Linear(1000.0))
        );
        assert_eq!(
            parse_uom(&iri("http://qudt.org/vocab/unit/HA")),
            Some(Uom::Area(1.0e4))
        );
        assert_eq!(
            parse_uom(&iri("http://www.opengis.net/def/uom/EPSG/0/9102")),
            Some(Uom::Angular(1.0))
        );
        // xsd:anyURI literals name units too.
        assert_eq!(
            parse_uom(&typed("http://qudt.org/vocab/unit/M", XSD_ANY_URI)),
            Some(Uom::Linear(1.0))
        );
        assert_eq!(parse_uom(&iri(vocabulary::UNITY)), Some(Uom::Unity));
        for unknown in [
            iri("http://qudt.org/vocab/unit/PARSEC"),
            iri("http://example.org/furlong"),
            typed("http://qudt.org/vocab/unit/M", XSD_STRING),
        ] {
            assert_eq!(parse_uom(&unknown), None, "{unknown}");
        }
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
