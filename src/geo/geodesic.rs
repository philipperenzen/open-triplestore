//! Geodesic measurement on the WGS84 ellipsoid: the GeoSPARQL 1.1 `metric*`
//! functions, and the metre units of `geof:distance` / `geof:buffer` on a
//! geographic CRS.
//!
//! Everything here works in CRS84 (longitude, latitude): an operand is first
//! reprojected through [`super::crs`] (an EPSG:4326 literal is only an axis
//! swap), so the answers are metres and square metres whatever CRS the literal
//! was written in. A CRS this build cannot reproject has no answer — the caller
//! returns unbound rather than a number in unknown units.
//!
//! The primitives are Karney's geodesic algorithms as the `geo` crate exposes
//! them (`GeodesicDistance`, `GeodesicLength`, `GeodesicArea`, and the bearing /
//! destination pair) — accurate to nanometres, and convergent even for nearly
//! antipodal points, where Vincenty's iteration fails. Lengths and areas treat
//! each edge as the geodesic between its vertices.
//!
//! The distance between two non-point geometries, and a buffer, need a plane.
//! They use the *ellipsoidal azimuthal equidistant* projection built from the
//! same primitives: a point at geodesic distance `s` and azimuth `α` from the
//! centre maps to `(s·sin α, s·cos α)`. Distances from the centre are exact, and
//! near it the projection is nearly isometric (the scale error grows as
//! `(r/R)²/6`, 4·10⁻⁵ at 100 km). Edges longer than a kilometre are first split
//! along their geodesic, so their chords in the plane follow the edges. Then:
//!
//! * a buffer is computed planar around the geometry's centroid and mapped
//!   back — around a point, every vertex lies exactly the radius away;
//! * a distance is found planar around the midpoint of the current nearest
//!   pair, twice (the first pair comes from the raw lon/lat plane, which at
//!   high latitudes can pick the wrong pair), and the final number is the exact
//!   geodesic distance between the pair found. Whether two geometries touch is
//!   decided in the lon/lat plane, as the topology functions decide it.

use geo::orient::Direction;
use geo::{
    Centroid, Coord, GeodesicArea, GeodesicBearing, GeodesicDestination, GeodesicDistance,
    GeodesicLength, Geometry, LineString, MapCoords, Orient, Point, Polygon,
};
use geos::{Geom, Geometry as GeosGeometry, GeometryTypes};
use oxrdf::Term;
use wkt::{ToWkt, TryFromWkt};

use super::crs::{transform_xy, Crs};
use super::datatypes::{literal_crs_uri, literal_wkt, parse_wkt_literal};
use super::gml::gml_srs_name;
use super::vocabulary as vocab;

/// GEOS buffer segments per quarter circle — the same as `geof:buffer`.
const QUADRANT_SEGMENTS: i32 = 16;

/// The CRS a geometry literal is written in: a WKT literal's `<crs>` prefix, a
/// GML literal's `srsName`, or CRS84 when there is none (GeoSPARQL's default,
/// and always the case for GeoJSON). `None` for a CRS this build cannot
/// reproject. (The topology functions do not read `srsName` yet; measuring in
/// metres has to, or RD New coordinates would be taken for degrees.)
pub fn literal_crs(term: &Term) -> Option<Crs> {
    let uri = match term {
        Term::Literal(l) if l.datatype().as_str() == vocab::GML_LITERAL => gml_srs_name(l.value()),
        _ => literal_crs_uri(term).map(str::to_string),
    };
    match uri {
        Some(uri) => Crs::from_uri(&uri),
        None => Some(Crs::Wgs84),
    }
}

/// A geometry literal of any serialisation as a `geo` geometry in CRS84, or
/// `None` when it does not parse, its CRS cannot be reprojected, or it does not
/// land on the ellipsoid (a latitude beyond ±90°).
pub fn literal_to_crs84(term: &Term) -> Option<Geometry<f64>> {
    use geo::CoordsIter;
    let crs = literal_crs(term)?;
    let geom: Geometry<f64> = Geometry::try_from_wkt_str(&literal_wkt(term)?).ok()?;
    let geom = reproject(&geom, crs, Crs::Wgs84)?;
    let on_the_ellipsoid = geom.coords_iter().all(|c| c.y.abs() <= 90.0);
    on_the_ellipsoid.then_some(geom)
}

/// A literal that is a single (non-empty) point, as a CRS84 point — through
/// the WKB-cached GEOS parse, so the common point-to-point distance costs one
/// geodesic inverse on top of what the planar distance did.
fn literal_point(term: &Term) -> Option<Point<f64>> {
    let g = parse_wkt_literal(term)?;
    if !matches!(g.geometry_type().ok()?, GeometryTypes::Point) || g.is_empty().ok()? {
        return None;
    }
    let (x, y) = transform_xy(
        literal_crs(term)?,
        Crs::Wgs84,
        g.get_x().ok()?,
        g.get_y().ok()?,
    )?;
    (y.abs() <= 90.0).then_some(Point::new(x, y))
}

/// [`metric_distance`] between two geometry literals, in any serialisation and
/// any CRS this build can reproject.
pub fn literal_distance(a: &Term, b: &Term) -> Option<f64> {
    if let (Some(p), Some(q)) = (literal_point(a), literal_point(b)) {
        return Some(p.geodesic_distance(&q));
    }
    metric_distance(&literal_to_crs84(a)?, &literal_to_crs84(b)?)
}

/// Reproject every coordinate from `from` to `to`, failing the whole geometry
/// if any coordinate does not transform (rather than keeping it unchanged).
pub fn reproject(geom: &Geometry<f64>, from: Crs, to: Crs) -> Option<Geometry<f64>> {
    geom.try_map_coords(|c| {
        transform_xy(from, to, c.x, c.y)
            .map(|(x, y)| Coord { x, y })
            .ok_or(())
    })
    .ok()
}

/// The ellipsoidal azimuthal equidistant projection centred at `centre` (CRS84).
#[derive(Clone, Copy)]
struct Aeqd {
    centre: Point<f64>,
}

impl Aeqd {
    fn forward(&self, lon: f64, lat: f64) -> Option<(f64, f64)> {
        let (azimuth, s) = self.centre.geodesic_bearing_distance(Point::new(lon, lat));
        let a = azimuth.to_radians();
        let (x, y) = (s * a.sin(), s * a.cos());
        (x.is_finite() && y.is_finite()).then_some((x, y))
    }

    fn inverse(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let s = x.hypot(y);
        let azimuth = x.atan2(y).to_degrees();
        let p = self.centre.geodesic_destination(azimuth, s);
        (p.x().is_finite() && p.y().is_finite()).then_some((p.x(), p.y()))
    }

    fn project(&self, geom: &Geometry<f64>) -> Option<Geometry<f64>> {
        geom.try_map_coords(|c| {
            self.forward(c.x, c.y)
                .map(|(x, y)| Coord { x, y })
                .ok_or(())
        })
        .ok()
    }

    fn unproject(&self, geom: &Geometry<f64>) -> Option<Geometry<f64>> {
        geom.try_map_coords(|c| {
            self.inverse(c.x, c.y)
                .map(|(x, y)| Coord { x, y })
                .ok_or(())
        })
        .ok()
    }
}

fn to_geos(geom: &Geometry<f64>) -> Option<GeosGeometry> {
    GeosGeometry::new_from_wkt(&geom.wkt_string()).ok()
}

fn from_geos(geom: &GeosGeometry) -> Option<Geometry<f64>> {
    Geometry::try_from_wkt_str(&geom.to_wkt().ok()?).ok()
}

/// The pair of nearest points of two GEOS geometries, `None` if either is empty.
fn nearest(a: &GeosGeometry, b: &GeosGeometry) -> Option<(Point<f64>, Point<f64>)> {
    if a.is_empty().ok()? || b.is_empty().ok()? {
        return None;
    }
    let cs = a.nearest_points(b).ok()?;
    Some((
        Point::new(cs.get_x(0).ok()?, cs.get_y(0).ok()?),
        Point::new(cs.get_x(1).ok()?, cs.get_y(1).ok()?),
    ))
}

/// The point halfway along the geodesic from `p` to `q`.
fn midpoint(p: Point<f64>, q: Point<f64>) -> Point<f64> {
    let (azimuth, s) = p.geodesic_bearing_distance(q);
    p.geodesic_destination(azimuth, s / 2.0)
}

/// Longest edge kept as a single chord in the azimuthal equidistant plane. A
/// geodesic edge maps to a curve there (unless it runs through the centre);
/// split into pieces this short, the chords stay within a millimetre or so of
/// it for geometries up to ~1000 km from the centre.
const MAX_CHORD_M: f64 = 1_000.0;

/// Split every edge longer than [`MAX_CHORD_M`] along its geodesic, so the
/// geometry keeps its shape through the azimuthal equidistant projection.
/// Edges already that short — the usual case for buildings and parcels — are
/// left as they are.
fn densify(geom: &Geometry<f64>) -> Geometry<f64> {
    use geo::{GeodesicIntermediate, GeometryCollection, MultiLineString, MultiPolygon};
    fn line(ls: &LineString<f64>) -> LineString<f64> {
        let mut out: Vec<Coord<f64>> = Vec::with_capacity(ls.0.len());
        for w in ls.0.windows(2) {
            out.push(w[0]);
            let (p, q) = (Point::from(w[0]), Point::from(w[1]));
            out.extend(
                p.geodesic_intermediate_fill(&q, MAX_CHORD_M, false)
                    .into_iter()
                    .map(|pt| pt.0),
            );
        }
        out.extend(ls.0.last().copied());
        LineString(out)
    }
    fn polygon(p: &Polygon<f64>) -> Polygon<f64> {
        Polygon::new(line(p.exterior()), p.interiors().iter().map(line).collect())
    }
    match geom {
        Geometry::Line(l) => Geometry::LineString(line(&LineString(vec![l.start, l.end]))),
        Geometry::LineString(ls) => Geometry::LineString(line(ls)),
        Geometry::MultiLineString(m) => {
            Geometry::MultiLineString(MultiLineString(m.iter().map(line).collect()))
        }
        Geometry::Polygon(p) => Geometry::Polygon(polygon(p)),
        Geometry::MultiPolygon(m) => {
            Geometry::MultiPolygon(MultiPolygon(m.iter().map(polygon).collect()))
        }
        Geometry::Rect(r) => Geometry::Polygon(polygon(&r.to_polygon())),
        Geometry::Triangle(t) => Geometry::Polygon(polygon(&t.to_polygon())),
        Geometry::GeometryCollection(gc) => {
            Geometry::GeometryCollection(GeometryCollection(gc.iter().map(densify).collect()))
        }
        points => points.clone(),
    }
}

/// The shortest geodesic distance in metres between any two points of `a` and
/// `b` (both CRS84), their edges taken as geodesics like the other metric
/// functions take them; 0 when they intersect, `None` when either is empty.
pub fn metric_distance(a: &Geometry<f64>, b: &Geometry<f64>) -> Option<f64> {
    if let (Geometry::Point(p), Geometry::Point(q)) = (a, b) {
        return Some(p.geodesic_distance(q));
    }
    // First estimate in the raw lon/lat plane: exact when they intersect (the
    // pair is then one shared point), otherwise only a starting point.
    let (mut p, mut q) = nearest(&to_geos(a)?, &to_geos(b)?)?;
    if p == q {
        return Some(0.0);
    }
    let (a, b) = (densify(a), densify(b));
    // A point operand is the best possible centre: distances from it are exact.
    let point_operand = match (&a, &b) {
        (Geometry::Point(c), _) | (_, Geometry::Point(c)) => Some(*c),
        _ => None,
    };
    let passes = if point_operand.is_some() { 1 } else { 2 };
    for _ in 0..passes {
        let aeqd = Aeqd {
            centre: point_operand.unwrap_or_else(|| midpoint(p, q)),
        };
        let (pa, pb) = nearest(&to_geos(&aeqd.project(&a)?)?, &to_geos(&aeqd.project(&b)?)?)?;
        let (px, py) = aeqd.inverse(pa.x(), pa.y())?;
        let (qx, qy) = aeqd.inverse(pb.x(), pb.y())?;
        p = Point::new(px, py);
        q = Point::new(qx, qy);
    }
    Some(p.geodesic_distance(&q))
}

/// Visit every polygon of a geometry (a `Rect`/`Triangle` as its polygon).
fn polygons(geom: &Geometry<f64>, f: &mut dyn FnMut(&Polygon<f64>)) {
    match geom {
        Geometry::Polygon(p) => f(p),
        Geometry::MultiPolygon(mp) => {
            for p in mp {
                f(p);
            }
        }
        Geometry::Rect(r) => f(&r.to_polygon()),
        Geometry::Triangle(t) => f(&t.to_polygon()),
        Geometry::GeometryCollection(gc) => {
            for g in gc {
                polygons(g, f);
            }
        }
        _ => {}
    }
}

/// The geodesic area in m² of the polygons of a CRS84 geometry — zero for
/// points and lines, as GeoSPARQL requires. Rings are oriented first (exterior
/// counter-clockwise): WKT does not fix a winding, and the area of a ring wound
/// the other way would be the rest of the ellipsoid.
pub fn metric_area(geom: &Geometry<f64>) -> f64 {
    let mut total = 0.0;
    polygons(geom, &mut |p| {
        total += p.orient(Direction::Default).geodesic_area_unsigned();
    });
    total
}

/// The geodesic length in metres of the rings of the polygons of a CRS84
/// geometry, holes included (like `ST_Perimeter`); zero for anything else.
pub fn metric_perimeter(geom: &Geometry<f64>) -> f64 {
    let mut total = 0.0;
    polygons(geom, &mut |p| {
        total += ring_length(p.exterior()) + p.interiors().iter().map(ring_length).sum::<f64>();
    });
    total
}

fn ring_length(ring: &LineString<f64>) -> f64 {
    ring.geodesic_length()
}

/// The geodesic length in metres of a CRS84 geometry's one-dimensional
/// content: its lines, and the boundary of its polygons (GEOS's `length`, and
/// JTS's, count an areal geometry's rings) — zero for points.
pub fn metric_length(geom: &Geometry<f64>) -> f64 {
    match geom {
        Geometry::Line(l) => l.geodesic_length(),
        Geometry::LineString(ls) => ls.geodesic_length(),
        Geometry::MultiLineString(mls) => mls.geodesic_length(),
        Geometry::GeometryCollection(gc) => gc.iter().map(metric_length).sum(),
        Geometry::Point(_) | Geometry::MultiPoint(_) => 0.0,
        polygonal => metric_perimeter(polygonal),
    }
}

/// A geodesic buffer of `radius_m` metres around a CRS84 geometry, as a CRS84
/// geometry: buffered in the azimuthal equidistant plane centred on its
/// centroid, then mapped back. `None` when the result cannot be expressed in
/// lon/lat — it crosses the antimeridian or encircles a pole — or does not map.
pub fn metric_buffer(geom: &Geometry<f64>, radius_m: f64) -> Option<Geometry<f64>> {
    if !radius_m.is_finite() {
        return None;
    }
    let Some(centre) = geom.centroid() else {
        // Empty: the buffer of nothing is the empty polygon.
        return from_geos(&to_geos(geom)?.buffer(radius_m, QUADRANT_SEGMENTS).ok()?);
    };
    let aeqd = Aeqd { centre };
    let buffered = to_geos(&aeqd.project(&densify(geom))?)?
        .buffer(radius_m, QUADRANT_SEGMENTS)
        .ok()?;
    let out = aeqd.unproject(&from_geos(&buffered)?)?;
    (!wraps_around(&out)).then_some(out)
}

/// Whether two consecutive vertices of a (buffer's) ring are more than 180° of
/// longitude apart — the tell of a ring that crossed the antimeridian or went
/// round a pole, which a lon/lat polygon cannot represent.
fn wraps_around(geom: &Geometry<f64>) -> bool {
    let jumps = |ls: &LineString<f64>| ls.0.windows(2).any(|w| (w[1].x - w[0].x).abs() > 180.0);
    let mut wrapped = false;
    polygons(geom, &mut |p| {
        wrapped |= jumps(p.exterior()) || p.interiors().iter().any(jumps);
    });
    wrapped
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{line_string, point, polygon};

    #[test]
    fn point_distances_match_published_values() {
        // GeographicLib's JFK–LHR example, and the geo crate's NYC–London.
        let jfk = point!(x: -73.8, y: 40.6);
        let lhr = point!(x: -0.5, y: 51.6);
        let d = metric_distance(&Geometry::Point(jfk), &Geometry::Point(lhr)).unwrap();
        assert!((d - 5_551_759.400).abs() < 0.001, "{d}");
    }

    #[test]
    fn aeqd_round_trips_and_is_exact_from_the_centre() {
        let aeqd = Aeqd {
            centre: point!(x: 5.0, y: 52.0),
        };
        let (x, y) = aeqd.forward(5.1, 52.05).unwrap();
        let (lon, lat) = aeqd.inverse(x, y).unwrap();
        assert!((lon - 5.1).abs() < 1e-12 && (lat - 52.05).abs() < 1e-12);
        let s = aeqd.centre.geodesic_distance(&point!(x: 5.1, y: 52.05));
        assert!((x.hypot(y) - s).abs() < 1e-6, "radial distance is exact");
        assert_eq!(aeqd.forward(5.0, 52.0), Some((0.0, 0.0)));
    }

    #[test]
    fn distance_between_a_point_and_a_line() {
        let line = Geometry::LineString(line_string![(x: -1.0, y: 0.0), (x: 1.0, y: 0.0)]);
        let d = metric_distance(&Geometry::Point(point!(x: 0.0, y: 0.5)), &line).unwrap();
        assert!((d - 55_287.152).abs() < 0.01, "{d}");
        // Symmetric.
        let e = metric_distance(&line, &Geometry::Point(point!(x: 0.0, y: 0.5))).unwrap();
        assert!((d - e).abs() < 1e-6);
    }

    #[test]
    fn a_long_edge_is_followed_not_cut_short() {
        // 20° of equator as one edge. Undensified, its chord in the projection
        // runs metres inside the equator's (curved) image.
        let line = Geometry::LineString(line_string![(x: -10.0, y: 0.0), (x: 10.0, y: 0.0)]);
        let d = metric_distance(&Geometry::Point(point!(x: 0.0, y: 0.5)), &line).unwrap();
        assert!((d - 55_287.152).abs() < 0.01, "{d}");
        // Two long edges, neither of them a point: the two-pass search.
        let other = Geometry::LineString(line_string![(x: -10.0, y: 1.0), (x: 10.0, y: 1.0)]);
        let d = metric_distance(&line, &other).unwrap();
        // The geodesic from (-10, 1) to (10, 1) bulges north of the parallel,
        // so the nearest pair is at the ends — at most one meridian degree.
        assert!(d > 110_000.0 && d <= 110_574.389 + 0.01, "{d}");
    }

    #[test]
    fn intersecting_geometries_are_zero_apart() {
        let a = Geometry::Polygon(
            polygon![(x: 0.0, y: 0.0), (x: 1.0, y: 0.0), (x: 1.0, y: 1.0), (x: 0.0, y: 1.0)],
        );
        let b = Geometry::LineString(line_string![(x: 0.5, y: 0.5), (x: 2.0, y: 2.0)]);
        assert_eq!(metric_distance(&a, &b), Some(0.0));
    }

    #[test]
    fn area_ignores_winding_and_is_zero_for_lines() {
        let ccw = Geometry::Polygon(
            polygon![(x: 0.0, y: 0.0), (x: 1.0, y: 0.0), (x: 1.0, y: 1.0), (x: 0.0, y: 1.0)],
        );
        let cw = Geometry::Polygon(
            polygon![(x: 0.0, y: 0.0), (x: 0.0, y: 1.0), (x: 1.0, y: 1.0), (x: 1.0, y: 0.0)],
        );
        let (a, b) = (metric_area(&ccw), metric_area(&cw));
        assert!((a - b).abs() < 1e-3, "{a} vs {b}");
        assert!(((a - 12_308_463_894.0) / a).abs() < 1e-4, "{a}");
        let line = Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 1.0)]);
        assert_eq!(metric_area(&line), 0.0);
        assert_eq!(metric_perimeter(&line), 0.0);
        assert!(metric_length(&line) > 156_899.0);
    }

    #[test]
    fn a_buffer_around_a_point_is_a_geodesic_circle() {
        let centre = point!(x: 5.0, y: 52.0);
        let buf = metric_buffer(&Geometry::Point(centre), 1000.0).unwrap();
        let Geometry::Polygon(p) = &buf else {
            panic!("a polygon, got {buf:?}")
        };
        for c in p.exterior().coords() {
            let d = centre.geodesic_distance(&Point::from(*c));
            assert!((d - 1000.0).abs() < 1e-6, "vertex at {d} m");
        }
    }

    #[test]
    fn a_buffer_round_a_pole_or_across_the_antimeridian_is_refused() {
        let near_pole = Geometry::Point(point!(x: 0.0, y: 89.9));
        assert!(metric_buffer(&near_pole, 50_000.0).is_none());
        let dateline = Geometry::Point(point!(x: 179.99, y: 0.0));
        assert!(metric_buffer(&dateline, 5_000.0).is_none());
        // Well clear of both, it is fine.
        assert!(metric_buffer(&Geometry::Point(point!(x: 170.0, y: 0.0)), 5_000.0).is_some());
    }

    fn literal(value: &str, datatype: &str) -> Term {
        Term::Literal(oxrdf::Literal::new_typed_literal(
            value,
            oxrdf::NamedNode::new_unchecked(datatype),
        ))
    }

    #[test]
    fn a_gml_literal_is_measured_in_its_srs_name() {
        let rd = literal(
            "<gml:Point srsName='urn:ogc:def:crs:EPSG::28992'><gml:pos>155000 463000</gml:pos></gml:Point>",
            vocab::GML_LITERAL,
        );
        assert_eq!(literal_crs(&rd), Some(Crs::RdNew));
        let p = literal_to_crs84(&rd).expect("reprojected from RD New");
        let Geometry::Point(p) = p else {
            panic!("a point")
        };
        // The Amersfoort origin of RD New: 5.387°E 52.155°N.
        assert!((p.x() - 5.387).abs() < 0.01 && (p.y() - 52.155).abs() < 0.01);
        let plain = literal(
            "<gml:Point><gml:pos>5 52</gml:pos></gml:Point>",
            vocab::GML_LITERAL,
        );
        assert_eq!(literal_crs(&plain), Some(Crs::Wgs84));
    }

    #[test]
    fn coordinates_off_the_ellipsoid_are_refused() {
        // RD New numbers read as CRS84: latitude 463000° is no place at all.
        let wrong = literal("POINT(155000 463000)", vocab::WKT_LITERAL);
        assert!(literal_to_crs84(&wrong).is_none());
        assert!(literal_point(&wrong).is_none());
        assert!(literal_distance(&wrong, &literal("POINT(5 52)", vocab::WKT_LITERAL)).is_none());
    }

    #[test]
    fn a_non_finite_radius_is_refused() {
        let p = Geometry::Point(point!(x: 5.0, y: 52.0));
        assert!(metric_buffer(&p, f64::NAN).is_none());
        assert!(metric_buffer(&p, f64::INFINITY).is_none());
    }
}
