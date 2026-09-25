//! `geo:geoJSONLiteral` — RFC 7946 GeoJSON geometry objects, read and written.
//!
//! GeoSPARQL 1.1 defines a `geo:geoJSONLiteral` as the GeoJSON serialisation of
//! a *geometry object*: `Point`, `MultiPoint`, `LineString`, `MultiLineString`,
//! `Polygon`, `MultiPolygon` or `GeometryCollection` (a `Feature` is not a
//! geometry and is rejected). RFC 7946 removed the `crs` member, so the
//! coordinates are always WGS84 longitude/latitude — CRS84, GeoSPARQL's
//! default — and a legacy `crs` member is ignored like any other foreign one.
//!
//! Reading goes GeoJSON → WKT, the bridge [`super::gml`] already uses for GML,
//! so every consumer of a geometry literal — the `geof:` functions, the spatial
//! index, the viewer feed — reaches GEOS by the one WKT path. Positions keep
//! their first two ordinates: an altitude is dropped, as the GML reader drops
//! Z, because the `geof:` surface is two-dimensional. An empty `coordinates`
//! (or `geometries`) array reads as the empty geometry of that type.
//!
//! Writing (`geof:asGeoJSON`) walks a GEOS geometry and emits the object,
//! mapping every coordinate through a caller-supplied transform so the output
//! is CRS84 whatever the source CRS was.
//!
//! Malformed input yields `None`, never a panic: invalid JSON, an unknown or
//! missing `type`, a position with fewer than two numbers, a line with fewer
//! than two positions, or a polygon ring that is not closed or has fewer than
//! four positions (RFC 7946 §3.1).

use geos::{Geom, GeometryTypes};
use serde_json::{json, Map, Value};

/// Convert a GeoJSON geometry object to WKT, or `None` if it is not one.
pub fn geojson_to_wkt(json: &str) -> Option<String> {
    let value: Value = serde_json::from_str(json.trim()).ok()?;
    let mut out = String::new();
    write_geometry(&value, &mut out)?;
    Some(out)
}

/// Write one geometry object as WKT. Nesting (a `GeometryCollection` inside
/// another) is bounded by serde_json's own recursion limit on the input.
fn write_geometry(value: &Value, out: &mut String) -> Option<()> {
    let obj = value.as_object()?;
    let kind = obj.get("type")?.as_str()?;
    if kind == "GeometryCollection" {
        let members = obj.get("geometries")?.as_array()?;
        if members.is_empty() {
            out.push_str("GEOMETRYCOLLECTION EMPTY");
            return Some(());
        }
        out.push_str("GEOMETRYCOLLECTION(");
        for (i, member) in members.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            write_geometry(member, out)?;
        }
        out.push(')');
        return Some(());
    }
    let coords = obj.get("coordinates")?.as_array()?;
    let (tag, body) = match kind {
        "Point" => ("POINT", point(coords)?),
        "MultiPoint" => ("MULTIPOINT", list(coords, |p| point(p.as_array()?))?),
        "LineString" => ("LINESTRING", line(coords)?),
        "MultiLineString" => ("MULTILINESTRING", list(coords, |l| line(l.as_array()?))?),
        "Polygon" => ("POLYGON", polygon(coords)?),
        "MultiPolygon" => ("MULTIPOLYGON", list(coords, |p| polygon(p.as_array()?))?),
        _ => return None,
    };
    out.push_str(tag);
    match body {
        Some(body) => {
            out.push_str(&body);
        }
        None => out.push_str(" EMPTY"),
    }
    Some(())
}

/// `(x y)` for a position, or `None` (outer) when it is not one; the inner
/// `None` is the empty point (`"coordinates": []`).
fn point(coords: &[Value]) -> Option<Option<String>> {
    if coords.is_empty() {
        return Some(None);
    }
    Some(Some(format!("({})", position(coords)?)))
}

/// `x y` from a position: two or more numbers, the first two finite.
fn position(p: &[Value]) -> Option<String> {
    if p.len() < 2 || !p.iter().all(Value::is_number) {
        return None;
    }
    let (x, y) = (p[0].as_f64()?, p[1].as_f64()?);
    (x.is_finite() && y.is_finite()).then(|| format!("{x} {y}"))
}

/// `(x y, x y, …)` from an array of positions.
fn positions(coords: &[Value]) -> Option<String> {
    let parts = coords
        .iter()
        .map(|p| position(p.as_array()?))
        .collect::<Option<Vec<_>>>()?;
    Some(format!("({})", parts.join(", ")))
}

/// A LineString's coordinates: two or more positions (or none — empty).
fn line(coords: &[Value]) -> Option<Option<String>> {
    match coords.len() {
        0 => Some(None),
        1 => None,
        _ => Some(Some(positions(coords)?)),
    }
}

/// A Polygon's coordinates: linear rings, each closed with four or more
/// positions (or no rings — empty).
fn polygon(rings: &[Value]) -> Option<Option<String>> {
    if rings.is_empty() {
        return Some(None);
    }
    let parts = rings
        .iter()
        .map(|r| {
            let r = r.as_array()?;
            let closed = r.len() >= 4 && same_position(r.first()?, r.last()?);
            closed.then(|| positions(r)).flatten()
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Some(format!("({})", parts.join(", "))))
}

/// Two positions with the same numeric values — `[0,0]` and `[0.0,0.0]`
/// close a ring alike, which a JSON-value comparison would not see.
fn same_position(a: &Value, b: &Value) -> bool {
    match (a.as_array(), b.as_array()) {
        (Some(a), Some(b)) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b)
                    .all(|(x, y)| x.as_f64().is_some() && x.as_f64() == y.as_f64())
        }
        _ => false,
    }
}

/// A Multi* body: each member through `member`, which yields `None` for a
/// malformed member and `Some(None)` for an empty one. RFC 7946 has no empty
/// member of a multi-geometry, so an empty member is malformed here.
fn list(
    members: &[Value],
    member: impl Fn(&Value) -> Option<Option<String>>,
) -> Option<Option<String>> {
    if members.is_empty() {
        return Some(None);
    }
    let parts = members
        .iter()
        .map(|m| member(m).flatten())
        .collect::<Option<Vec<_>>>()?;
    Some(Some(format!("({})", parts.join(", "))))
}

/// Serialise a GEOS geometry as a GeoJSON geometry object, mapping every
/// coordinate through `xy` (the reprojection into CRS84). `None` when a
/// coordinate does not transform or the geometry has a type GeoJSON cannot
/// express (curves).
pub fn geometry_to_geojson(
    geom: &impl Geom,
    xy: &dyn Fn(f64, f64) -> Option<(f64, f64)>,
) -> Option<Value> {
    let kind = geom.geometry_type().ok()?;
    let object = |kind: &str, coordinates: Value| -> Value {
        let mut m = Map::new();
        m.insert("type".into(), Value::from(kind));
        m.insert("coordinates".into(), coordinates);
        Value::Object(m)
    };
    Some(match kind {
        GeometryTypes::Point => object("Point", point_coords(geom, xy)?),
        GeometryTypes::LineString | GeometryTypes::LinearRing => {
            object("LineString", sequence(geom, xy)?)
        }
        GeometryTypes::Polygon => object("Polygon", rings(geom, xy)?),
        GeometryTypes::MultiPoint => object(
            "MultiPoint",
            members(geom, |g| {
                let c = point_coords(g, xy)?;
                // An empty member point has no GeoJSON position: skip it.
                Some(c.as_array().is_some_and(|a| !a.is_empty()).then_some(c))
            })?,
        ),
        GeometryTypes::MultiLineString => object(
            "MultiLineString",
            members(geom, |g| sequence(g, xy).map(Some))?,
        ),
        GeometryTypes::MultiPolygon => {
            object("MultiPolygon", members(geom, |g| rings(g, xy).map(Some))?)
        }
        // A collection, or (with a GEOS build that has them) a curve type
        // GeoJSON cannot express.
        _ => {
            if !matches!(kind, GeometryTypes::GeometryCollection) {
                return None;
            }
            let n = geom.get_num_geometries().ok()?;
            let geometries = (0..n)
                .map(|i| geometry_to_geojson(&geom.get_geometry_n(i).ok()?, xy))
                .collect::<Option<Vec<_>>>()?;
            json!({ "type": "GeometryCollection", "geometries": geometries })
        }
    })
}

/// A point's position, or `[]` for the empty point.
fn point_coords(geom: &impl Geom, xy: &dyn Fn(f64, f64) -> Option<(f64, f64)>) -> Option<Value> {
    if geom.is_empty().ok()? {
        return Some(json!([]));
    }
    let (x, y) = xy(geom.get_x().ok()?, geom.get_y().ok()?)?;
    Some(json!([x, y]))
}

/// The positions of a line or ring.
fn sequence(geom: &impl Geom, xy: &dyn Fn(f64, f64) -> Option<(f64, f64)>) -> Option<Value> {
    if geom.is_empty().ok()? {
        return Some(json!([]));
    }
    let cs = geom.get_coord_seq().ok()?;
    let n = cs.size().ok()?;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let (x, y) = xy(cs.get_x(i).ok()?, cs.get_y(i).ok()?)?;
        out.push(json!([x, y]));
    }
    Some(Value::Array(out))
}

/// A polygon's rings, exterior first.
fn rings(geom: &impl Geom, xy: &dyn Fn(f64, f64) -> Option<(f64, f64)>) -> Option<Value> {
    if geom.is_empty().ok()? {
        return Some(json!([]));
    }
    let mut out = vec![sequence(&geom.get_exterior_ring().ok()?, xy)?];
    for i in 0..geom.get_num_interior_rings().ok()? {
        out.push(sequence(&geom.get_interior_ring_n(i).ok()?, xy)?);
    }
    Some(Value::Array(out))
}

/// The coordinates of each member of a multi-geometry; `member` returns
/// `Some(None)` to skip one (an empty point).
fn members<G: Geom>(
    geom: &G,
    member: impl Fn(&geos::ConstGeometry<'_>) -> Option<Option<Value>>,
) -> Option<Value> {
    let n = geom.get_num_geometries().ok()?;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        if let Some(c) = member(&geom.get_geometry_n(i).ok()?)? {
            out.push(c);
        }
    }
    Some(Value::Array(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use geos::Geometry;

    fn wkt(json: &str) -> Option<String> {
        geojson_to_wkt(json)
    }

    #[test]
    fn every_geometry_type_reads_as_wkt() {
        assert_eq!(
            wkt(r#"{"type":"Point","coordinates":[-0.1278,51.5074]}"#).as_deref(),
            Some("POINT(-0.1278 51.5074)")
        );
        assert_eq!(
            wkt(r#"{"type":"MultiPoint","coordinates":[[1,2],[3,4]]}"#).as_deref(),
            Some("MULTIPOINT((1 2), (3 4))")
        );
        assert_eq!(
            wkt(r#"{"type":"LineString","coordinates":[[0,0],[1,1],[2,0]]}"#).as_deref(),
            Some("LINESTRING(0 0, 1 1, 2 0)")
        );
        assert_eq!(
            wkt(r#"{"type":"MultiLineString","coordinates":[[[0,0],[1,1]],[[2,2],[3,3]]]}"#)
                .as_deref(),
            Some("MULTILINESTRING((0 0, 1 1), (2 2, 3 3))")
        );
        assert_eq!(
            wkt(r#"{"type":"Polygon","coordinates":[[[0,0],[4,0],[4,4],[0,4],[0,0]],[[1,1],[2,1],[2,2],[1,1]]]}"#)
                .as_deref(),
            Some("POLYGON((0 0, 4 0, 4 4, 0 4, 0 0), (1 1, 2 1, 2 2, 1 1))")
        );
        assert_eq!(
            wkt(r#"{"type":"MultiPolygon","coordinates":[[[[0,0],[1,0],[1,1],[0,0]]],[[[2,2],[3,2],[3,3],[2,2]]]]}"#)
                .as_deref(),
            Some("MULTIPOLYGON(((0 0, 1 0, 1 1, 0 0)), ((2 2, 3 2, 3 3, 2 2)))")
        );
        assert_eq!(
            wkt(r#"{"type":"GeometryCollection","geometries":[{"type":"Point","coordinates":[0,0]},{"type":"LineString","coordinates":[[1,1],[2,2]]}]}"#)
                .as_deref(),
            Some("GEOMETRYCOLLECTION(POINT(0 0), LINESTRING(1 1, 2 2))")
        );
    }

    #[test]
    fn a_ring_closed_with_a_differently_written_number_is_closed() {
        assert_eq!(
            wkt(r#"{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0.0,0.0]]]}"#).as_deref(),
            Some("POLYGON((0 0, 1 0, 1 1, 0 0))")
        );
    }

    #[test]
    fn altitude_and_foreign_members_are_dropped() {
        // A third ordinate (altitude) is dropped; bbox, crs and any other
        // foreign member do not change the geometry.
        assert_eq!(
            wkt(r#"{"type":"Point","coordinates":[5.86,51.85,12.5],"bbox":[0,0,1,1],"crs":{"type":"name"}}"#)
                .as_deref(),
            Some("POINT(5.86 51.85)")
        );
    }

    #[test]
    fn empty_coordinates_read_as_empty_geometries() {
        assert_eq!(
            wkt(r#"{"type":"Point","coordinates":[]}"#).as_deref(),
            Some("POINT EMPTY")
        );
        assert_eq!(
            wkt(r#"{"type":"MultiPolygon","coordinates":[]}"#).as_deref(),
            Some("MULTIPOLYGON EMPTY")
        );
        assert_eq!(
            wkt(r#"{"type":"GeometryCollection","geometries":[]}"#).as_deref(),
            Some("GEOMETRYCOLLECTION EMPTY")
        );
        // Every WKT this module writes parses in GEOS.
        for w in [
            "POINT EMPTY",
            "MULTIPOLYGON EMPTY",
            "GEOMETRYCOLLECTION EMPTY",
        ] {
            assert!(Geometry::new_from_wkt(w).is_ok(), "{w}");
        }
    }

    #[test]
    fn malformed_input_is_none_not_a_panic() {
        for bad in [
            "",
            "not json",
            "{",
            "[]",
            "42",
            r#"{"coordinates":[0,0]}"#,
            r#"{"type":"Feature","geometry":{"type":"Point","coordinates":[0,0]}}"#,
            r#"{"type":"FeatureCollection","features":[]}"#,
            r#"{"type":"point","coordinates":[0,0]}"#,
            r#"{"type":"Point"}"#,
            r#"{"type":"Point","coordinates":[0]}"#,
            r#"{"type":"Point","coordinates":["0","1"]}"#,
            r#"{"type":"Point","coordinates":[0,null]}"#,
            r#"{"type":"Point","coordinates":[1e999,0]}"#,
            r#"{"type":"Point","coordinates":{"x":0}}"#,
            r#"{"type":"LineString","coordinates":[[0,0]]}"#,
            r#"{"type":"LineString","coordinates":[0,0,1,1]}"#,
            // Not closed, and too few positions.
            r#"{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,1]]]}"#,
            r#"{"type":"Polygon","coordinates":[[[0,0],[1,0],[0,0]]]}"#,
            r#"{"type":"MultiPoint","coordinates":[[]]}"#,
            r#"{"type":"GeometryCollection","geometries":[{"type":"Point","coordinates":[0]}]}"#,
            r#"{"type":"GeometryCollection"}"#,
        ] {
            assert_eq!(wkt(bad), None, "{bad:?} must not parse");
        }
        // Pathologically deep nesting is refused by the JSON reader's own
        // recursion limit rather than overflowing the stack.
        let deep = format!(
            "{}{{\"type\":\"Point\",\"coordinates\":[0,0]}}{}",
            r#"{"type":"GeometryCollection","geometries":["#.repeat(200),
            "]}".repeat(200)
        );
        assert_eq!(wkt(&deep), None);
    }

    fn identity(x: f64, y: f64) -> Option<(f64, f64)> {
        Some((x, y))
    }

    #[test]
    fn geos_geometries_write_as_geojson_and_read_back() {
        for w in [
            "POINT (1.5 -2)",
            "LINESTRING (0 0, 1 1, 2 0)",
            "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (1 1, 2 1, 2 2, 1 1))",
            "MULTIPOINT ((1 2), (3 4))",
            "MULTILINESTRING ((0 0, 1 1), (2 2, 3 3))",
            "MULTIPOLYGON (((0 0, 1 0, 1 1, 0 0)), ((2 2, 3 2, 3 3, 2 2)))",
            "GEOMETRYCOLLECTION (POINT (0 0), LINESTRING (1 1, 2 2))",
        ] {
            let g = Geometry::new_from_wkt(w).unwrap();
            let json = geometry_to_geojson(&g, &identity).unwrap();
            let back = Geometry::new_from_wkt(&wkt(&json.to_string()).unwrap()).unwrap();
            assert!(
                g.equals_exact(&back, 0.0).unwrap(),
                "{w} -> {json} -> {:?}",
                back.to_wkt()
            );
        }
    }

    #[test]
    fn empty_geometries_write_as_empty_coordinates() {
        let g = Geometry::new_from_wkt("POINT EMPTY").unwrap();
        assert_eq!(
            geometry_to_geojson(&g, &identity).unwrap(),
            json!({ "type": "Point", "coordinates": [] })
        );
        let g = Geometry::new_from_wkt("GEOMETRYCOLLECTION EMPTY").unwrap();
        assert_eq!(
            geometry_to_geojson(&g, &identity).unwrap(),
            json!({ "type": "GeometryCollection", "geometries": [] })
        );
    }

    #[test]
    fn a_coordinate_that_does_not_transform_fails_the_whole_geometry() {
        let g = Geometry::new_from_wkt("LINESTRING (0 0, 1 1)").unwrap();
        let refuse = |x: f64, y: f64| (x < 0.5).then_some((x, y));
        assert!(geometry_to_geojson(&g, &refuse).is_none());
    }
}
