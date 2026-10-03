//! Coordinate Reference System registry and closed-form transforms for the CRS
//! used by Dutch infrastructure linked data: **EPSG:28992** (Amersfoort / RD New),
//! **EPSG:4326 / CRS84** (WGS84 geographic), and **EPSG:3857** (Web Mercator).
//!
//! These three are implemented with pure-Rust closed-form approximations rather
//! than binding the PROJ C library, keeping the build self-contained (no system
//! dependency, no CI changes). The RD↔WGS84 conversion uses the well-known
//! Strang-van-Hees / Schreutelkamp approximation (accurate to a few decimetres,
//! ample for visualisation and the conformance fixtures); WGS84↔Web-Mercator is
//! the exact spherical Mercator formula.
//!
//! **Axis order.** Internally every geographic coordinate is `(x = longitude,
//! y = latitude)` — the CRS84 / WKT / GeoJSON convention — so a transformed
//! geometry can feed a `[lng, lat]` map layer directly. Projected CRS use
//! `(x = easting, y = northing)`.
//!
//! **EPSG:4326 is not CRS84.** The OGC registry defines EPSG:4326 with the
//! authority's axis order, `(latitude, longitude)`, and GeoSPARQL says a WKT
//! literal's coordinates are in the order its CRS prescribes. Only the unprefixed
//! default and `OGC/1.3/CRS84` are lon/lat. This module models the two as
//! distinct CRS so a literal carrying the `EPSG/0/4326` prefix is read and
//! written as lat/lon; earlier versions treated both as lon/lat, which
//! transposed every authority-ordered geometry.
//!
//! **Domains.** A transform outside a CRS's domain fails (`None`) rather than
//! returning plausible numbers: a latitude beyond ±90°, Web Mercator beyond its
//! ±85.06° latitude limit, and RD New outside the Netherlands. The RD New
//! polynomials are a local fit around Amersfoort; [`RD_LON`]/[`RD_LAT`] and
//! [`RD_X`]/[`RD_Y`] are EPSG:28992's area of use with about 50 km to spare.

use std::borrow::Cow;
use std::f64::consts::PI;

/// A supported coordinate reference system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Crs {
    /// Amersfoort / RD New (EPSG:28992) — easting/northing in metres.
    RdNew,
    /// WGS84 geographic in `(lon, lat)` axis order — OGC:CRS84, and GeoSPARQL's
    /// default for a literal with no CRS prefix.
    Wgs84,
    /// WGS84 geographic in the authority's `(lat, lon)` axis order — EPSG:4326.
    /// Same datum as [`Crs::Wgs84`]; only the ordinate order differs.
    Epsg4326,
    /// Web Mercator (EPSG:3857) — easting/northing in metres.
    WebMercator,
}

impl Crs {
    /// Resolve a CRS from a CRS URI as used in WKT/GeoSPARQL literals. Recognises the
    /// common EPSG and OGC forms; returns `None` for an unsupported CRS.
    pub fn from_uri(uri: &str) -> Option<Crs> {
        let u = uri.trim_end_matches('>').trim_start_matches('<');
        if u.ends_with("CRS84h") || u.ends_with("CRS84") {
            Some(Crs::Wgs84)
        } else if u.ends_with("/4326") || u.ends_with(":4326") {
            Some(Crs::Epsg4326)
        } else if u.ends_with("/28992") || u.ends_with(":28992")
            // EPSG:7415 = RD New (28992) + NAP height: horizontally identical to
            // RD New; the NAP Z ordinate passes through reprojection unchanged.
            || u.ends_with("/7415") || u.ends_with(":7415")
        {
            Some(Crs::RdNew)
        } else if u.ends_with("/3857") || u.ends_with(":3857") || u.ends_with("/900913") {
            Some(Crs::WebMercator)
        } else {
            None
        }
    }

    /// Canonical CRS URI for this CRS (the form used in GeoSPARQL WKT prefixes).
    ///
    /// [`Crs::Wgs84`] maps to CRS84, not EPSG:4326: labelling lon/lat output
    /// with the EPSG URI would claim the authority's lat/lon order for it.
    pub fn to_uri(self) -> &'static str {
        match self {
            Crs::RdNew => "http://www.opengis.net/def/crs/EPSG/0/28992",
            Crs::Wgs84 => "http://www.opengis.net/def/crs/OGC/1.3/CRS84",
            Crs::Epsg4326 => "http://www.opengis.net/def/crs/EPSG/0/4326",
            Crs::WebMercator => "http://www.opengis.net/def/crs/EPSG/0/3857",
        }
    }
}

/// The canonical `http://www.opengis.net/def/crs/{authority}/{version}/{code}`
/// form of a CRS name as GML writes `srsName`: `EPSG:28992`,
/// `urn:ogc:def:crs:EPSG::28992`, `urn:ogc:def:crs:OGC:1.3:CRS84`,
/// `http://www.opengis.net/gml/srs/epsg.xml#28992`, or the https spelling of the
/// canonical IRI. Anything else is returned as it is.
///
/// Every EPSG spelling names the EPSG CRS, axis order included: `EPSG:4326` is
/// latitude first, like the canonical IRI.
pub fn normalise_crs_uri(name: &str) -> Cow<'_, str> {
    const BASE: &str = "http://www.opengis.net/def/crs/";
    let name = name.trim();
    let canonical = |authority: &str, version: &str, code: &str| {
        let version = if version.is_empty() { "0" } else { version };
        Cow::Owned(format!("{BASE}{authority}/{version}/{code}"))
    };
    let urn = name
        .strip_prefix("urn:ogc:def:crs:")
        .or_else(|| name.strip_prefix("urn:x-ogc:def:crs:"));
    if let Some(rest) = urn {
        // authority:version:code, the version often empty.
        let parts: Vec<&str> = rest.split(':').collect();
        return match parts.as_slice() {
            [authority, version, code] if !code.is_empty() => canonical(authority, version, code),
            [authority, code] if !code.is_empty() => canonical(authority, "", code),
            _ => Cow::Borrowed(name),
        };
    }
    if let Some(code) = name
        .strip_prefix("EPSG:")
        .or_else(|| name.strip_prefix("epsg:"))
    {
        if !code.is_empty() && code.bytes().all(|b| b.is_ascii_digit()) {
            return canonical("EPSG", "0", code);
        }
    }
    if let Some(code) = name.strip_prefix("http://www.opengis.net/gml/srs/epsg.xml#") {
        return canonical("EPSG", "0", code);
    }
    if let Some(rest) = name.strip_prefix("https://www.opengis.net/def/crs/") {
        return Cow::Owned(format!("{BASE}{rest}"));
    }
    Cow::Borrowed(name)
}

/// RD New's domain: EPSG:28992's area of use (3.2°–7.22° E, 50.75°–53.7° N)
/// with about 50 km to spare, in CRS84 degrees and in RD metres.
const RD_LON: (f64, f64) = (2.5, 8.0);
const RD_LAT: (f64, f64) = (50.3, 54.2);
const RD_X: (f64, f64) = (-50_000.0, 350_000.0);
const RD_Y: (f64, f64) = (250_000.0, 700_000.0);

/// Web Mercator's latitude limit (the square world map ends at ±85.0511°).
const MERCATOR_MAX_LAT: f64 = 85.06;

fn within(v: f64, (lo, hi): (f64, f64)) -> bool {
    (lo..=hi).contains(&v)
}

/// Transform a single coordinate `(x, y)` from `from` to `to`. Coordinates are in
/// each CRS's natural WKT axis order (geographic = lon/lat). `None` when the
/// coordinate is outside either CRS's domain (see the module docs) or a value is
/// not finite — never the coordinate passed through.
pub fn transform_xy(from: Crs, to: Crs, x: f64, y: f64) -> Option<(f64, f64)> {
    if from == to {
        return finite(x, y);
    }
    // Route everything through WGS84 lon/lat as the pivot. EPSG:4326 is the
    // same datum in the opposite axis order, so its "transform" is a swap.
    let (lon, lat) = match from {
        Crs::Wgs84 => (x, y),
        Crs::Epsg4326 => (y, x),
        Crs::RdNew if within(x, RD_X) && within(y, RD_Y) => rd_to_wgs84(x, y),
        Crs::RdNew => return None,
        Crs::WebMercator => webmercator_to_wgs84(x, y),
    };
    if !finite(lon, lat).is_some_and(|_| lat.abs() <= 90.0) {
        return None;
    }
    let (ox, oy) = match to {
        Crs::Wgs84 => (lon, lat),
        Crs::Epsg4326 => (lat, lon),
        Crs::RdNew if within(lon, RD_LON) && within(lat, RD_LAT) => wgs84_to_rd(lon, lat),
        Crs::WebMercator if lat.abs() <= MERCATOR_MAX_LAT && lon.abs() <= 180.0 => {
            wgs84_to_webmercator(lon, lat)
        }
        Crs::RdNew | Crs::WebMercator => return None,
    };
    finite(ox, oy)
}

fn finite(x: f64, y: f64) -> Option<(f64, f64)> {
    (x.is_finite() && y.is_finite()).then_some((x, y))
}

/// Reproject the WKT body of a geometry literal from `source` to `target`,
/// returning the bare transformed WKT (no CRS prefix), or `None` when any
/// coordinate does not transform. `wkt_body` must not carry a `<crs>` prefix —
/// strip it first (see `datatypes::extract_crs`/`extract_wkt`).
pub fn reproject_wkt(wkt_body: &str, source: Crs, target: Crs) -> Option<String> {
    use geo::MapCoords;
    use wkt::{ToWkt, TryFromWkt};
    let geom: geo::Geometry<f64> = geo::Geometry::try_from_wkt_str(wkt_body.trim()).ok()?;
    let out = geom
        .try_map_coords(|c| {
            transform_xy(source, target, c.x, c.y)
                .map(|(x, y)| geo::Coord { x, y })
                .ok_or(())
        })
        .ok()?;
    Some(out.wkt_string())
}

// ─── RD New (EPSG:28992) ↔ WGS84 — Strang van Hees approximation ───

/// RD (easting `x`, northing `y`, metres) → WGS84 `(lon, lat)` degrees.
fn rd_to_wgs84(x: f64, y: f64) -> (f64, f64) {
    let dx = (x - 155_000.0) * 1e-5;
    let dy = (y - 463_000.0) * 1e-5;

    // Latitude (north) series, in arc-seconds.
    let mut north = 0.0;
    for &(p, q, c) in &[
        (0.0, 1.0, 3235.65389),
        (2.0, 0.0, -32.58297),
        (0.0, 2.0, -0.24750),
        (2.0, 1.0, -0.84978),
        (0.0, 3.0, -0.06550),
        (2.0, 2.0, -0.01709),
        (1.0, 0.0, -0.00738),
        (4.0, 0.0, 0.00530),
        (2.0, 3.0, -0.00039),
        (4.0, 1.0, 0.00033),
        (1.0, 1.0, -0.00012),
    ] {
        north += c * dx.powf(p) * dy.powf(q);
    }
    let lat = 52.15517440 + north / 3600.0;

    // Longitude (east) series, in arc-seconds.
    let mut east = 0.0;
    for &(p, q, c) in &[
        (1.0, 0.0, 5260.52916),
        (1.0, 1.0, 105.94684),
        (1.0, 2.0, 2.45656),
        (3.0, 0.0, -0.81885),
        (1.0, 3.0, 0.05594),
        (3.0, 1.0, -0.05607),
        (0.0, 1.0, 0.01199),
        (3.0, 2.0, -0.00256),
        (1.0, 4.0, 0.00128),
        (0.0, 2.0, 0.00022),
        (2.0, 0.0, -0.00022),
        (5.0, 0.0, 0.00026),
    ] {
        east += c * dx.powf(p) * dy.powf(q);
    }
    let lon = 5.38720621 + east / 3600.0;

    (lon, lat)
}

/// WGS84 `(lon, lat)` degrees → RD (easting, northing) metres.
fn wgs84_to_rd(lon: f64, lat: f64) -> (f64, f64) {
    let dlat = 0.36 * (lat - 52.15517440);
    let dlon = 0.36 * (lon - 5.38720621);

    // Coefficients are (p over Δλ/dlon, q over Δφ/dlat, K). Easting is dominated by
    // dlon (1,0), northing by dlat (0,1) — Schreutelkamp & Strang van Hees.
    let mut x = 0.0;
    for &(p, q, c) in &[
        (1.0, 0.0, 190094.945),
        (1.0, 1.0, -11832.228),
        (1.0, 2.0, -114.221),
        (3.0, 0.0, -32.391),
        (0.0, 1.0, -0.705),
        (3.0, 1.0, -2.340),
        (1.0, 3.0, -0.608),
        (0.0, 2.0, -0.008),
        (3.0, 2.0, 0.148),
    ] {
        x += c * dlon.powf(p) * dlat.powf(q);
    }
    let easting = 155_000.0 + x;

    let mut y = 0.0;
    for &(p, q, c) in &[
        (0.0, 1.0, 309056.544),
        (2.0, 0.0, 3638.893),
        (0.0, 2.0, 73.077),
        (2.0, 1.0, -157.984),
        (0.0, 3.0, 59.788),
        (1.0, 0.0, 0.433),
        (2.0, 2.0, -6.439),
        (1.0, 1.0, -0.032),
        (0.0, 4.0, 0.092),
        (1.0, 4.0, -0.054),
    ] {
        y += c * dlon.powf(p) * dlat.powf(q);
    }
    let northing = 463_000.0 + y;

    (easting, northing)
}

// ─── WGS84 ↔ Web Mercator (EPSG:3857) — spherical Mercator ───

const EARTH_R: f64 = 6_378_137.0;

fn wgs84_to_webmercator(lon: f64, lat: f64) -> (f64, f64) {
    let x = EARTH_R * lon.to_radians();
    let y = EARTH_R * ((PI / 4.0) + (lat.to_radians() / 2.0)).tan().ln();
    (x, y)
}

fn webmercator_to_wgs84(x: f64, y: f64) -> (f64, f64) {
    let lon = (x / EARTH_R).to_degrees();
    let lat = (2.0 * (y / EARTH_R).exp().atan() - PI / 2.0).to_degrees();
    (lon, lat)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crs_from_uri() {
        assert_eq!(
            Crs::from_uri("http://www.opengis.net/def/crs/EPSG/0/28992"),
            Some(Crs::RdNew)
        );
        assert_eq!(
            Crs::from_uri("http://www.opengis.net/def/crs/OGC/1.3/CRS84"),
            Some(Crs::Wgs84)
        );
        assert_eq!(
            Crs::from_uri("http://www.opengis.net/def/crs/EPSG/0/3857"),
            Some(Crs::WebMercator)
        );
        assert_eq!(Crs::from_uri("urn:nonsense"), None);
        // EPSG:4326 is the authority's lat/lon order — distinct from CRS84.
        assert_eq!(
            Crs::from_uri("http://www.opengis.net/def/crs/EPSG/0/4326"),
            Some(Crs::Epsg4326)
        );
        assert_eq!(
            Crs::from_uri("urn:ogc:def:crs:EPSG::4326"),
            Some(Crs::Epsg4326)
        );
    }

    /// EPSG:4326 ↔ CRS84 is a pure axis swap on the same datum.
    #[test]
    fn epsg4326_is_crs84_with_axes_swapped() {
        assert_eq!(
            transform_xy(Crs::Epsg4326, Crs::Wgs84, 52.36, 4.885),
            Some((4.885, 52.36))
        );
        assert_eq!(
            transform_xy(Crs::Wgs84, Crs::Epsg4326, 4.885, 52.36),
            Some((52.36, 4.885))
        );
        // Going through a projected CRS lands at the same place either way.
        let (ex, ey) = transform_xy(Crs::Epsg4326, Crs::RdNew, 52.36, 4.885).unwrap();
        let (wx, wy) = transform_xy(Crs::Wgs84, Crs::RdNew, 4.885, 52.36).unwrap();
        assert!((ex - wx).abs() < 1e-6 && (ey - wy).abs() < 1e-6);
    }

    #[test]
    fn rd_to_wgs84_nijmegen() {
        // An RD New point in Nijmegen, POINT(187420 428470) → (~51.85, ~5.86).
        let (lon, lat) = transform_xy(Crs::RdNew, Crs::Wgs84, 187420.0, 428470.0).unwrap();
        assert!((lat - 51.85).abs() < 0.05, "lat {lat}");
        assert!((lon - 5.86).abs() < 0.05, "lon {lon}");
    }

    #[test]
    fn rd_wgs84_roundtrip_within_tolerance() {
        let (x0, y0) = (187420.0, 428470.0);
        let (lon, lat) = transform_xy(Crs::RdNew, Crs::Wgs84, x0, y0).unwrap();
        let (x1, y1) = transform_xy(Crs::Wgs84, Crs::RdNew, lon, lat).unwrap();
        // The forward/inverse approximations agree to well under a metre.
        assert!((x1 - x0).abs() < 1.0, "x {x0} -> {x1}");
        assert!((y1 - y0).abs() < 1.0, "y {y0} -> {y1}");
    }

    #[test]
    fn gml_srs_names_normalise_to_the_canonical_iri() {
        let rd = "http://www.opengis.net/def/crs/EPSG/0/28992";
        for name in [
            "EPSG:28992",
            "urn:ogc:def:crs:EPSG::28992",
            "urn:x-ogc:def:crs:EPSG:28992",
            "http://www.opengis.net/gml/srs/epsg.xml#28992",
            "https://www.opengis.net/def/crs/EPSG/0/28992",
            rd,
        ] {
            assert_eq!(normalise_crs_uri(name), rd, "{name}");
        }
        assert_eq!(
            normalise_crs_uri("urn:ogc:def:crs:OGC:1.3:CRS84"),
            "http://www.opengis.net/def/crs/OGC/1.3/CRS84"
        );
        assert_eq!(
            normalise_crs_uri("urn:example:my-crs"),
            "urn:example:my-crs"
        );
    }

    #[test]
    fn rd_new_outside_the_netherlands_does_not_transform() {
        // Paris and a point 1000 km off the RD origin: no RD New coordinates.
        assert_eq!(transform_xy(Crs::Wgs84, Crs::RdNew, 2.35, 48.85), None);
        assert_eq!(
            transform_xy(Crs::RdNew, Crs::Wgs84, 1_155_000.0, 463_000.0),
            None
        );
        // Just inside the area of use still works.
        assert!(transform_xy(Crs::Wgs84, Crs::RdNew, 3.3, 51.4).is_some());
    }

    #[test]
    fn coordinates_off_the_globe_do_not_transform() {
        assert_eq!(transform_xy(Crs::Wgs84, Crs::WebMercator, 0.0, 90.0), None);
        assert_eq!(transform_xy(Crs::Wgs84, Crs::WebMercator, 0.0, -86.0), None);
        assert_eq!(transform_xy(Crs::Wgs84, Crs::Epsg4326, 0.0, 91.0), None);
        // An EPSG:4326 literal written lon-first by mistake: latitude 120.
        assert_eq!(transform_xy(Crs::Epsg4326, Crs::Wgs84, 120.0, 30.0), None);
        // reproject_wkt fails the whole geometry, it does not copy coordinates.
        assert_eq!(
            reproject_wkt("LINESTRING(0 0, 0 89.9)", Crs::Wgs84, Crs::WebMercator),
            None
        );
    }

    #[test]
    fn wgs84_webmercator_roundtrip() {
        let (lon, lat) = (5.86, 51.85);
        let (x, y) = transform_xy(Crs::Wgs84, Crs::WebMercator, lon, lat).unwrap();
        let (lon2, lat2) = transform_xy(Crs::WebMercator, Crs::Wgs84, x, y).unwrap();
        assert!((lon2 - lon).abs() < 1e-9 && (lat2 - lat).abs() < 1e-9);
    }
}
