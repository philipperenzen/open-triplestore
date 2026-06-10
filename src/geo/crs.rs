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
//! **Axis order.** Geographic coordinates are handled in WKT/GeoJSON order
//! `(x = longitude, y = latitude)` throughout, so a transformed geometry can feed
//! a `[lng, lat]` map layer directly. Projected CRS use `(x = easting,
//! y = northing)`.

use std::f64::consts::PI;

/// A supported coordinate reference system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Crs {
    /// Amersfoort / RD New (EPSG:28992) — easting/northing in metres.
    RdNew,
    /// WGS84 geographic (EPSG:4326 / OGC:CRS84) — lon/lat in degrees.
    Wgs84,
    /// Web Mercator (EPSG:3857) — easting/northing in metres.
    WebMercator,
}

impl Crs {
    /// Resolve a CRS from a CRS URI as used in WKT/GeoSPARQL literals. Recognises the
    /// common EPSG and OGC forms; returns `None` for an unsupported CRS.
    pub fn from_uri(uri: &str) -> Option<Crs> {
        let u = uri.trim_end_matches('>').trim_start_matches('<');
        if u.ends_with("CRS84") || u.ends_with("/4326") || u.ends_with(":4326") {
            Some(Crs::Wgs84)
        } else if u.ends_with("/28992") || u.ends_with(":28992") {
            Some(Crs::RdNew)
        } else if u.ends_with("/3857") || u.ends_with(":3857") || u.ends_with("/900913") {
            Some(Crs::WebMercator)
        } else {
            None
        }
    }

    /// Canonical EPSG CRS URI for this CRS (the form used in GeoSPARQL WKT prefixes).
    pub fn to_uri(self) -> &'static str {
        match self {
            Crs::RdNew => "http://www.opengis.net/def/crs/EPSG/0/28992",
            Crs::Wgs84 => "http://www.opengis.net/def/crs/EPSG/0/4326",
            Crs::WebMercator => "http://www.opengis.net/def/crs/EPSG/0/3857",
        }
    }
}

/// Transform a single coordinate `(x, y)` from `from` to `to`. Coordinates are in
/// each CRS's natural WKT axis order (geographic = lon/lat). Returns `None` only if
/// an intermediate value is non-finite.
pub fn transform_xy(from: Crs, to: Crs, x: f64, y: f64) -> Option<(f64, f64)> {
    if from == to {
        return finite(x, y);
    }
    // Route everything through WGS84 lon/lat as the pivot.
    let (lon, lat) = match from {
        Crs::Wgs84 => (x, y),
        Crs::RdNew => rd_to_wgs84(x, y),
        Crs::WebMercator => webmercator_to_wgs84(x, y),
    };
    let (ox, oy) = match to {
        Crs::Wgs84 => (lon, lat),
        Crs::RdNew => wgs84_to_rd(lon, lat),
        Crs::WebMercator => wgs84_to_webmercator(lon, lat),
    };
    finite(ox, oy)
}

fn finite(x: f64, y: f64) -> Option<(f64, f64)> {
    (x.is_finite() && y.is_finite()).then_some((x, y))
}

/// Reproject the WKT body of a geometry literal from `source` to `target`,
/// returning the bare transformed WKT (no CRS prefix). `wkt_body` must not carry
/// a `<crs>` prefix — strip it first (see `datatypes::extract_crs`/`extract_wkt`).
pub fn reproject_wkt(wkt_body: &str, source: Crs, target: Crs) -> Option<String> {
    use geo::MapCoords;
    use wkt::{ToWkt, TryFromWkt};
    let geom: geo::Geometry<f64> = geo::Geometry::try_from_wkt_str(wkt_body.trim()).ok()?;
    let out = geom.map_coords(|c| {
        let (x, y) = transform_xy(source, target, c.x, c.y).unwrap_or((c.x, c.y));
        geo::Coord { x, y }
    });
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
    }

    #[test]
    fn rd_to_wgs84_nijmegen() {
        // Waalbrug Boog-Noord, RD POINT(187420 428470) → near Nijmegen (~51.85, ~5.86).
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
    fn wgs84_webmercator_roundtrip() {
        let (lon, lat) = (5.86, 51.85);
        let (x, y) = transform_xy(Crs::Wgs84, Crs::WebMercator, lon, lat).unwrap();
        let (lon2, lat2) = transform_xy(Crs::WebMercator, Crs::Wgs84, x, y).unwrap();
        assert!((lon2 - lon).abs() < 1e-9 && (lat2 - lat).abs() < 1e-9);
    }
}
