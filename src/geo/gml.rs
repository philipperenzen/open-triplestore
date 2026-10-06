//! `geo:gmlLiteral` — the GML 3.2 geometry profile this store reads and writes.
//!
//! GEOS (via the `geos` crate) exposes no GML reader, so a GML geometry is
//! translated into WKT, which the WKT path then parses with GEOS; `geof:asGML`
//! and constructive results on a GML operand go the other way
//! ([`geometry_to_gml`]). GeoSPARQL 1.1 Req 22 asks an implementation to say
//! which GML profile it supports. It is this one:
//!
//! | GML element | Read as |
//! |---|---|
//! | `Point` | `POINT` |
//! | `LineString`, a standalone `LinearRing` | `LINESTRING` |
//! | `Curve` of `LineStringSegment`s, `OrientableCurve`, `CompositeCurve` | `LINESTRING` (members joined end to start; a reversed `OrientableCurve` reversed) |
//! | `Polygon`, `PolygonPatch`, `Triangle`, `Rectangle` | `POLYGON`; rings are `LinearRing`s or `Ring`s of linear `curveMember`s |
//! | `Surface` | its one patch as a `POLYGON`, several patches as a `MULTIPOLYGON` |
//! | `PolyhedralSurface`, `Tin`, `TriangulatedSurface`, `CompositeSurface` | `MULTIPOLYGON` of their patches or members |
//! | `OrientableSurface` | its base surface |
//! | `MultiPoint`, `MultiCurve`/`MultiLineString`, `MultiSurface`/`MultiPolygon` | the `MULTI*` type (an empty one is `MULTI* EMPTY`) |
//! | `MultiGeometry` | `GEOMETRYCOLLECTION` |
//! | `Envelope` | the `POLYGON` between `lowerCorner` and `upperCorner` |
//!
//! Coordinates come from `gml:pos`, `gml:posList`, `gml:coordinates` (with its
//! `cs`, `ts` and `decimal` attributes), `gml:pointProperty`/`gml:pointRep` and
//! GML 2's `gml:coord`; text anywhere else (`gml:name`, `gml:description`) is not
//! a coordinate. `srsDimension="3"`, on the geometry or on a `pos`/`posList`, is
//! kept as Z; a `gml:pos` without one is 2D or 3D by its number count. Any other
//! dimension, a `posList` whose numbers do not divide into it, a number that does
//! not parse, and every element outside this profile — arcs and other curved
//! segments (`Arc`, `ArcString`, `Circle`, splines …), curved patches, `Solid`,
//! `MultiSolid`, `CompositeSolid` — make the literal not a geometry: functions
//! over it are unbound rather than computed on a straight-line approximation.
//!
//! The `srsName` (and so the CRS and axis order) is the caller's concern:
//! [`gml_srs_name`] reads it, [`super::datatypes::literal_crs_uri`] normalises it.

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

/// The GML 3.2 namespace, as [`geometry_to_gml`] declares it.
pub const GML_NAMESPACE: &str = "http://www.opengis.net/gml/3.2";

/// Deepest element nesting a geometry may have (a `MultiGeometry` inside a
/// `MultiGeometry` …) before the literal is refused rather than recursed into.
const MAX_DEPTH: usize = 32;

/// One position: x, y and an optional z, in the CRS's axis order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct P {
    pub x: f64,
    pub y: f64,
    pub z: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum MultiKind {
    Point,
    Line,
    Poly,
    Geom,
}

/// A parsed geometry, ready to serialise to WKT, GML or KML.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum G {
    /// `None` is the empty point.
    Point(Option<P>),
    /// No positions is the empty line string.
    Line(Vec<P>),
    /// Rings, exterior first; no rings is the empty polygon.
    Poly(Vec<Vec<P>>),
    Multi(MultiKind, Vec<G>),
}

impl G {
    /// Every position of the geometry, in document order.
    pub(super) fn positions(&self, out: &mut Vec<P>) {
        match self {
            G::Point(p) => out.extend(p.iter().copied()),
            G::Line(ps) => out.extend_from_slice(ps),
            G::Poly(rings) => rings.iter().for_each(|r| out.extend_from_slice(r)),
            G::Multi(_, items) => items.iter().for_each(|g| g.positions(out)),
        }
    }

    /// Whether every position has a Z (and there is at least one): only then
    /// is the geometry written in three dimensions. A geometry mixing 2D and 3D
    /// positions is written in 2D rather than with an invented Z.
    pub(super) fn has_z(&self) -> bool {
        let mut ps = Vec::new();
        self.positions(&mut ps);
        !ps.is_empty() && ps.iter().all(|p| p.z.is_some())
    }

    pub(super) fn is_empty(&self) -> bool {
        match self {
            G::Point(p) => p.is_none(),
            G::Line(ps) => ps.is_empty(),
            G::Poly(rings) => rings.is_empty(),
            G::Multi(_, items) => items.iter().all(G::is_empty),
        }
    }
}

// ─── XML events ─────────────────────────────────────────────────────────────

/// The attributes of an element that the readers use.
#[derive(Debug, Default)]
pub(super) struct Attrs {
    /// `srsDimension`; an unparseable value is `Some(0)`, which no geometry
    /// accepts.
    pub srs_dimension: Option<usize>,
    /// `gml:coordinates`'s coordinate separator, tuple separator and decimal mark.
    pub cs: Option<String>,
    pub ts: Option<String>,
    pub decimal: Option<String>,
    /// `orientation="-"` on an `OrientableCurve`/`OrientableSurface`.
    pub reversed: bool,
}

/// A flattened XML event. `End` carries no name: every `Start` (an empty
/// element included) has exactly one matching `End`, whose index is in
/// [`Doc::ends`].
#[derive(Debug)]
pub(super) enum Ev {
    Start(String, Attrs),
    End,
    Text(String),
}

/// A tokenised XML document, with the index of each element's `End`.
pub(super) struct Doc {
    pub ev: Vec<Ev>,
    ends: Vec<usize>,
}

impl Doc {
    /// Tokenise `xml`, or `None` when it is not well-formed.
    pub(super) fn parse(xml: &str) -> Option<Doc> {
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);
        let mut ev = Vec::new();
        let mut ends = Vec::new();
        let mut open = Vec::new();
        loop {
            match reader.read_event().ok()? {
                Event::Start(e) => {
                    open.push(ev.len());
                    ev.push(Ev::Start(local_name(&e), attrs(&e)));
                    ends.push(0);
                }
                Event::Empty(e) => {
                    let i = ev.len();
                    ev.push(Ev::Start(local_name(&e), attrs(&e)));
                    ends.push(i + 1);
                    ev.push(Ev::End);
                    ends.push(0);
                }
                Event::End(_) => {
                    let start = open.pop()?;
                    ends[start] = ev.len();
                    ev.push(Ev::End);
                    ends.push(0);
                }
                Event::Text(e) => {
                    let text = quick_xml::escape::unescape(&e).ok()?.trim().to_string();
                    if !text.is_empty() {
                        ev.push(Ev::Text(text));
                        ends.push(0);
                    }
                }
                Event::CData(e) => {
                    let text = e.trim().to_string();
                    if !text.is_empty() {
                        ev.push(Ev::Text(text));
                        ends.push(0);
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
        open.is_empty().then_some(Doc { ev, ends })
    }

    /// The local name and attributes of the element starting at `i`.
    pub(super) fn start(&self, i: usize) -> Option<(&str, &Attrs)> {
        match self.ev.get(i)? {
            Ev::Start(name, attrs) => Some((name.as_str(), attrs)),
            _ => None,
        }
    }

    /// The direct child elements of the element at `i`, as their start indices.
    pub(super) fn children(&self, i: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let end = self.ends[i];
        let mut j = i + 1;
        while j < end {
            match self.ev[j] {
                Ev::Start(..) => {
                    out.push(j);
                    j = self.ends[j] + 1;
                }
                _ => j += 1,
            }
        }
        out
    }

    /// The direct children of `i` named `name`.
    pub(super) fn children_named(&self, i: usize, names: &[&str]) -> Vec<usize> {
        self.children(i)
            .into_iter()
            .filter(|&c| self.start(c).is_some_and(|(n, _)| names.contains(&n)))
            .collect()
    }

    /// The text directly inside the element at `i`, joined with spaces.
    pub(super) fn text(&self, i: usize) -> String {
        let mut out = String::new();
        let end = self.ends[i];
        let mut j = i + 1;
        while j < end {
            match &self.ev[j] {
                Ev::Text(t) => {
                    if !out.is_empty() {
                        out.push(' ');
                    }
                    out.push_str(t);
                    j += 1;
                }
                Ev::Start(..) => j = self.ends[j] + 1,
                Ev::End => j += 1,
            }
        }
        out
    }

    /// The first element, in document order, whose local name passes `pred`.
    pub(super) fn first(&self, pred: impl Fn(&str) -> bool) -> Option<usize> {
        (0..self.ev.len()).find(|&i| self.start(i).is_some_and(|(n, _)| pred(n)))
    }
}

fn local_name(e: &BytesStart) -> String {
    e.local_name().as_ref().to_string()
}

fn attr_value(e: &BytesStart, name: &str) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        (a.key.local_name().as_ref() == name)
            .then(|| a.normalized_value(quick_xml::XmlVersion::Explicit1_0).ok())
            .flatten()
            .map(|v| v.to_string())
    })
}

fn attrs(e: &BytesStart) -> Attrs {
    Attrs {
        srs_dimension: attr_value(e, "srsDimension").map(|v| v.trim().parse().unwrap_or(0)),
        cs: attr_value(e, "cs"),
        ts: attr_value(e, "ts"),
        decimal: attr_value(e, "decimal"),
        reversed: attr_value(e, "orientation").is_some_and(|v| v.trim() == "-"),
    }
}

/// Parse numbers separated by whitespace; `None` if any is not a finite number.
pub(super) fn numbers(text: &str) -> Option<Vec<f64>> {
    text.split_whitespace()
        .map(|t| t.parse::<f64>().ok().filter(|v| v.is_finite()))
        .collect()
}

/// Group `nums` into positions of `dim` (2 or 3) numbers, exactly.
fn group(nums: &[f64], dim: usize) -> Option<Vec<P>> {
    if !(2..=3).contains(&dim) || !nums.len().is_multiple_of(dim) {
        return None;
    }
    Some(
        nums.chunks(dim)
            .map(|c| P {
                x: c[0],
                y: c[1],
                z: c.get(2).copied(),
            })
            .collect(),
    )
}

// ─── GML → geometry ─────────────────────────────────────────────────────────

/// Extract the `srsName` CRS URI from the outermost GML geometry element, if any
/// (e.g. `<gml:Point srsName="http://www.opengis.net/def/crs/EPSG/0/28992">`).
pub fn gml_srs_name(gml: &str) -> Option<String> {
    let mut reader = Reader::from_str(gml);
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                if let Some(v) = attr_value(&e, "srsName") {
                    return Some(v);
                }
                // Keep scanning: the srsName may sit on a nested geometry element.
            }
            Ok(Event::Eof) | Err(_) => return None,
            _ => {}
        }
    }
}

/// The local name of a GML literal's geometry element (`Surface`, `Envelope`,
/// …), when it reads as a geometry.
pub fn gml_root_name(gml: &str) -> Option<String> {
    let doc = Doc::parse(gml)?;
    let root = doc.first(is_geometry)?;
    Reader2 { doc: &doc }.geometry(root, None, 0)?;
    doc.start(root).map(|(name, _)| name.to_string())
}

/// Convert a GML geometry document to a WKT string, or `None` if it is not a
/// geometry of the supported profile (see the module docs).
pub fn gml_to_wkt(gml: &str) -> Option<String> {
    gml_to_geometry(gml).map(|g| to_wkt(&g))
}

/// Read the first GML geometry element of a document.
pub(super) fn gml_to_geometry(gml: &str) -> Option<G> {
    let doc = Doc::parse(gml)?;
    let root = doc.first(is_geometry)?;
    Reader2 { doc: &doc }.geometry(root, None, 0)
}

/// Element names that are GML geometries — read, or refused as outside the
/// profile. The first one in a document is the literal's geometry.
fn is_geometry(name: &str) -> bool {
    matches!(
        name,
        "Point"
            | "LineString"
            | "LinearRing"
            | "Curve"
            | "OrientableCurve"
            | "CompositeCurve"
            | "Ring"
            | "Polygon"
            | "Triangle"
            | "Rectangle"
            | "Surface"
            | "OrientableSurface"
            | "CompositeSurface"
            | "PolyhedralSurface"
            | "Tin"
            | "TriangulatedSurface"
            | "MultiPoint"
            | "MultiCurve"
            | "MultiLineString"
            | "MultiSurface"
            | "MultiPolygon"
            | "MultiGeometry"
            | "Envelope"
            | "Solid"
            | "CompositeSolid"
            | "MultiSolid"
            | "GeometricComplex"
    )
}

struct Reader2<'a> {
    doc: &'a Doc,
}

impl Reader2<'_> {
    /// The geometry element at `i`. `dim` is the `srsDimension` inherited from
    /// an enclosing element.
    fn geometry(&self, i: usize, dim: Option<usize>, depth: usize) -> Option<G> {
        if depth > MAX_DEPTH {
            return None;
        }
        let (name, attrs) = self.doc.start(i)?;
        let dim = attrs.srs_dimension.or(dim);
        let d = depth + 1;
        match name {
            "Point" => match self.positions(i, dim)?.as_slice() {
                [p] => Some(G::Point(Some(*p))),
                _ => None,
            },
            "LineString" | "LinearRing" => self.line(i, dim).map(G::Line),
            "Curve" | "OrientableCurve" | "CompositeCurve" | "Ring" => {
                self.curve(i, dim, d).map(G::Line)
            }
            "Polygon" | "Triangle" | "Rectangle" => self.polygon(i, dim, d).map(G::Poly),
            "Surface" => {
                let mut polys = self.surface(i, dim, d)?;
                match polys.len() {
                    0 => None,
                    1 => polys.pop().map(G::Poly),
                    _ => Some(multi_poly(polys)),
                }
            }
            "PolyhedralSurface"
            | "Tin"
            | "TriangulatedSurface"
            | "CompositeSurface"
            | "OrientableSurface" => {
                let polys = self.surface(i, dim, d)?;
                (!polys.is_empty()).then(|| multi_poly(polys))
            }
            "MultiPoint" => {
                let items = self.members(i, &["pointMember", "pointMembers"], dim, d)?;
                items
                    .iter()
                    .all(|g| matches!(g, G::Point(_)))
                    .then_some(G::Multi(MultiKind::Point, items))
            }
            "MultiCurve" | "MultiLineString" => {
                let items = self.members(
                    i,
                    &["curveMember", "curveMembers", "lineStringMember"],
                    dim,
                    d,
                )?;
                let mut lines = Vec::new();
                for g in items {
                    match g {
                        G::Line(_) => lines.push(g),
                        G::Multi(MultiKind::Line, ls) => lines.extend(ls),
                        _ => return None,
                    }
                }
                Some(G::Multi(MultiKind::Line, lines))
            }
            "MultiSurface" | "MultiPolygon" => {
                let items = self.members(
                    i,
                    &["surfaceMember", "surfaceMembers", "polygonMember"],
                    dim,
                    d,
                )?;
                let mut polys = Vec::new();
                for g in items {
                    match g {
                        G::Poly(_) => polys.push(g),
                        G::Multi(MultiKind::Poly, ps) => polys.extend(ps),
                        _ => return None,
                    }
                }
                Some(G::Multi(MultiKind::Poly, polys))
            }
            "MultiGeometry" => {
                let items = self.members(i, &["geometryMember", "geometryMembers"], dim, d)?;
                Some(G::Multi(MultiKind::Geom, items))
            }
            "Envelope" => self.envelope(i, dim),
            // Solids, curved geometry and anything else: outside the profile.
            _ => None,
        }
    }

    /// The positions directly inside a geometry element, in document order.
    fn positions(&self, i: usize, dim: Option<usize>) -> Option<Vec<P>> {
        let mut out = Vec::new();
        for c in self.doc.children(i) {
            let (name, attrs) = self.doc.start(c)?;
            let dim = attrs.srs_dimension.or(dim);
            match name {
                "pos" | "lowerCorner" | "upperCorner" => {
                    let nums = numbers(&self.doc.text(c))?;
                    let dim = dim.unwrap_or(nums.len());
                    match group(&nums, dim)?.as_slice() {
                        [p] => out.push(*p),
                        _ => return None,
                    }
                }
                "posList" => out.extend(group(&numbers(&self.doc.text(c))?, dim.unwrap_or(2))?),
                "coordinates" => out.extend(coordinates(&self.doc.text(c), attrs)?),
                "coord" => out.push(self.coord(c)?),
                "pointProperty" | "pointRep" => {
                    let point = self.doc.children_named(c, &["Point"]);
                    match self.geometry(*point.first()?, dim, MAX_DEPTH)? {
                        G::Point(Some(p)) => out.push(p),
                        _ => return None,
                    }
                }
                _ => {}
            }
        }
        Some(out)
    }

    /// GML 2's `<gml:coord><gml:X>…</gml:X><gml:Y>…</gml:Y>[<gml:Z>…</gml:Z>]</gml:coord>`.
    fn coord(&self, i: usize) -> Option<P> {
        let value = |name: &str| -> Option<Option<f64>> {
            match self.doc.children_named(i, &[name]).first() {
                None => Some(None),
                Some(&c) => match numbers(&self.doc.text(c))?.as_slice() {
                    [v] => Some(Some(*v)),
                    _ => None,
                },
            }
        };
        Some(P {
            x: value("X")??,
            y: value("Y")??,
            z: value("Z")?,
        })
    }

    /// A line string's positions: two or more.
    fn line(&self, i: usize, dim: Option<usize>) -> Option<Vec<P>> {
        let ps = self.positions(i, dim)?;
        (ps.len() >= 2).then_some(ps)
    }

    /// A curve's positions: a `LineString`/`LinearRing`, a `Curve` of linear
    /// segments, an `OrientableCurve` (reversed when its orientation is `-`), or
    /// a `CompositeCurve`/`Ring` of curve members joined end to start.
    fn curve(&self, i: usize, dim: Option<usize>, depth: usize) -> Option<Vec<P>> {
        if depth > MAX_DEPTH {
            return None;
        }
        let (name, attrs) = self.doc.start(i)?;
        let dim = attrs.srs_dimension.or(dim);
        let d = depth + 1;
        match name {
            "LineString" | "LinearRing" => self.line(i, dim),
            "Curve" => {
                let mut parts = Vec::new();
                for segments in self.doc.children_named(i, &["segments"]) {
                    for s in self.doc.children(segments) {
                        let (segment, sattrs) = self.doc.start(s)?;
                        // Only straight segments: an arc read as straight lines
                        // between its control points would be a different shape.
                        if segment != "LineStringSegment" {
                            return None;
                        }
                        parts.push(self.line(s, sattrs.srs_dimension.or(dim))?);
                    }
                }
                join(parts)
            }
            "OrientableCurve" => {
                let base = self.doc.children_named(i, &["baseCurve"]);
                let curve = *self.doc.children(*base.first()?).first()?;
                let mut ps = self.curve(curve, dim, d)?;
                if attrs.reversed {
                    ps.reverse();
                }
                Some(ps)
            }
            "CompositeCurve" | "Ring" => {
                let mut parts = Vec::new();
                for m in self.doc.children_named(i, &["curveMember"]) {
                    parts.push(self.curve(*self.doc.children(m).first()?, dim, d)?);
                }
                join(parts)
            }
            _ => None,
        }
    }

    /// A polygon's rings — `exterior` (GML 2 `outerBoundaryIs`) first, then the
    /// `interior`s (`innerBoundaryIs`) — each a `LinearRing` or a `Ring`, closed
    /// with four or more positions. No exterior is the empty polygon.
    fn polygon(&self, i: usize, dim: Option<usize>, depth: usize) -> Option<Vec<Vec<P>>> {
        let ring = |b: usize| -> Option<Vec<P>> {
            let r = *self.doc.children(b).first()?;
            let ps = self.curve(r, dim, depth)?;
            (ps.len() >= 4 && ps.first() == ps.last()).then_some(ps)
        };
        let exterior = self.doc.children_named(i, &["exterior", "outerBoundaryIs"]);
        let interior = self.doc.children_named(i, &["interior", "innerBoundaryIs"]);
        let Some(&e) = exterior.first() else {
            return interior.is_empty().then(Vec::new);
        };
        if exterior.len() > 1 {
            return None;
        }
        let mut rings = vec![ring(e)?];
        for b in interior {
            rings.push(ring(b)?);
        }
        Some(rings)
    }

    /// The polygons of a surface: its planar patches (`PolygonPatch`, `Triangle`,
    /// `Rectangle`), or for a `CompositeSurface` its members, or for an
    /// `OrientableSurface` its base surface.
    fn surface(&self, i: usize, dim: Option<usize>, depth: usize) -> Option<Vec<Vec<Vec<P>>>> {
        if depth > MAX_DEPTH {
            return None;
        }
        let (name, attrs) = self.doc.start(i)?;
        let dim = attrs.srs_dimension.or(dim);
        let d = depth + 1;
        let mut polys = Vec::new();
        match name {
            "OrientableSurface" => {
                let base = self.doc.children_named(i, &["baseSurface"]);
                return self.surface(*self.doc.children(*base.first()?).first()?, dim, d);
            }
            "Polygon" | "Triangle" | "Rectangle" | "PolygonPatch" => {
                polys.push(self.polygon(i, dim, d)?);
            }
            "CompositeSurface" => {
                for m in self.doc.children_named(i, &["surfaceMember"]) {
                    polys.extend(self.surface(*self.doc.children(m).first()?, dim, d)?);
                }
            }
            "Surface" | "PolyhedralSurface" | "Tin" | "TriangulatedSurface" => {
                let patches = self
                    .doc
                    .children_named(i, &["patches", "polygonPatches", "trianglePatches"]);
                for group in patches {
                    for p in self.doc.children(group) {
                        let (patch, _) = self.doc.start(p)?;
                        // Cones, cylinders, spheres and spline patches are curved.
                        if !matches!(patch, "PolygonPatch" | "Triangle" | "Rectangle") {
                            return None;
                        }
                        polys.push(self.polygon(p, dim, d)?);
                    }
                }
            }
            _ => return None,
        }
        Some(polys)
    }

    /// The geometries inside the member properties of a `Multi*`: each member
    /// element holds one geometry, a `…Members` element any number. A member
    /// that does not read (an arc, an unresolved `xlink:href`) fails the whole
    /// collection.
    fn members(
        &self,
        i: usize,
        names: &[&str],
        dim: Option<usize>,
        depth: usize,
    ) -> Option<Vec<G>> {
        let mut items = Vec::new();
        for m in self.doc.children_named(i, names) {
            let inner = self.doc.children(m);
            let (member, _) = self.doc.start(m)?;
            if inner.is_empty() || (!member.ends_with('s') && inner.len() > 1) {
                return None;
            }
            for g in inner {
                items.push(self.geometry(g, dim, depth)?);
            }
        }
        Some(items)
    }

    /// An `Envelope`'s rectangle, from its `lowerCorner` and `upperCorner`
    /// (or GML 2's two `coord`/`pos`/`coordinates` positions). A degenerate
    /// envelope is the point or line it is.
    fn envelope(&self, i: usize, dim: Option<usize>) -> Option<G> {
        let [lo, hi] = <[P; 2]>::try_from(self.positions(i, dim)?).ok()?;
        let (x0, x1) = (lo.x.min(hi.x), lo.x.max(hi.x));
        let (y0, y1) = (lo.y.min(hi.y), lo.y.max(hi.y));
        let p = |x, y| P { x, y, z: None };
        Some(match (x0 == x1, y0 == y1) {
            (true, true) => G::Point(Some(p(x0, y0))),
            (true, false) | (false, true) => G::Line(vec![p(x0, y0), p(x1, y1)]),
            (false, false) => G::Poly(vec![vec![
                p(x0, y0),
                p(x1, y0),
                p(x1, y1),
                p(x0, y1),
                p(x0, y0),
            ]]),
        })
    }
}

fn multi_poly(polys: Vec<Vec<Vec<P>>>) -> G {
    G::Multi(MultiKind::Poly, polys.into_iter().map(G::Poly).collect())
}

/// Join curve parts end to start into one sequence of positions: each part
/// must begin where the previous one ended (that shared position is kept once).
fn join(parts: Vec<Vec<P>>) -> Option<Vec<P>> {
    let mut out: Vec<P> = Vec::new();
    for part in parts {
        match out.last() {
            None => out = part,
            Some(last) if part.first() == Some(last) => out.extend_from_slice(&part[1..]),
            Some(_) => return None,
        }
    }
    (out.len() >= 2).then_some(out)
}

/// `gml:coordinates`: tuples separated by `ts` (whitespace by default), the
/// ordinates of a tuple by `cs` (`,` by default), with `decimal` (`.` by
/// default) as the decimal mark.
fn coordinates(text: &str, attrs: &Attrs) -> Option<Vec<P>> {
    let cs = attrs.cs.as_deref().unwrap_or(",");
    let decimal = attrs.decimal.as_deref().unwrap_or(".");
    let tuples: Vec<&str> = match attrs.ts.as_deref() {
        None => text.split_whitespace().collect(),
        Some(ts) if ts.trim().is_empty() => text.split_whitespace().collect(),
        Some(ts) => text
            .split(ts)
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .collect(),
    };
    if cs.is_empty() || decimal.is_empty() || cs == decimal {
        return None;
    }
    tuples
        .into_iter()
        .map(|t| {
            let nums = t
                .split(cs)
                .map(|n| {
                    let n = n.trim();
                    let n = if decimal == "." {
                        n.to_string()
                    } else {
                        n.replace(decimal, ".")
                    };
                    n.parse::<f64>().ok().filter(|v| v.is_finite())
                })
                .collect::<Option<Vec<_>>>()?;
            match group(&nums, nums.len())?.as_slice() {
                [p] => Some(*p),
                _ => None,
            }
        })
        .collect()
}

// ─── geometry → WKT ─────────────────────────────────────────────────────────

fn num(v: f64) -> String {
    // `{}` is the shortest round-tripping form and drops a trailing `.0`.
    format!("{v}")
}

fn wkt_pos(p: &P, z: bool) -> String {
    match (z, p.z) {
        (true, Some(pz)) => format!("{} {} {}", num(p.x), num(p.y), num(pz)),
        _ => format!("{} {}", num(p.x), num(p.y)),
    }
}

fn wkt_seq(ps: &[P], z: bool) -> String {
    let parts: Vec<String> = ps.iter().map(|p| wkt_pos(p, z)).collect();
    format!("({})", parts.join(", "))
}

fn wkt_rings(rings: &[Vec<P>], z: bool) -> String {
    let parts: Vec<String> = rings.iter().map(|r| wkt_seq(r, z)).collect();
    format!("({})", parts.join(", "))
}

/// The WKT of a parsed geometry: `Z` when every position has one.
pub(super) fn to_wkt(g: &G) -> String {
    wkt_of(g, g.has_z())
}

fn wkt_of(g: &G, z: bool) -> String {
    let tag = |t: &str| if z { format!("{t} Z ") } else { t.to_string() };
    match g {
        G::Point(None) => "POINT EMPTY".into(),
        G::Point(Some(p)) => format!("{}({})", tag("POINT"), wkt_pos(p, z)),
        G::Line(ps) if ps.is_empty() => "LINESTRING EMPTY".into(),
        G::Line(ps) => format!("{}{}", tag("LINESTRING"), wkt_seq(ps, z)),
        G::Poly(rings) if rings.is_empty() => "POLYGON EMPTY".into(),
        G::Poly(rings) => format!("{}{}", tag("POLYGON"), wkt_rings(rings, z)),
        G::Multi(kind, items) => {
            let name = match kind {
                MultiKind::Point => "MULTIPOINT",
                MultiKind::Line => "MULTILINESTRING",
                MultiKind::Poly => "MULTIPOLYGON",
                MultiKind::Geom => "GEOMETRYCOLLECTION",
            };
            // An empty member has no place in a Multi* body (and GEOS 3.11
            // reads none): it is left out.
            let parts: Vec<String> = items
                .iter()
                .filter(|g| *kind == MultiKind::Geom || !g.is_empty())
                .map(|g| match (kind, g) {
                    (MultiKind::Point, G::Point(Some(p))) => format!("({})", wkt_pos(p, z)),
                    (MultiKind::Line, G::Line(ps)) => wkt_seq(ps, z),
                    (MultiKind::Poly, G::Poly(rings)) => wkt_rings(rings, z),
                    _ => wkt_of(g, z),
                })
                .collect();
            if parts.is_empty() {
                format!("{name} EMPTY")
            } else {
                format!("{}({})", tag(name), parts.join(", "))
            }
        }
    }
}

// ─── GEOS → geometry ────────────────────────────────────────────────────────

/// A GEOS geometry as a [`G`], every position mapped through `xy` (a
/// reprojection, or the identity). `None` when a position does not map or the
/// geometry is of a type the profile cannot express (a curve).
pub(super) fn from_geos(
    geom: &impl geos::Geom,
    xy: &dyn Fn(f64, f64) -> Option<(f64, f64)>,
) -> Option<G> {
    use geos::GeometryTypes as T;
    let kind = geom.geometry_type().ok()?;
    let members = |kind: MultiKind| -> Option<G> {
        let n = geom.get_num_geometries().ok()?;
        let items = (0..n)
            .map(|i| from_geos(&geom.get_geometry_n(i).ok()?, xy))
            .collect::<Option<Vec<_>>>()?;
        Some(G::Multi(kind, items))
    };
    Some(match kind {
        T::Point => {
            if geom.is_empty().ok()? {
                G::Point(None)
            } else {
                G::Point(sequence(geom, xy)?.into_iter().next())
            }
        }
        T::LineString | T::LinearRing => G::Line(sequence(geom, xy)?),
        T::Polygon => {
            if geom.is_empty().ok()? {
                G::Poly(Vec::new())
            } else {
                let mut rings = vec![sequence(&geom.get_exterior_ring().ok()?, xy)?];
                for i in 0..geom.get_num_interior_rings().ok()? {
                    rings.push(sequence(&geom.get_interior_ring_n(i).ok()?, xy)?);
                }
                G::Poly(rings)
            }
        }
        T::MultiPoint => members(MultiKind::Point)?,
        T::MultiLineString => members(MultiKind::Line)?,
        T::MultiPolygon => members(MultiKind::Poly)?,
        T::GeometryCollection => members(MultiKind::Geom)?,
        #[allow(unreachable_patterns)] // curve types, with GEOS >= 3.13 features
        _ => return None,
    })
}

/// The positions of a point, line or ring, Z kept.
fn sequence(geom: &impl geos::Geom, xy: &dyn Fn(f64, f64) -> Option<(f64, f64)>) -> Option<Vec<P>> {
    if geom.is_empty().ok()? {
        return Some(Vec::new());
    }
    let cs = geom.get_coord_seq().ok()?;
    let z = geom.has_z().ok()?;
    (0..cs.size().ok()?)
        .map(|i| {
            let (x, y) = xy(cs.get_x(i).ok()?, cs.get_y(i).ok()?)?;
            let z = if z {
                cs.get_z(i).ok().filter(|v| v.is_finite())
            } else {
                None
            };
            Some(P { x, y, z })
        })
        .collect()
}

// ─── geometry → GML ─────────────────────────────────────────────────────────

/// Escape a value for an XML attribute.
pub(super) fn xml_attr(v: &str) -> String {
    v.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// A GEOS geometry as a GML 3.2 document of this profile, with `srs_name` as
/// its `srsName` and `srsDimension="3"` when it has Z. An empty geometry is an
/// empty `Multi*` element — it keeps its `srsName`, which an empty literal could
/// not. `None` for a geometry the profile cannot express.
pub fn geometry_to_gml(geom: &impl geos::Geom, srs_name: Option<&str>) -> Option<String> {
    let identity = |x, y| Some((x, y));
    Some(write_gml(&from_geos(geom, &identity)?, srs_name))
}

pub(super) fn write_gml(g: &G, srs_name: Option<&str>) -> String {
    let z = g.has_z();
    let mut root = format!(" xmlns:gml=\"{GML_NAMESPACE}\"");
    if let Some(srs) = srs_name {
        root.push_str(&format!(" srsName=\"{}\"", xml_attr(srs)));
    }
    if z {
        root.push_str(" srsDimension=\"3\"");
    }
    if g.is_empty() {
        let name = match g {
            G::Point(_) | G::Multi(MultiKind::Point, _) => "MultiPoint",
            G::Line(_) | G::Multi(MultiKind::Line, _) => "MultiCurve",
            G::Poly(_) | G::Multi(MultiKind::Poly, _) => "MultiSurface",
            G::Multi(MultiKind::Geom, _) => "MultiGeometry",
        };
        return format!("<gml:{name}{root}/>");
    }
    let mut out = String::new();
    gml_element(g, z, &root, &mut out);
    out
}

fn pos_list(ps: &[P], z: bool) -> String {
    ps.iter()
        .map(|p| wkt_pos(p, z))
        .collect::<Vec<_>>()
        .join(" ")
}

fn gml_element(g: &G, z: bool, attrs: &str, out: &mut String) {
    match g {
        G::Point(Some(p)) => out.push_str(&format!(
            "<gml:Point{attrs}><gml:pos>{}</gml:pos></gml:Point>",
            wkt_pos(p, z)
        )),
        G::Line(ps) => out.push_str(&format!(
            "<gml:LineString{attrs}><gml:posList>{}</gml:posList></gml:LineString>",
            pos_list(ps, z)
        )),
        G::Poly(rings) => {
            out.push_str(&format!("<gml:Polygon{attrs}>"));
            for (i, r) in rings.iter().enumerate() {
                let b = if i == 0 { "exterior" } else { "interior" };
                out.push_str(&format!(
                    "<gml:{b}><gml:LinearRing><gml:posList>{}</gml:posList></gml:LinearRing></gml:{b}>",
                    pos_list(r, z)
                ));
            }
            out.push_str("</gml:Polygon>");
        }
        G::Multi(kind, items) => {
            let (name, member) = match kind {
                MultiKind::Point => ("MultiPoint", "pointMember"),
                MultiKind::Line => ("MultiCurve", "curveMember"),
                MultiKind::Poly => ("MultiSurface", "surfaceMember"),
                MultiKind::Geom => ("MultiGeometry", "geometryMember"),
            };
            out.push_str(&format!("<gml:{name}{attrs}>"));
            for item in items.iter().filter(|g| !g.is_empty()) {
                out.push_str(&format!("<gml:{member}>"));
                gml_element(item, z, "", out);
                out.push_str(&format!("</gml:{member}>"));
            }
            out.push_str(&format!("</gml:{name}>"));
        }
        // Empty members are skipped above, and an empty root never gets here.
        G::Point(None) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wkt(g: &str) -> Option<String> {
        gml_to_wkt(g)
    }

    #[test]
    fn point() {
        let g = "<gml:Point srsName=\"EPSG:28992\"><gml:pos>187330 428345</gml:pos></gml:Point>";
        assert_eq!(wkt(g).as_deref(), Some("POINT(187330 428345)"));
    }

    #[test]
    fn linestring_poslist() {
        let g = "<gml:LineString><gml:posList>0 0 1 1 2 0</gml:posList></gml:LineString>";
        assert_eq!(wkt(g).as_deref(), Some("LINESTRING(0 0, 1 1, 2 0)"));
    }

    #[test]
    fn polygon_with_hole() {
        let g = "<gml:Polygon><gml:exterior><gml:LinearRing><gml:posList>0 0 10 0 10 10 0 10 0 0</gml:posList></gml:LinearRing></gml:exterior><gml:interior><gml:LinearRing><gml:posList>3 3 4 3 4 4 3 4 3 3</gml:posList></gml:LinearRing></gml:interior></gml:Polygon>";
        assert_eq!(
            wkt(g).as_deref(),
            Some("POLYGON((0 0, 10 0, 10 10, 0 10, 0 0), (3 3, 4 3, 4 4, 3 4, 3 3))")
        );
    }

    #[test]
    fn multipoint() {
        let g = "<gml:MultiPoint><gml:pointMember><gml:Point><gml:pos>1 2</gml:pos></gml:Point></gml:pointMember><gml:pointMember><gml:Point><gml:pos>3 4</gml:pos></gml:Point></gml:pointMember></gml:MultiPoint>";
        assert_eq!(wkt(g).as_deref(), Some("MULTIPOINT((1 2), (3 4))"));
        let g = "<gml:MultiPoint><gml:pointMembers><gml:Point><gml:pos>1 2</gml:pos></gml:Point><gml:Point><gml:pos>3 4</gml:pos></gml:Point></gml:pointMembers></gml:MultiPoint>";
        assert_eq!(wkt(g).as_deref(), Some("MULTIPOINT((1 2), (3 4))"));
    }

    #[test]
    fn srs_dimension_3_keeps_z() {
        // srsDimension on the geometry element…
        let g = "<gml:LineString srsDimension=\"3\"><gml:posList>0 0 10 1 1 10</gml:posList></gml:LineString>";
        assert_eq!(wkt(g).as_deref(), Some("LINESTRING Z (0 0 10, 1 1 10)"));
        // …or on the posList itself.
        let g = "<gml:LineString><gml:posList srsDimension=\"3\">0 0 10 1 1 10</gml:posList></gml:LineString>";
        assert_eq!(wkt(g).as_deref(), Some("LINESTRING Z (0 0 10, 1 1 10)"));
        // A gml:pos without one counts its numbers.
        let g = "<gml:Point><gml:pos>4 52 12.5</gml:pos></gml:Point>";
        assert_eq!(wkt(g).as_deref(), Some("POINT Z (4 52 12.5)"));
    }

    #[test]
    fn unsupported_or_inconsistent_dimensions_are_none() {
        for g in [
            "<gml:LineString srsDimension=\"4\"><gml:posList>0 0 1 1 2 2 3 3</gml:posList></gml:LineString>",
            "<gml:LineString srsDimension=\"x\"><gml:posList>0 0 1 1</gml:posList></gml:LineString>",
            // Five numbers do not divide into 2D positions.
            "<gml:LineString><gml:posList>0 0 1 1 2</gml:posList></gml:LineString>",
            "<gml:Point><gml:pos>1</gml:pos></gml:Point>",
            "<gml:Point><gml:pos>1 two</gml:pos></gml:Point>",
        ] {
            assert_eq!(wkt(g), None, "{g}");
        }
    }

    #[test]
    fn text_outside_coordinates_is_not_a_coordinate() {
        let g = "<gml:Point><gml:name>Pier 7</gml:name><gml:pos>1 2</gml:pos></gml:Point>";
        assert_eq!(wkt(g).as_deref(), Some("POINT(1 2)"));
    }

    #[test]
    fn envelope_is_its_rectangle() {
        let g = "<gml:Envelope srsName='EPSG:28992'><gml:lowerCorner>0 0</gml:lowerCorner><gml:upperCorner>2 3</gml:upperCorner></gml:Envelope>";
        assert_eq!(
            wkt(g).as_deref(),
            Some("POLYGON((0 0, 2 0, 2 3, 0 3, 0 0))")
        );
        let g = "<gml:Envelope><gml:lowerCorner>1 1</gml:lowerCorner><gml:upperCorner>1 1</gml:upperCorner></gml:Envelope>";
        assert_eq!(wkt(g).as_deref(), Some("POINT(1 1)"));
    }

    #[test]
    fn standalone_linear_ring_is_a_closed_line() {
        let g = "<gml:LinearRing><gml:posList>0 0 1 0 1 1 0 0</gml:posList></gml:LinearRing>";
        assert_eq!(wkt(g).as_deref(), Some("LINESTRING(0 0, 1 0, 1 1, 0 0)"));
    }

    #[test]
    fn surface_patches_are_one_polygon_or_a_multipolygon() {
        let patch = |c: &str| {
            format!("<gml:PolygonPatch><gml:exterior><gml:LinearRing><gml:posList>{c}</gml:posList></gml:LinearRing></gml:exterior></gml:PolygonPatch>")
        };
        let one = format!(
            "<gml:Surface><gml:patches>{}</gml:patches></gml:Surface>",
            patch("0 0 1 0 1 1 0 0")
        );
        assert_eq!(wkt(&one).as_deref(), Some("POLYGON((0 0, 1 0, 1 1, 0 0))"));
        let two = format!(
            "<gml:Surface><gml:patches>{}{}</gml:patches></gml:Surface>",
            patch("0 0 1 0 1 1 0 0"),
            patch("5 5 6 5 6 6 5 5")
        );
        assert_eq!(
            wkt(&two).as_deref(),
            Some("MULTIPOLYGON(((0 0, 1 0, 1 1, 0 0)), ((5 5, 6 5, 6 6, 5 5)))")
        );
    }

    #[test]
    fn ring_of_linear_curve_members() {
        let g = "<gml:Polygon><gml:exterior><gml:Ring>\
            <gml:curveMember><gml:LineString><gml:posList>0 0 4 0 4 4</gml:posList></gml:LineString></gml:curveMember>\
            <gml:curveMember><gml:Curve><gml:segments><gml:LineStringSegment><gml:posList>4 4 0 4 0 0</gml:posList></gml:LineStringSegment></gml:segments></gml:Curve></gml:curveMember>\
            </gml:Ring></gml:exterior></gml:Polygon>";
        assert_eq!(
            wkt(g).as_deref(),
            Some("POLYGON((0 0, 4 0, 4 4, 0 4, 0 0))")
        );
        // Members that do not meet end to start are not a ring.
        let gap = g.replace("4 4 0 4 0 0", "5 5 0 4 0 0");
        assert_eq!(wkt(&gap), None);
    }

    #[test]
    fn composites_triangles_tins_and_polyhedral_surfaces() {
        let tri = "<gml:Triangle><gml:exterior><gml:LinearRing><gml:posList>0 0 1 0 0 1 0 0</gml:posList></gml:LinearRing></gml:exterior></gml:Triangle>";
        assert_eq!(wkt(tri).as_deref(), Some("POLYGON((0 0, 1 0, 0 1, 0 0))"));
        let tin = format!(
            "<gml:Tin><gml:patches>{tri}{}</gml:patches></gml:Tin>",
            tri.replace("0 0 1 0 0 1 0 0", "1 0 1 1 0 1 1 0")
        );
        assert_eq!(
            wkt(&tin).as_deref(),
            Some("MULTIPOLYGON(((0 0, 1 0, 0 1, 0 0)), ((1 0, 1 1, 0 1, 1 0)))")
        );
        let poly = "<gml:Polygon><gml:exterior><gml:LinearRing><gml:posList>0 0 1 0 1 1 0 0</gml:posList></gml:LinearRing></gml:exterior></gml:Polygon>";
        let phs = format!("<gml:PolyhedralSurface><gml:polygonPatches>{}</gml:polygonPatches></gml:PolyhedralSurface>", poly.replace("gml:Polygon", "gml:PolygonPatch"));
        assert_eq!(
            wkt(&phs).as_deref(),
            Some("MULTIPOLYGON(((0 0, 1 0, 1 1, 0 0)))")
        );
        let cs = format!("<gml:CompositeSurface><gml:surfaceMember>{poly}</gml:surfaceMember></gml:CompositeSurface>");
        assert_eq!(
            wkt(&cs).as_deref(),
            Some("MULTIPOLYGON(((0 0, 1 0, 1 1, 0 0)))")
        );
        let cc = "<gml:CompositeCurve>\
            <gml:curveMember><gml:LineString><gml:posList>0 0 1 0</gml:posList></gml:LineString></gml:curveMember>\
            <gml:curveMember><gml:LineString><gml:posList>1 0 1 1</gml:posList></gml:LineString></gml:curveMember>\
            </gml:CompositeCurve>";
        assert_eq!(wkt(cc).as_deref(), Some("LINESTRING(0 0, 1 0, 1 1)"));
        let reversed = "<gml:OrientableCurve orientation='-'><gml:baseCurve><gml:LineString><gml:posList>0 0 1 0</gml:posList></gml:LineString></gml:baseCurve></gml:OrientableCurve>";
        assert_eq!(wkt(reversed).as_deref(), Some("LINESTRING(1 0, 0 0)"));
    }

    #[test]
    fn coordinates_with_cs_ts_and_decimal() {
        let g = "<gml:LineString><gml:coordinates>0,0 1.5,2</gml:coordinates></gml:LineString>";
        assert_eq!(wkt(g).as_deref(), Some("LINESTRING(0 0, 1.5 2)"));
        let g = "<gml:LineString><gml:coordinates cs=' ' ts=';' decimal=','>0 0;1,5 2</gml:coordinates></gml:LineString>";
        assert_eq!(wkt(g).as_deref(), Some("LINESTRING(0 0, 1.5 2)"));
        let g = "<gml:Point><gml:coord><gml:X>1</gml:X><gml:Y>2</gml:Y><gml:Z>3</gml:Z></gml:coord></gml:Point>";
        assert_eq!(wkt(g).as_deref(), Some("POINT Z (1 2 3)"));
    }

    #[test]
    fn arcs_solids_and_curved_patches_are_outside_the_profile() {
        for g in [
            "<gml:Curve><gml:segments><gml:Arc><gml:posList>0 0 1 1 2 0</gml:posList></gml:Arc></gml:segments></gml:Curve>",
            "<gml:Curve><gml:segments><gml:LineStringSegment><gml:posList>0 0 1 1</gml:posList></gml:LineStringSegment><gml:ArcString><gml:posList>1 1 2 2 3 1</gml:posList></gml:ArcString></gml:segments></gml:Curve>",
            "<gml:Solid><gml:exterior><gml:Shell/></gml:exterior></gml:Solid>",
            "<gml:Surface><gml:patches><gml:Sphere/></gml:patches></gml:Surface>",
            "<gml:MultiCurve><gml:curveMember><gml:Curve><gml:segments><gml:Circle><gml:posList>0 0 1 1 2 0</gml:posList></gml:Circle></gml:segments></gml:Curve></gml:curveMember></gml:MultiCurve>",
            "<gml:Point><gml:pos>1 2</gml:pos>",
        ] {
            assert_eq!(wkt(g), None, "{g}");
        }
    }

    #[test]
    fn empty_multi_geometries_read_as_empty() {
        assert_eq!(
            wkt("<gml:MultiSurface srsName='EPSG:28992'/>").as_deref(),
            Some("MULTIPOLYGON EMPTY")
        );
        assert_eq!(
            wkt("<gml:MultiGeometry></gml:MultiGeometry>").as_deref(),
            Some("GEOMETRYCOLLECTION EMPTY")
        );
    }

    #[test]
    fn deep_nesting_is_refused_not_recursed_into() {
        let deep = format!(
            "{}<gml:Point><gml:pos>0 0</gml:pos></gml:Point>{}",
            "<gml:MultiGeometry><gml:geometryMember>".repeat(100),
            "</gml:geometryMember></gml:MultiGeometry>".repeat(100)
        );
        assert_eq!(wkt(&deep), None);
    }

    #[test]
    fn written_gml_reads_back() {
        for w in [
            "POINT (1.5 -2)",
            "POINT Z (1 2 3)",
            "LINESTRING (0 0, 1 1, 2 0)",
            "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (1 1, 2 1, 2 2, 1 1))",
            "MULTIPOINT ((1 2), (3 4))",
            "MULTILINESTRING Z ((0 0 1, 1 1 2), (2 2 3, 3 3 4))",
            "MULTIPOLYGON (((0 0, 1 0, 1 1, 0 0)), ((2 2, 3 2, 3 3, 2 2)))",
            "GEOMETRYCOLLECTION (POINT (0 0), LINESTRING (1 1, 2 2))",
        ] {
            let g = geos::Geometry::new_from_wkt(w).unwrap();
            let gml =
                geometry_to_gml(&g, Some("http://www.opengis.net/def/crs/EPSG/0/28992")).unwrap();
            assert_eq!(
                gml_srs_name(&gml).as_deref(),
                Some("http://www.opengis.net/def/crs/EPSG/0/28992")
            );
            let back = geos::Geometry::new_from_wkt(&gml_to_wkt(&gml).unwrap()).unwrap();
            use geos::Geom;
            assert!(
                g.equals_exact(&back, 0.0).unwrap(),
                "{w} -> {gml} -> {:?}",
                back.to_wkt()
            );
        }
        let empty = geos::Geometry::new_from_wkt("POLYGON EMPTY").unwrap();
        let gml = geometry_to_gml(&empty, Some("EPSG:28992")).unwrap();
        assert_eq!(
            gml,
            "<gml:MultiSurface xmlns:gml=\"http://www.opengis.net/gml/3.2\" srsName=\"EPSG:28992\"/>"
        );
        assert_eq!(gml_to_wkt(&gml).as_deref(), Some("MULTIPOLYGON EMPTY"));
    }
}
