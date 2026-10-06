//! `geo:kmlLiteral` — OGC KML 2.2 geometry elements, read and written.
//!
//! GeoSPARQL 1.1 (Req 30–34) defines a `geo:kmlLiteral` as a KML geometry
//! element. KML coordinates are always WGS84 longitude, latitude and an optional
//! altitude — CRS84, GeoSPARQL's default — so a KML literal names no CRS.
//!
//! Read: `Point`, `LineString`, `LinearRing` (a closed line), `Polygon` (an
//! `outerBoundaryIs` and any number of `innerBoundaryIs` rings) and
//! `MultiGeometry` (a `MULTIPOINT`, `MULTILINESTRING` or `MULTIPOLYGON` when its
//! members are all of one kind, a `GEOMETRYCOLLECTION` otherwise). The
//! geometry may sit inside a `Placemark`; `extrude`, `tessellate` and
//! `altitudeMode` do not change it. An altitude on every tuple is kept as Z. An
//! empty literal, and an empty `MultiGeometry`, is the empty geometry. A `Model`,
//! `gx:Track` or `gx:MultiTrack` is not a geometry of the profile, and malformed
//! input — a tuple without two numbers, a ring that is not closed — is not a
//! geometry at all: functions over it are unbound, never a panic.
//!
//! Written by `geof:asKML` (reprojected to CRS84 first) and by the constructive
//! functions on a KML operand: [`geometry_to_kml`].

use super::gml::{from_geos, Doc, MultiKind, G, P};

/// The KML 2.2 namespace, as [`geometry_to_kml`] declares it.
pub const KML_NAMESPACE: &str = "http://www.opengis.net/kml/2.2";

const MAX_DEPTH: usize = 32;

/// Convert a KML geometry to WKT, or `None` if it is not one of the profile.
pub fn kml_to_wkt(kml: &str) -> Option<String> {
    kml_to_geometry(kml).map(|g| super::gml::to_wkt(&g))
}

fn is_geometry(name: &str) -> bool {
    matches!(
        name,
        "Point"
            | "LineString"
            | "LinearRing"
            | "Polygon"
            | "MultiGeometry"
            | "Model"
            | "Track"
            | "MultiTrack"
    )
}

pub(super) fn kml_to_geometry(kml: &str) -> Option<G> {
    let doc = Doc::parse(kml)?;
    let root = doc.first(is_geometry)?;
    geometry(&doc, root, 0)
}

fn geometry(doc: &Doc, i: usize, depth: usize) -> Option<G> {
    if depth > MAX_DEPTH {
        return None;
    }
    let (name, _) = doc.start(i)?;
    match name {
        "Point" => match coordinates(doc, i)?.as_slice() {
            [p] => Some(G::Point(Some(*p))),
            _ => None,
        },
        "LineString" => {
            let ps = coordinates(doc, i)?;
            (ps.len() >= 2).then_some(G::Line(ps))
        }
        "LinearRing" => ring(doc, i).map(G::Line),
        "Polygon" => {
            let outer = doc.children_named(i, &["outerBoundaryIs"]);
            let [o] = outer.as_slice() else {
                return None;
            };
            let [exterior] = <[Vec<P>; 1]>::try_from(boundary_rings(doc, *o)?).ok()?;
            let mut rings = vec![exterior];
            for b in doc.children_named(i, &["innerBoundaryIs"]) {
                rings.extend(boundary_rings(doc, b)?);
            }
            Some(G::Poly(rings))
        }
        "MultiGeometry" => {
            let items = doc
                .children(i)
                .into_iter()
                .filter(|&c| doc.start(c).is_some_and(|(n, _)| is_geometry(n)))
                .map(|c| geometry(doc, c, depth + 1))
                .collect::<Option<Vec<_>>>()?;
            let all = |f: fn(&G) -> bool| !items.is_empty() && items.iter().all(f);
            let kind = if all(|g| matches!(g, G::Point(_))) {
                MultiKind::Point
            } else if all(|g| matches!(g, G::Line(_))) {
                MultiKind::Line
            } else if all(|g| matches!(g, G::Poly(_))) {
                MultiKind::Poly
            } else {
                MultiKind::Geom
            };
            Some(G::Multi(kind, items))
        }
        // Model, gx:Track, gx:MultiTrack: outside the profile.
        _ => None,
    }
}

/// The `LinearRing`s of an `outerBoundaryIs`/`innerBoundaryIs`.
fn boundary_rings(doc: &Doc, b: usize) -> Option<Vec<Vec<P>>> {
    let rings = doc.children_named(b, &["LinearRing"]);
    if rings.is_empty() {
        return None;
    }
    rings.into_iter().map(|r| ring(doc, r)).collect()
}

/// A closed ring of four or more tuples.
fn ring(doc: &Doc, i: usize) -> Option<Vec<P>> {
    let ps = coordinates(doc, i)?;
    (ps.len() >= 4 && ps.first() == ps.last()).then_some(ps)
}

/// The tuples of the `coordinates` child of `i`: `lon,lat[,alt]`, separated by
/// whitespace. Whitespace after a comma, which some writers emit, is tolerated.
fn coordinates(doc: &Doc, i: usize) -> Option<Vec<P>> {
    let [c] = doc.children_named(i, &["coordinates"])[..] else {
        return None;
    };
    let text = doc.text(c);
    let mut joined = String::with_capacity(text.len());
    let mut after_comma = false;
    for ch in text.chars() {
        if ch.is_whitespace() && after_comma {
            continue;
        }
        after_comma = ch == ',';
        joined.push(ch);
    }
    joined
        .split_whitespace()
        .map(|t| {
            let nums = t
                .split(',')
                .map(|n| n.parse::<f64>().ok().filter(|v| v.is_finite()))
                .collect::<Option<Vec<_>>>()?;
            match nums.as_slice() {
                [x, y] => Some(P {
                    x: *x,
                    y: *y,
                    z: None,
                }),
                [x, y, z] => Some(P {
                    x: *x,
                    y: *y,
                    z: Some(*z),
                }),
                _ => None,
            }
        })
        .collect()
}

/// A GEOS geometry as a KML 2.2 geometry element, every coordinate mapped
/// through `xy` (the reprojection into CRS84). The empty geometry is the empty
/// literal (Req 32). `None` when a coordinate does not transform or the
/// geometry is a curve.
pub fn geometry_to_kml(
    geom: &impl geos::Geom,
    xy: &dyn Fn(f64, f64) -> Option<(f64, f64)>,
) -> Option<String> {
    let g = from_geos(geom, xy)?;
    if g.is_empty() {
        return Some(String::new());
    }
    let z = g.has_z();
    let mut out = String::new();
    element(&g, z, &format!(" xmlns=\"{KML_NAMESPACE}\""), &mut out);
    Some(out)
}

fn tuple(p: &P, z: bool) -> String {
    match (z, p.z) {
        (true, Some(pz)) => format!("{},{},{}", p.x, p.y, pz),
        _ => format!("{},{}", p.x, p.y),
    }
}

fn coords(ps: &[P], z: bool) -> String {
    let parts: Vec<String> = ps.iter().map(|p| tuple(p, z)).collect();
    format!("<coordinates>{}</coordinates>", parts.join(" "))
}

fn element(g: &G, z: bool, attrs: &str, out: &mut String) {
    match g {
        G::Point(Some(p)) => out.push_str(&format!(
            "<Point{attrs}>{}</Point>",
            coords(std::slice::from_ref(p), z)
        )),
        G::Line(ps) => out.push_str(&format!(
            "<LineString{attrs}>{}</LineString>",
            coords(ps, z)
        )),
        G::Poly(rings) => {
            out.push_str(&format!("<Polygon{attrs}>"));
            for (i, r) in rings.iter().enumerate() {
                let b = if i == 0 {
                    "outerBoundaryIs"
                } else {
                    "innerBoundaryIs"
                };
                out.push_str(&format!(
                    "<{b}><LinearRing>{}</LinearRing></{b}>",
                    coords(r, z)
                ));
            }
            out.push_str("</Polygon>");
        }
        G::Multi(_, items) => {
            out.push_str(&format!("<MultiGeometry{attrs}>"));
            for item in items.iter().filter(|g| !g.is_empty()) {
                element(item, z, "", out);
            }
            out.push_str("</MultiGeometry>");
        }
        G::Point(None) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geos::Geom;

    #[test]
    fn every_element_reads_as_wkt() {
        let cases = [
            ("<Point><coordinates>5.86,51.85</coordinates></Point>", "POINT(5.86 51.85)"),
            ("<Point><coordinates>5.86,51.85,12.5</coordinates></Point>", "POINT Z (5.86 51.85 12.5)"),
            ("<LineString><tessellate>1</tessellate><coordinates>0,0 1,1\n 2,0</coordinates></LineString>", "LINESTRING(0 0, 1 1, 2 0)"),
            ("<LinearRing><coordinates>0,0 1,0 1,1 0,0</coordinates></LinearRing>", "LINESTRING(0 0, 1 0, 1 1, 0 0)"),
            (
                "<Polygon><outerBoundaryIs><LinearRing><coordinates>0,0 4,0 4,4 0,4 0,0</coordinates></LinearRing></outerBoundaryIs>\
                 <innerBoundaryIs><LinearRing><coordinates>1,1 2,1 2,2 1,1</coordinates></LinearRing></innerBoundaryIs></Polygon>",
                "POLYGON((0 0, 4 0, 4 4, 0 4, 0 0), (1 1, 2 1, 2 2, 1 1))",
            ),
            (
                "<MultiGeometry><Point><coordinates>1,2</coordinates></Point><Point><coordinates>3,4</coordinates></Point></MultiGeometry>",
                "MULTIPOINT((1 2), (3 4))",
            ),
            (
                "<MultiGeometry><Point><coordinates>1,2</coordinates></Point><LineString><coordinates>0,0 1,1</coordinates></LineString></MultiGeometry>",
                "GEOMETRYCOLLECTION(POINT(1 2), LINESTRING(0 0, 1 1))",
            ),
            ("<MultiGeometry/>", "GEOMETRYCOLLECTION EMPTY"),
            (
                "<Placemark xmlns='http://www.opengis.net/kml/2.2'><name>Pier 7</name><Point><coordinates>1, 2</coordinates></Point></Placemark>",
                "POINT(1 2)",
            ),
        ];
        for (kml, expected) in cases {
            assert_eq!(kml_to_wkt(kml).as_deref(), Some(expected), "{kml}");
        }
    }

    #[test]
    fn malformed_and_out_of_profile_input_is_none() {
        for bad in [
            "",
            "not xml <",
            "<Point/>",
            "<Point><coordinates>1</coordinates></Point>",
            "<Point><coordinates>1,a</coordinates></Point>",
            "<Point><coordinates>1,2,3,4</coordinates></Point>",
            "<LineString><coordinates>0,0</coordinates></LineString>",
            "<Polygon><outerBoundaryIs><LinearRing><coordinates>0,0 1,0 1,1 0,1</coordinates></LinearRing></outerBoundaryIs></Polygon>",
            "<Polygon/>",
            "<Model><Location><longitude>1</longitude></Location></Model>",
            "<gx:Track xmlns:gx='http://www.google.com/kml/ext/2.2'><gx:coord>1 2 3</gx:coord></gx:Track>",
            "<Point><coordinates>1,2</coordinates>",
        ] {
            assert_eq!(kml_to_wkt(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn written_kml_reads_back() {
        let identity = |x, y| Some((x, y));
        for w in [
            "POINT (1.5 -2)",
            "POINT Z (1 2 3)",
            "LINESTRING (0 0, 1 1, 2 0)",
            "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (1 1, 2 1, 2 2, 1 1))",
            "MULTIPOINT ((1 2), (3 4))",
            "MULTILINESTRING ((0 0, 1 1), (2 2, 3 3))",
            "MULTIPOLYGON (((0 0, 1 0, 1 1, 0 0)), ((2 2, 3 2, 3 3, 2 2)))",
            "GEOMETRYCOLLECTION (POINT (0 0), LINESTRING (1 1, 2 2))",
        ] {
            let g = geos::Geometry::new_from_wkt(w).unwrap();
            let kml = geometry_to_kml(&g, &identity).unwrap();
            let back = geos::Geometry::new_from_wkt(&kml_to_wkt(&kml).unwrap()).unwrap();
            assert!(
                g.equals_exact(&back, 0.0).unwrap(),
                "{w} -> {kml} -> {:?}",
                back.to_wkt()
            );
        }
        let empty = geos::Geometry::new_from_wkt("POINT EMPTY").unwrap();
        assert_eq!(geometry_to_kml(&empty, &identity).as_deref(), Some(""));
    }
}
