//! Minimal GML 3.2 → WKT conversion for parsing `geo:gmlLiteral` geometries.
//!
//! GEOS (via the `geos` crate) exposes no GML reader, so we translate the
//! GeoSPARQL-relevant GML subset — `Point`, `LineString`/`Curve`,
//! `Polygon`/`Surface`, and the `Multi*` collections — into WKT, which the
//! existing WKT path then parses with GEOS. Coordinates are read from
//! `gml:pos` / `gml:posList` / `gml:coordinates`, grouped per `srsDimension`
//! (2 by default; 3D drops the Z ordinate, anything else is rejected); the
//! `srsName`/CRS is the caller's concern. Returns `None` for an unrecognised or
//! malformed document rather than panicking.

use quick_xml::events::Event;
use quick_xml::Reader;

/// Flattened XML event. `End` carries no name — nesting is tracked by depth, and
/// every `Start` (including an expanded empty element) has exactly one matching `End`.
/// `Start` carries the element's `srsDimension` attribute, if present (it may sit
/// on the geometry element or on `pos`/`posList`); an unparseable value becomes
/// `Some(0)` so dimension validation rejects the geometry.
#[derive(Debug)]
enum Ev {
    Start(String, Option<usize>),
    End,
    Text(String),
}

#[derive(Clone, Copy)]
enum MultiKind {
    Point,
    Line,
    Poly,
    Geom,
}

/// Parsed geometry, ready to serialise to WKT.
enum G {
    Point((f64, f64)),
    Line(Vec<(f64, f64)>),
    /// Rings, exterior first (GML always orders the exterior boundary first).
    Poly(Vec<Vec<(f64, f64)>>),
    Multi(MultiKind, Vec<G>),
}

/// Extract the `srsName` CRS URI from the outermost GML geometry element, if any
/// (e.g. `<gml:Point srsName="http://www.opengis.net/def/crs/EPSG/0/28992">`).
pub fn gml_srs_name(gml: &str) -> Option<String> {
    let mut reader = Reader::from_str(gml);
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                for attr in e.attributes().flatten() {
                    if attr.key.local_name().as_ref() == b"srsName" {
                        if let Ok(v) = attr.unescape_value() {
                            return Some(v.to_string());
                        }
                    }
                }
                // Keep scanning: the srsName may sit on a nested geometry element.
            }
            Ok(Event::Eof) | Err(_) => return None,
            _ => {}
        }
    }
}

/// Convert a GML geometry document to a WKT string, or `None` if it is not a
/// recognised/parseable GML geometry.
pub fn gml_to_wkt(gml: &str) -> Option<String> {
    let ev = tokenize(gml);
    let mut i = 0;
    while i < ev.len() {
        if let Ev::Start(n, _) = &ev[i] {
            if is_geometry(n) {
                return parse_geometry(&ev, i, None).map(|g| to_wkt(&g));
            }
        }
        i += 1;
    }
    None
}

fn tokenize(gml: &str) -> Vec<Ev> {
    let mut reader = Reader::from_str(gml);
    reader.config_mut().trim_text(true);
    let mut out = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => out.push(Ev::Start(
                local_name(e.local_name().as_ref()),
                srs_dimension(&e),
            )),
            Ok(Event::Empty(e)) => {
                out.push(Ev::Start(
                    local_name(e.local_name().as_ref()),
                    srs_dimension(&e),
                ));
                out.push(Ev::End);
            }
            Ok(Event::End(_)) => out.push(Ev::End),
            Ok(Event::Text(e)) => {
                if let Ok(t) = e.decode().map_err(|_| ()).and_then(|s| {
                    quick_xml::escape::unescape(&s)
                        .map(|u| u.into_owned())
                        .map_err(|_| ())
                }) {
                    let s = t.trim();
                    if !s.is_empty() {
                        out.push(Ev::Text(s.to_string()));
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => return Vec::new(),
            _ => {}
        }
    }
    out
}

fn local_name(b: &[u8]) -> String {
    String::from_utf8_lossy(b).to_string()
}

/// `srsDimension` attribute of an element, if present; unparseable → `Some(0)`
/// (an invalid dimension, so the geometry is rejected downstream).
fn srs_dimension(e: &quick_xml::events::BytesStart) -> Option<usize> {
    for attr in e.attributes().flatten() {
        if attr.key.local_name().as_ref() == b"srsDimension" {
            let dim = attr
                .unescape_value()
                .ok()
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0);
            return Some(dim);
        }
    }
    None
}

fn is_geometry(name: &str) -> bool {
    matches!(
        name,
        "Point"
            | "LineString"
            | "Curve"
            | "Polygon"
            | "Surface"
            | "MultiPoint"
            | "MultiCurve"
            | "MultiLineString"
            | "MultiSurface"
            | "MultiPolygon"
            | "MultiGeometry"
    )
}

/// Index of the `End` matching the `Start` at `start`.
fn subtree_end(ev: &[Ev], start: usize) -> usize {
    let mut depth = 0i32;
    let mut i = start;
    while i < ev.len() {
        match ev[i] {
            Ev::Start(..) => depth += 1,
            Ev::End => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
        i += 1;
    }
    ev.len()
}

/// All `(x, y)` pairs found in the `Text` of `ev[lo..hi]`. Within a GML geometry,
/// text only appears inside `gml:pos`/`gml:posList`/`gml:coordinates`, so this
/// collects every coordinate number (split on whitespace and commas) and groups
/// them per the effective `srsDimension` — inherited from the enclosing geometry
/// (`dim`) unless overridden on a nested element (e.g. `posList`), defaulting to
/// 2. 3D drops the Z ordinate; any other dimension returns `None`.
fn coords_in(ev: &[Ev], lo: usize, hi: usize, dim: Option<usize>) -> Option<Vec<(f64, f64)>> {
    let mut dim = dim;
    let mut nums = Vec::new();
    for e in &ev[lo..hi.min(ev.len())] {
        match e {
            Ev::Start(_, Some(d)) => dim = Some(*d),
            Ev::Text(t) => {
                for tok in t.split([' ', ',', '\n', '\t', '\r']) {
                    let tok = tok.trim();
                    if !tok.is_empty() {
                        if let Ok(n) = tok.parse::<f64>() {
                            nums.push(n);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let dim = dim.unwrap_or(2);
    if !(2..=3).contains(&dim) {
        return None;
    }
    Some(
        nums.chunks(dim)
            .filter(|c| c.len() == dim)
            .map(|c| (c[0], c[1]))
            .collect(),
    )
}

/// `dim` is the `srsDimension` inherited from an enclosing element; the
/// geometry's own attribute (and nested `pos`/`posList` ones) override it.
fn parse_geometry(ev: &[Ev], idx: usize, dim: Option<usize>) -> Option<G> {
    let (name, dim) = match &ev[idx] {
        Ev::Start(n, d) => (n.as_str(), d.or(dim)),
        _ => return None,
    };
    let end = subtree_end(ev, idx);
    match name {
        "Point" => coords_in(ev, idx + 1, end, dim)?
            .into_iter()
            .next()
            .map(G::Point),
        "LineString" | "Curve" => {
            let c = coords_in(ev, idx + 1, end, dim)?;
            (!c.is_empty()).then_some(G::Line(c))
        }
        "Polygon" | "Surface" => {
            // Each LinearRing subtree is one ring, in document order (exterior first).
            let mut rings = Vec::new();
            let mut i = idx + 1;
            while i < end {
                if matches!(&ev[i], Ev::Start(n, _) if n == "LinearRing") {
                    let re = subtree_end(ev, i);
                    let r = coords_in(ev, i + 1, re, dim)?;
                    if !r.is_empty() {
                        rings.push(r);
                    }
                    i = re + 1;
                    continue;
                }
                i += 1;
            }
            (!rings.is_empty()).then_some(G::Poly(rings))
        }
        "MultiPoint" => parse_members(ev, idx + 1, end, dim, MultiKind::Point),
        "MultiCurve" | "MultiLineString" => parse_members(ev, idx + 1, end, dim, MultiKind::Line),
        "MultiSurface" | "MultiPolygon" => parse_members(ev, idx + 1, end, dim, MultiKind::Poly),
        "MultiGeometry" => parse_members(ev, idx + 1, end, dim, MultiKind::Geom),
        _ => None,
    }
}

fn parse_members(
    ev: &[Ev],
    lo: usize,
    hi: usize,
    dim: Option<usize>,
    kind: MultiKind,
) -> Option<G> {
    let mut items = Vec::new();
    let mut i = lo;
    while i < hi {
        if matches!(&ev[i], Ev::Start(n, _) if is_geometry(n)) {
            if let Some(g) = parse_geometry(ev, i, dim) {
                items.push(g);
            }
            i = subtree_end(ev, i) + 1;
            continue;
        }
        i += 1;
    }
    (!items.is_empty()).then_some(G::Multi(kind, items))
}

fn fmt(v: f64) -> String {
    // `{}` drops a trailing `.0`; GEOS parses both forms, this keeps output tidy.
    format!("{v}")
}

fn pair(p: &(f64, f64)) -> String {
    format!("{} {}", fmt(p.0), fmt(p.1))
}

fn ring(r: &[(f64, f64)]) -> String {
    format!("({})", r.iter().map(pair).collect::<Vec<_>>().join(", "))
}

fn to_wkt(g: &G) -> String {
    match g {
        G::Point(p) => format!("POINT({})", pair(p)),
        G::Line(ps) => format!(
            "LINESTRING({})",
            ps.iter().map(pair).collect::<Vec<_>>().join(", ")
        ),
        G::Poly(rings) => format!(
            "POLYGON({})",
            rings.iter().map(|r| ring(r)).collect::<Vec<_>>().join(", ")
        ),
        G::Multi(MultiKind::Point, items) => format!(
            "MULTIPOINT({})",
            items
                .iter()
                .filter_map(|g| match g {
                    G::Point(p) => Some(format!("({})", pair(p))),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(", ")
        ),
        G::Multi(MultiKind::Line, items) => format!(
            "MULTILINESTRING({})",
            items
                .iter()
                .filter_map(|g| match g {
                    G::Line(ps) => Some(format!(
                        "({})",
                        ps.iter().map(pair).collect::<Vec<_>>().join(", ")
                    )),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(", ")
        ),
        G::Multi(MultiKind::Poly, items) => format!(
            "MULTIPOLYGON({})",
            items
                .iter()
                .filter_map(|g| match g {
                    G::Poly(rings) => Some(format!(
                        "({})",
                        rings.iter().map(|r| ring(r)).collect::<Vec<_>>().join(", ")
                    )),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(", ")
        ),
        G::Multi(MultiKind::Geom, items) => format!(
            "GEOMETRYCOLLECTION({})",
            items.iter().map(to_wkt).collect::<Vec<_>>().join(", ")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point() {
        let g = "<gml:Point srsName=\"EPSG:28992\"><gml:pos>187330 428345</gml:pos></gml:Point>";
        assert_eq!(gml_to_wkt(g).as_deref(), Some("POINT(187330 428345)"));
    }

    #[test]
    fn linestring_poslist() {
        let g = "<gml:LineString><gml:posList>0 0 1 1 2 0</gml:posList></gml:LineString>";
        assert_eq!(gml_to_wkt(g).as_deref(), Some("LINESTRING(0 0, 1 1, 2 0)"));
    }

    #[test]
    fn polygon_with_hole() {
        let g = "<gml:Polygon><gml:exterior><gml:LinearRing><gml:posList>0 0 10 0 10 10 0 10 0 0</gml:posList></gml:LinearRing></gml:exterior><gml:interior><gml:LinearRing><gml:posList>3 3 4 3 4 4 3 4 3 3</gml:posList></gml:LinearRing></gml:interior></gml:Polygon>";
        assert_eq!(
            gml_to_wkt(g).as_deref(),
            Some("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0), (3 3, 4 3, 4 4, 3 4, 3 3))")
        );
    }

    #[test]
    fn multipoint() {
        let g = "<gml:MultiPoint><gml:pointMember><gml:Point><gml:pos>1 2</gml:pos></gml:Point></gml:pointMember><gml:pointMember><gml:Point><gml:pos>3 4</gml:pos></gml:Point></gml:pointMember></gml:MultiPoint>";
        assert_eq!(gml_to_wkt(g).as_deref(), Some("MULTIPOINT((1 2), (3 4))"));
    }

    #[test]
    fn linestring_3d_drops_z() {
        // srsDimension on the geometry element…
        let g = "<gml:LineString srsDimension=\"3\"><gml:posList>0 0 10 1 1 10</gml:posList></gml:LineString>";
        assert_eq!(gml_to_wkt(g).as_deref(), Some("LINESTRING(0 0, 1 1)"));
        // …or on the posList itself.
        let g = "<gml:LineString><gml:posList srsDimension=\"3\">0 0 10 1 1 10</gml:posList></gml:LineString>";
        assert_eq!(gml_to_wkt(g).as_deref(), Some("LINESTRING(0 0, 1 1)"));
    }

    #[test]
    fn point_3d_drops_z() {
        let g = "<gml:Point srsDimension=\"3\"><gml:pos>4 52 12.5</gml:pos></gml:Point>";
        assert_eq!(gml_to_wkt(g).as_deref(), Some("POINT(4 52)"));
    }

    #[test]
    fn unsupported_srs_dimension_is_none() {
        let g = "<gml:LineString srsDimension=\"4\"><gml:posList>0 0 1 1 2 2 3 3</gml:posList></gml:LineString>";
        assert_eq!(gml_to_wkt(g), None);
    }
}
