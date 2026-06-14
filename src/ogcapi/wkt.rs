//! Minimal WKT (WGS84, `(lon lat)` axis order) → GeoJSON-geometry converter.
//!
//! The viewer feed already reprojects geometry to EPSG:4326 and stores it as a
//! plain WKT string with **no** CRS prefix (see [`crate::geo::viewer_feed`]), so
//! this parser deliberately rejects any leading `<…>` CRS token rather than
//! guessing. It covers the OGC-Simple-Features primitives the feed can emit —
//! `POINT`, `LINESTRING`, `POLYGON`, their `MULTI*` forms, and
//! `GEOMETRYCOLLECTION`. Z/M coordinates are tolerated (extra ordinates past the
//! first two are dropped) so a 3D `POINT Z (x y z)` still maps cleanly.
//!
//! Unparseable input yields `None`; callers skip such elements.

use serde_json::{json, Value};

/// A parsed planar bounding box in `(minx, miny, maxx, maxy)` = `(lon, lat)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BBox {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl BBox {
    /// Two bboxes overlap iff they overlap on both axes (closed intervals).
    pub fn intersects(&self, other: &BBox) -> bool {
        self.min_x <= other.max_x
            && self.max_x >= other.min_x
            && self.min_y <= other.max_y
            && self.max_y >= other.min_y
    }
}

/// Convert a single WGS84 WKT string into a GeoJSON geometry object.
///
/// Returns `None` for empty, CRS-prefixed, or syntactically-invalid input.
pub fn wkt_to_geojson(wkt: &str) -> Option<Value> {
    let s = wkt.trim();
    // A CRS-prefixed literal (`<uri> POINT(...)`) is not plain WGS84 WKT here.
    if s.starts_with('<') {
        return None;
    }
    let (kind, rest) = split_kind(s)?;
    match kind.as_str() {
        "POINT" => {
            let c = parse_coord(strip_parens(rest)?)?;
            Some(json!({ "type": "Point", "coordinates": [c.0, c.1] }))
        }
        "LINESTRING" => {
            let coords = parse_coord_list(strip_parens(rest)?)?;
            if coords.is_empty() {
                return None;
            }
            Some(json!({ "type": "LineString", "coordinates": coords_to_json(&coords) }))
        }
        "POLYGON" => {
            let rings = parse_rings(strip_parens(rest)?)?;
            if rings.is_empty() {
                return None;
            }
            Some(json!({ "type": "Polygon", "coordinates": rings_to_json(&rings) }))
        }
        "MULTIPOINT" => {
            // Both `MULTIPOINT(1 2, 3 4)` and `MULTIPOINT((1 2),(3 4))` are legal.
            let inner = strip_parens(rest)?;
            let coords = if inner.contains('(') {
                parse_rings(inner)?.into_iter().filter_map(|r| r.into_iter().next()).collect()
            } else {
                parse_coord_list(inner)?
            };
            if coords.is_empty() {
                return None;
            }
            Some(json!({ "type": "MultiPoint", "coordinates": coords_to_json(&coords) }))
        }
        "MULTILINESTRING" => {
            let lines = parse_rings(strip_parens(rest)?)?;
            if lines.is_empty() {
                return None;
            }
            Some(json!({ "type": "MultiLineString", "coordinates": rings_to_json(&lines) }))
        }
        "MULTIPOLYGON" => {
            let polys = parse_multi_polygon(strip_parens(rest)?)?;
            if polys.is_empty() {
                return None;
            }
            let json_polys: Vec<Value> = polys.iter().map(|p| json!(rings_to_json(p))).collect();
            Some(json!({ "type": "MultiPolygon", "coordinates": json_polys }))
        }
        "GEOMETRYCOLLECTION" => {
            let geoms = parse_geometry_collection(strip_parens(rest)?)?;
            if geoms.is_empty() {
                return None;
            }
            Some(json!({ "type": "GeometryCollection", "geometries": geoms }))
        }
        _ => None,
    }
}

/// The planar bounding box of a WKT geometry, or `None` if it has no coordinates.
pub fn wkt_bbox(wkt: &str) -> Option<BBox> {
    let geom = wkt_to_geojson(wkt)?;
    geojson_bbox(&geom)
}

/// Recursively compute the bbox of a GeoJSON geometry value.
fn geojson_bbox(geom: &Value) -> Option<BBox> {
    let mut bb: Option<BBox> = None;
    let mut extend = |x: f64, y: f64| {
        bb = Some(match bb {
            None => BBox { min_x: x, min_y: y, max_x: x, max_y: y },
            Some(b) => BBox {
                min_x: b.min_x.min(x),
                min_y: b.min_y.min(y),
                max_x: b.max_x.max(x),
                max_y: b.max_y.max(y),
            },
        });
    };
    fn walk(v: &Value, f: &mut dyn FnMut(f64, f64)) {
        // A coordinate is the innermost array of two finite numbers.
        if let Some(arr) = v.as_array() {
            if arr.len() >= 2 && arr[0].is_number() && arr[1].is_number() {
                if let (Some(x), Some(y)) = (arr[0].as_f64(), arr[1].as_f64()) {
                    f(x, y);
                    return;
                }
            }
            for item in arr {
                walk(item, f);
            }
        }
    }
    if let Some(members) = geom.get("geometries").and_then(|g| g.as_array()) {
        for m in members {
            if let Some(c) = m.get("coordinates") {
                walk(c, &mut extend);
            }
        }
    } else if let Some(c) = geom.get("coordinates") {
        walk(c, &mut extend);
    }
    bb
}

// ── parsing internals ───────────────────────────────────────────────────────

/// Split a leading uppercase geometry keyword (with optional ` Z`/` M`/` ZM`
/// dimensionality flag and `EMPTY`) from the rest of the string.
fn split_kind(s: &str) -> Option<(String, &str)> {
    let s = s.trim_start();
    let end = s.find(|c: char| c == '(' || c.is_whitespace()).unwrap_or(s.len());
    let kind = s[..end].to_ascii_uppercase();
    if kind.is_empty() {
        return None;
    }
    let mut rest = s[end..].trim_start();
    // Drop a Z / M / ZM dimensionality flag if present.
    for flag in ["ZM", "Z", "M"] {
        if let Some(stripped) = rest.strip_prefix(flag) {
            if stripped.trim_start().starts_with('(')
                || stripped.trim_start().eq_ignore_ascii_case("EMPTY")
                || stripped.is_empty()
            {
                rest = stripped.trim_start();
                break;
            }
        }
    }
    Some((kind, rest))
}

/// Strip one pair of outermost parentheses, returning the inner content.
fn strip_parens(s: &str) -> Option<&str> {
    let s = s.trim();
    let inner = s.strip_prefix('(')?.strip_suffix(')')?;
    Some(inner.trim())
}

/// Parse `"x y[ z[ m]]"` into an `(x, y)` pair, dropping extra ordinates.
fn parse_coord(s: &str) -> Option<(f64, f64)> {
    let mut it = s.split_whitespace();
    let x: f64 = it.next()?.parse().ok()?;
    let y: f64 = it.next()?.parse().ok()?;
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    Some((x, y))
}

/// Parse a comma-separated coordinate list (`"1 2, 3 4, …"`).
fn parse_coord_list(s: &str) -> Option<Vec<(f64, f64)>> {
    s.split(',')
        .map(|c| parse_coord(c.trim()))
        .collect::<Option<Vec<_>>>()
}

/// Parse a parenthesised list of coordinate lists — rings of a POLYGON, lines
/// of a MULTILINESTRING, or the inner of a `MULTIPOINT((x y),…)`.
fn parse_rings(s: &str) -> Option<Vec<Vec<(f64, f64)>>> {
    split_top_level(s)
        .into_iter()
        .map(|part| parse_coord_list(strip_parens(part)?))
        .collect::<Option<Vec<_>>>()
}

/// Parse the inner of a MULTIPOLYGON: a list of polygons, each a list of rings.
fn parse_multi_polygon(s: &str) -> Option<Vec<Vec<Vec<(f64, f64)>>>> {
    split_top_level(s)
        .into_iter()
        .map(|part| parse_rings(strip_parens(part)?))
        .collect::<Option<Vec<_>>>()
}

/// Parse the inner of a GEOMETRYCOLLECTION into GeoJSON member geometries.
fn parse_geometry_collection(s: &str) -> Option<Vec<Value>> {
    if s.trim().is_empty() {
        return Some(Vec::new());
    }
    // Each top-level member is itself a full WKT sub-geometry (keyword + parens).
    split_top_level(s)
        .into_iter()
        .map(wkt_to_geojson)
        .collect::<Option<Vec<_>>>()
}

/// Split a string on top-level commas (commas not nested inside parentheses).
fn split_top_level(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, ch) in s.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    let tail = s[start..].trim();
    if !tail.is_empty() {
        parts.push(tail);
    }
    parts
}

fn coords_to_json(coords: &[(f64, f64)]) -> Vec<Value> {
    coords.iter().map(|(x, y)| json!([x, y])).collect()
}

fn rings_to_json(rings: &[Vec<(f64, f64)>]) -> Vec<Value> {
    rings.iter().map(|r| json!(coords_to_json(r))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn point() {
        let g = wkt_to_geojson("POINT(5.86 51.85)").unwrap();
        assert_eq!(g, json!({ "type": "Point", "coordinates": [5.86, 51.85] }));
    }

    #[test]
    fn point_z_drops_third_ordinate() {
        let g = wkt_to_geojson("POINT Z (5.0 51.0 12.0)").unwrap();
        assert_eq!(g, json!({ "type": "Point", "coordinates": [5.0, 51.0] }));
    }

    #[test]
    fn linestring() {
        let g = wkt_to_geojson("LINESTRING(0 0, 1 1, 2 0)").unwrap();
        assert_eq!(
            g,
            json!({ "type": "LineString", "coordinates": [[0.0, 0.0], [1.0, 1.0], [2.0, 0.0]] })
        );
    }

    #[test]
    fn polygon_with_hole() {
        let g = wkt_to_geojson("POLYGON((0 0, 4 0, 4 4, 0 4, 0 0),(1 1, 2 1, 2 2, 1 1))").unwrap();
        assert_eq!(g["type"], "Polygon");
        let rings = g["coordinates"].as_array().unwrap();
        assert_eq!(rings.len(), 2, "outer ring + hole");
        assert_eq!(rings[0].as_array().unwrap().len(), 5);
    }

    #[test]
    fn multipoint_both_syntaxes() {
        let a = wkt_to_geojson("MULTIPOINT(1 2, 3 4)").unwrap();
        let b = wkt_to_geojson("MULTIPOINT((1 2),(3 4))").unwrap();
        let expected = json!({ "type": "MultiPoint", "coordinates": [[1.0, 2.0], [3.0, 4.0]] });
        assert_eq!(a, expected);
        assert_eq!(b, expected);
    }

    #[test]
    fn multilinestring() {
        let g = wkt_to_geojson("MULTILINESTRING((0 0, 1 1),(2 2, 3 3))").unwrap();
        assert_eq!(g["type"], "MultiLineString");
        assert_eq!(g["coordinates"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn multipolygon() {
        let g = wkt_to_geojson("MULTIPOLYGON(((0 0, 1 0, 1 1, 0 0)),((2 2, 3 2, 3 3, 2 2)))")
            .unwrap();
        assert_eq!(g["type"], "MultiPolygon");
        let polys = g["coordinates"].as_array().unwrap();
        assert_eq!(polys.len(), 2);
        // each polygon is a list of rings; each ring a list of coords
        assert_eq!(polys[0].as_array().unwrap()[0].as_array().unwrap().len(), 4);
    }

    #[test]
    fn geometry_collection() {
        let g = wkt_to_geojson("GEOMETRYCOLLECTION(POINT(0 0), LINESTRING(1 1, 2 2))").unwrap();
        assert_eq!(g["type"], "GeometryCollection");
        let members = g["geometries"].as_array().unwrap();
        assert_eq!(members.len(), 2);
        assert_eq!(members[0]["type"], "Point");
        assert_eq!(members[1]["type"], "LineString");
    }

    #[test]
    fn crs_prefixed_is_rejected() {
        assert!(wkt_to_geojson("<http://www.opengis.net/def/crs/EPSG/0/28992> POINT(0 0)").is_none());
    }

    #[test]
    fn garbage_is_none() {
        assert!(wkt_to_geojson("").is_none());
        assert!(wkt_to_geojson("NOTAGEOM(1 2)").is_none());
        assert!(wkt_to_geojson("POINT(abc def)").is_none());
        assert!(wkt_to_geojson("POINT(1)").is_none());
    }

    #[test]
    fn bbox_of_polygon() {
        let bb = wkt_bbox("POLYGON((0 0, 4 0, 4 3, 0 3, 0 0))").unwrap();
        assert_eq!(bb, BBox { min_x: 0.0, min_y: 0.0, max_x: 4.0, max_y: 3.0 });
    }

    #[test]
    fn bbox_of_point() {
        let bb = wkt_bbox("POINT(5.86 51.85)").unwrap();
        assert_eq!(bb, BBox { min_x: 5.86, min_y: 51.85, max_x: 5.86, max_y: 51.85 });
    }

    #[test]
    fn bbox_of_collection() {
        let bb = wkt_bbox("GEOMETRYCOLLECTION(POINT(0 0), POINT(10 20))").unwrap();
        assert_eq!(bb, BBox { min_x: 0.0, min_y: 0.0, max_x: 10.0, max_y: 20.0 });
    }

    #[test]
    fn bbox_intersection() {
        let a = BBox { min_x: 0.0, min_y: 0.0, max_x: 10.0, max_y: 10.0 };
        let inside = BBox { min_x: 1.0, min_y: 1.0, max_x: 2.0, max_y: 2.0 };
        let overlap = BBox { min_x: 5.0, min_y: 5.0, max_x: 15.0, max_y: 15.0 };
        let disjoint = BBox { min_x: 20.0, min_y: 20.0, max_x: 30.0, max_y: 30.0 };
        let edge = BBox { min_x: 10.0, min_y: 10.0, max_x: 12.0, max_y: 12.0 };
        assert!(a.intersects(&inside));
        assert!(a.intersects(&overlap));
        assert!(!a.intersects(&disjoint));
        assert!(a.intersects(&edge), "touching edges count as intersecting");
    }
}
