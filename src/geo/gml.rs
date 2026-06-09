//! Minimal GML 3.2 → WKT conversion for parsing `geo:gmlLiteral` geometries.
//!
//! GEOS (via the `geos` crate) exposes no GML reader, so we translate the
//! GeoSPARQL-relevant GML subset — `Point`, `LineString`/`Curve`,
//! `Polygon`/`Surface`, and the `Multi*` collections — into WKT, which the
//! existing WKT path then parses with GEOS. Coordinates are read from
//! `gml:pos` / `gml:posList` / `gml:coordinates` as 2D `(x y)` pairs (the
//! `srsName`/CRS is the caller's concern). Returns `None` for an unrecognised or
//! malformed document rather than panicking.

use quick_xml::events::Event;
use quick_xml::Reader;

/// Flattened XML event. `End` carries no name — nesting is tracked by depth, and
/// every `Start` (including an expanded empty element) has exactly one matching `End`.
#[derive(Debug)]
enum Ev {
    Start(String),
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

/// Convert a GML geometry document to a WKT string, or `None` if it is not a
/// recognised/parseable GML geometry.
pub fn gml_to_wkt(gml: &str) -> Option<String> {
    let ev = tokenize(gml);
    let mut i = 0;
    while i < ev.len() {
        if let Ev::Start(n) = &ev[i] {
            if is_geometry(n) {
                return parse_geometry(&ev, i).map(|g| to_wkt(&g));
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
            Ok(Event::Start(e)) => out.push(Ev::Start(local_name(e.local_name().as_ref()))),
            Ok(Event::Empty(e)) => {
                out.push(Ev::Start(local_name(e.local_name().as_ref())));
                out.push(Ev::End);
            }
            Ok(Event::End(_)) => out.push(Ev::End),
            Ok(Event::Text(e)) => {
                if let Ok(t) = e.unescape() {
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
            Ev::Start(_) => depth += 1,
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
/// collects every coordinate number (split on whitespace and commas) and pairs
/// them as 2D. 3D coordinate lists (`srsDimension="3"`) are not supported.
fn coords_in(ev: &[Ev], lo: usize, hi: usize) -> Vec<(f64, f64)> {
    let mut nums = Vec::new();
    for e in &ev[lo..hi.min(ev.len())] {
        if let Ev::Text(t) = e {
            for tok in t.split([' ', ',', '\n', '\t', '\r']) {
                let tok = tok.trim();
                if !tok.is_empty() {
                    if let Ok(n) = tok.parse::<f64>() {
                        nums.push(n);
                    }
                }
            }
        }
    }
    nums.chunks(2)
        .filter(|c| c.len() == 2)
        .map(|c| (c[0], c[1]))
        .collect()
}

fn parse_geometry(ev: &[Ev], idx: usize) -> Option<G> {
    let name = match &ev[idx] {
        Ev::Start(n) => n.as_str(),
        _ => return None,
    };
    let end = subtree_end(ev, idx);
    match name {
        "Point" => coords_in(ev, idx + 1, end).into_iter().next().map(G::Point),
        "LineString" | "Curve" => {
            let c = coords_in(ev, idx + 1, end);
            (!c.is_empty()).then_some(G::Line(c))
        }
        "Polygon" | "Surface" => {
            // Each LinearRing subtree is one ring, in document order (exterior first).
            let mut rings = Vec::new();
            let mut i = idx + 1;
            while i < end {
                if matches!(&ev[i], Ev::Start(n) if n == "LinearRing") {
                    let re = subtree_end(ev, i);
                    let r = coords_in(ev, i + 1, re);
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
        "MultiPoint" => parse_members(ev, idx + 1, end, MultiKind::Point),
        "MultiCurve" | "MultiLineString" => parse_members(ev, idx + 1, end, MultiKind::Line),
        "MultiSurface" | "MultiPolygon" => parse_members(ev, idx + 1, end, MultiKind::Poly),
        "MultiGeometry" => parse_members(ev, idx + 1, end, MultiKind::Geom),
        _ => None,
    }
}

fn parse_members(ev: &[Ev], lo: usize, hi: usize, kind: MultiKind) -> Option<G> {
    let mut items = Vec::new();
    let mut i = lo;
    while i < hi {
        if matches!(&ev[i], Ev::Start(n) if is_geometry(n)) {
            if let Some(g) = parse_geometry(ev, i) {
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
    fn malformed_is_none() {
        assert_eq!(gml_to_wkt("<gml:Nonsense/>"), None);
        assert_eq!(gml_to_wkt("not xml at all"), None);
    }
}
