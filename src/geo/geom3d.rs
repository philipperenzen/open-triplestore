//! 3D geometry type system and WKT-Z parsing — the additive volumetric layer
//! beside the GEOS-backed 2D/2.5D path (spec §3.2–3.3).
//!
//! GeoSPARQL 1.1 + GEOS reason in the plane (Z is carried but not topologically
//! reasoned). 3D BAG LoD2.2 buildings are *solids* — closed polyhedral volumes.
//! This module models them on ISO 19107 / CityGML and parses the ISO-13249 WKT
//! `Z` forms (`POINT Z`, `POLYHEDRALSURFACE Z`, `TIN Z`, …) that the 2D path
//! deliberately leaves to us.
//!
//! Everything here is pure Rust (no GEOS, no native deps). Exact solid topology
//! and certified boolean/volume operations arrive in a later increment backed by
//! `parry3d` / SFCGAL behind a cargo feature; this baseline covers parsing,
//! bounding boxes, metric measures and footprints, which is what the index, the
//! viewer feed and the OGC API need first.

/// A 3-space coordinate. `z` defaults to 0 for a 2D vertex promoted into 3-space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coord3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Coord3 {
    pub fn new(x: f64, y: f64, z: f64) -> Self {
        Coord3 { x, y, z }
    }
    fn sub(self, o: Coord3) -> Coord3 {
        Coord3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
    fn cross(self, o: Coord3) -> Coord3 {
        Coord3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    fn dot(self, o: Coord3) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    fn norm(self) -> f64 {
        self.dot(self).sqrt()
    }
}

/// A planar polygon in 3-space: one exterior ring and optional interior rings.
#[derive(Debug, Clone, PartialEq)]
pub struct Polygon3 {
    pub exterior: Vec<Coord3>,
    pub interiors: Vec<Vec<Coord3>>,
}

/// A volumetric / 3D geometry (ISO 19107 / CityGML lineage).
#[derive(Debug, Clone, PartialEq)]
pub enum Geometry3D {
    Point(Coord3),
    LineString(Vec<Coord3>),
    Polygon(Polygon3),
    Triangle([Coord3; 3]),
    MultiPoint(Vec<Coord3>),
    MultiLineString(Vec<Vec<Coord3>>),
    MultiPolygon(Vec<Polygon3>),
    /// CityGML MultiSurface — a collection of faces. A closed `PolyhedralSurface`
    /// is the WKT encoding of a CityJSON `Solid`'s outer shell.
    PolyhedralSurface(Vec<Polygon3>),
    /// Triangulated irregular network (terrain).
    Tin(Vec<[Coord3; 3]>),
    GeometryCollection(Vec<Geometry3D>),
}

/// Axis-aligned 3D bounding box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb3 {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl Aabb3 {
    /// Do two AABBs overlap (closed intervals)? The broad-phase test the 3D
    /// index and `ots-geof:sf3dIntersects` use.
    pub fn overlaps(&self, o: &Aabb3) -> bool {
        self.min[0] <= o.max[0]
            && self.max[0] >= o.min[0]
            && self.min[1] <= o.max[1]
            && self.max[1] >= o.min[1]
            && self.min[2] <= o.max[2]
            && self.max[2] >= o.min[2]
    }
}

// ─── Parsing ────────────────────────────────────────────────────────────────

/// Heuristic: does this WKT body carry 3D geometry? True when it tags `Z`/`ZM`
/// or names a natively-3D type (`POLYHEDRALSURFACE`/`TIN`/`SOLID`). A plain 2D
/// `POINT(..)` is false so the existing GEOS 2D path keeps owning it.
/// Exposed for the datatypes-routing rule (spec §3.3); not yet wired internally.
#[allow(dead_code)]
pub fn wkt_is_3d(wkt: &str) -> bool {
    let upper = wkt.trim().to_ascii_uppercase();
    upper.contains(" Z ")
        || upper.contains(" Z(")
        || upper.contains(" ZM")
        || upper.starts_with("POLYHEDRALSURFACE")
        || upper.starts_with("TIN")
        || upper.starts_with("SOLID")
}

/// Parse a WKT-Z geometry. Tolerant of the optional `Z`/`ZM` tag and of vertices
/// written with only two ordinates (Z defaults to 0). Returns `None` on a shape
/// it does not model. The optional leading `<crs>` prefix must already be stripped.
pub fn parse_wkt3d(wkt: &str) -> Option<Geometry3D> {
    let mut p = Parser::new(wkt);
    let g = p.parse_geometry()?;
    Some(g)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> Parser<'a> {
    fn new(s: &'a str) -> Self {
        Parser { s: s.as_bytes(), i: 0 }
    }
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    /// Read an uppercase keyword (alphabetic run).
    fn keyword(&mut self) -> String {
        self.ws();
        let start = self.i;
        while self.i < self.s.len() && self.s[self.i].is_ascii_alphabetic() {
            self.i += 1;
        }
        std::str::from_utf8(&self.s[start..self.i])
            .unwrap_or("")
            .to_ascii_uppercase()
    }
    fn peek(&mut self) -> Option<u8> {
        self.ws();
        self.s.get(self.i).copied()
    }
    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    /// Skip an optional `Z` / `M` / `ZM` dimensionality tag and the `EMPTY` keyword.
    fn skip_dim_tag(&mut self) -> bool {
        self.ws();
        let save = self.i;
        let kw = self.keyword();
        if kw == "EMPTY" {
            return true; // signal empty
        }
        if kw == "Z" || kw == "M" || kw == "ZM" || kw.is_empty() {
            false
        } else {
            self.i = save; // not a tag — put it back
            false
        }
    }

    fn parse_geometry(&mut self) -> Option<Geometry3D> {
        let kw = self.keyword();
        match kw.as_str() {
            "POINT" => {
                if self.skip_dim_tag() {
                    return Some(Geometry3D::Point(Coord3::new(0.0, 0.0, 0.0)));
                }
                let pts = self.coord_group()?;
                Some(Geometry3D::Point(*pts.first()?))
            }
            "LINESTRING" => {
                if self.skip_dim_tag() {
                    return Some(Geometry3D::LineString(vec![]));
                }
                Some(Geometry3D::LineString(self.coord_group()?))
            }
            "POLYGON" => {
                if self.skip_dim_tag() {
                    return Some(Geometry3D::Polygon(Polygon3 { exterior: vec![], interiors: vec![] }));
                }
                Some(Geometry3D::Polygon(self.polygon_body()?))
            }
            "TRIANGLE" => {
                self.skip_dim_tag();
                let poly = self.polygon_body()?;
                let r = poly.exterior;
                if r.len() >= 3 {
                    Some(Geometry3D::Triangle([r[0], r[1], r[2]]))
                } else {
                    None
                }
            }
            "MULTIPOINT" => {
                if self.skip_dim_tag() {
                    return Some(Geometry3D::MultiPoint(vec![]));
                }
                // Accept both MULTIPOINT(1 2 3, 4 5 6) and MULTIPOINT((1 2 3),(4 5 6)).
                self.expect_open()?;
                let mut pts = Vec::new();
                loop {
                    if self.eat(b'(') {
                        if let Some(c) = self.one_coord() {
                            pts.push(c);
                        }
                        self.eat(b')');
                    } else if let Some(c) = self.one_coord() {
                        pts.push(c);
                    }
                    if !self.eat(b',') {
                        break;
                    }
                }
                self.eat(b')');
                Some(Geometry3D::MultiPoint(pts))
            }
            "MULTILINESTRING" => {
                if self.skip_dim_tag() {
                    return Some(Geometry3D::MultiLineString(vec![]));
                }
                self.expect_open()?;
                let mut lines = Vec::new();
                loop {
                    lines.push(self.coord_group()?);
                    if !self.eat(b',') {
                        break;
                    }
                }
                self.eat(b')');
                Some(Geometry3D::MultiLineString(lines))
            }
            "MULTIPOLYGON" => {
                if self.skip_dim_tag() {
                    return Some(Geometry3D::MultiPolygon(vec![]));
                }
                Some(Geometry3D::MultiPolygon(self.polygon_list()?))
            }
            "POLYHEDRALSURFACE" => {
                if self.skip_dim_tag() {
                    return Some(Geometry3D::PolyhedralSurface(vec![]));
                }
                Some(Geometry3D::PolyhedralSurface(self.polygon_list()?))
            }
            "TIN" => {
                if self.skip_dim_tag() {
                    return Some(Geometry3D::Tin(vec![]));
                }
                let polys = self.polygon_list()?;
                let mut tris = Vec::new();
                for p in polys {
                    if p.exterior.len() >= 3 {
                        tris.push([p.exterior[0], p.exterior[1], p.exterior[2]]);
                    }
                }
                Some(Geometry3D::Tin(tris))
            }
            "SOLID" => {
                // A SOLID's body is a parenthesised list of shells, each a
                // polyhedral surface. We keep the outer shell's faces.
                self.skip_dim_tag();
                self.expect_open()?;
                let faces = self.polygon_list()?;
                // consume any inner shells without modelling them yet
                while self.eat(b',') {
                    let _ = self.polygon_list();
                }
                self.eat(b')');
                Some(Geometry3D::PolyhedralSurface(faces))
            }
            "GEOMETRYCOLLECTION" => {
                if self.skip_dim_tag() {
                    return Some(Geometry3D::GeometryCollection(vec![]));
                }
                self.expect_open()?;
                let mut geoms = Vec::new();
                loop {
                    geoms.push(self.parse_geometry()?);
                    if !self.eat(b',') {
                        break;
                    }
                }
                self.eat(b')');
                Some(Geometry3D::GeometryCollection(geoms))
            }
            _ => None,
        }
    }

    fn expect_open(&mut self) -> Option<()> {
        if self.eat(b'(') {
            Some(())
        } else {
            None
        }
    }

    /// `( c, c, c )` — a single parenthesised coordinate sequence.
    fn coord_group(&mut self) -> Option<Vec<Coord3>> {
        self.expect_open()?;
        let mut coords = Vec::new();
        loop {
            coords.push(self.one_coord()?);
            if !self.eat(b',') {
                break;
            }
        }
        self.eat(b')');
        Some(coords)
    }

    /// `(( ext ),( int ),...)` — a polygon: exterior + interior rings.
    fn polygon_body(&mut self) -> Option<Polygon3> {
        self.expect_open()?;
        let mut rings = Vec::new();
        loop {
            rings.push(self.coord_group()?);
            if !self.eat(b',') {
                break;
            }
        }
        self.eat(b')');
        let mut it = rings.into_iter();
        let exterior = it.next()?;
        Some(Polygon3 {
            exterior,
            interiors: it.collect(),
        })
    }

    /// `( poly, poly, ... )` — used by MULTIPOLYGON / POLYHEDRALSURFACE / TIN.
    fn polygon_list(&mut self) -> Option<Vec<Polygon3>> {
        self.expect_open()?;
        let mut polys = Vec::new();
        loop {
            polys.push(self.polygon_body()?);
            if !self.eat(b',') {
                break;
            }
        }
        self.eat(b')');
        Some(polys)
    }

    /// One `x y [z [m]]` coordinate (z defaults to 0; m ignored).
    fn one_coord(&mut self) -> Option<Coord3> {
        let x = self.number()?;
        let y = self.number()?;
        let z = self.number().unwrap_or(0.0);
        let _m = self.number(); // optional measure, discarded
        Some(Coord3::new(x, y, z))
    }

    fn number(&mut self) -> Option<f64> {
        self.ws();
        let start = self.i;
        if self.i < self.s.len() && (self.s[self.i] == b'+' || self.s[self.i] == b'-') {
            self.i += 1;
        }
        let mut saw_digit = false;
        while self.i < self.s.len()
            && (self.s[self.i].is_ascii_digit()
                || self.s[self.i] == b'.'
                || self.s[self.i] == b'e'
                || self.s[self.i] == b'E'
                || self.s[self.i] == b'+'
                || self.s[self.i] == b'-')
        {
            if self.s[self.i].is_ascii_digit() {
                saw_digit = true;
            }
            // stop sign from being grabbed unless part of an exponent
            if (self.s[self.i] == b'+' || self.s[self.i] == b'-')
                && self.i > start
                && !(self.s[self.i - 1] == b'e' || self.s[self.i - 1] == b'E')
            {
                break;
            }
            self.i += 1;
        }
        if !saw_digit {
            self.i = start;
            return None;
        }
        std::str::from_utf8(&self.s[start..self.i])
            .ok()?
            .parse::<f64>()
            .ok()
    }
}

// ─── Measures & derived geometry ──────────────────────────────────────────────

impl Geometry3D {
    /// Visit every coordinate in the geometry.
    pub fn for_each_coord(&self, f: &mut impl FnMut(Coord3)) {
        match self {
            Geometry3D::Point(c) => f(*c),
            Geometry3D::LineString(cs) | Geometry3D::MultiPoint(cs) => cs.iter().for_each(|c| f(*c)),
            Geometry3D::Triangle(t) => t.iter().for_each(|c| f(*c)),
            Geometry3D::MultiLineString(ls) => ls.iter().flatten().for_each(|c| f(*c)),
            Geometry3D::Polygon(p) => poly_coords(p, f),
            Geometry3D::MultiPolygon(ps) | Geometry3D::PolyhedralSurface(ps) => {
                ps.iter().for_each(|p| poly_coords(p, f))
            }
            Geometry3D::Tin(ts) => ts.iter().flatten().for_each(|c| f(*c)),
            Geometry3D::GeometryCollection(gs) => gs.iter().for_each(|g| g.for_each_coord(f)),
        }
    }

    /// Axis-aligned bounding box, or `None` for an empty geometry.
    pub fn aabb(&self) -> Option<Aabb3> {
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        let mut any = false;
        self.for_each_coord(&mut |c| {
            any = true;
            min[0] = min[0].min(c.x);
            min[1] = min[1].min(c.y);
            min[2] = min[2].min(c.z);
            max[0] = max[0].max(c.x);
            max[1] = max[1].max(c.y);
            max[2] = max[2].max(c.z);
        });
        any.then_some(Aabb3 { min, max })
    }

    pub fn z_min(&self) -> Option<f64> {
        self.aabb().map(|b| b.min[2])
    }
    pub fn z_max(&self) -> Option<f64> {
        self.aabb().map(|b| b.max[2])
    }
    /// Vertical extent (`zMax - zMin`).
    pub fn height(&self) -> Option<f64> {
        self.aabb().map(|b| b.max[2] - b.min[2])
    }

    /// Geometric centre: the surface-area-weighted centroid of the triangulated
    /// faces (robust to repeated/shared ring vertices), falling back to the mean
    /// vertex for point/line geometry without faces. Not the volumetric centroid
    /// (that arrives with the exact solid kernel), but a stable centre — e.g.
    /// `(0.5, 0.5, 0.5)` for a unit cube.
    pub fn centroid(&self) -> Option<Coord3> {
        let tris = self.triangles();
        if !tris.is_empty() {
            let mut area_sum = 0.0;
            let mut acc = Coord3::new(0.0, 0.0, 0.0);
            for [a, b, c] in &tris {
                let area = 0.5 * b.sub(*a).cross(c.sub(*a)).norm();
                acc.x += area * (a.x + b.x + c.x) / 3.0;
                acc.y += area * (a.y + b.y + c.y) / 3.0;
                acc.z += area * (a.z + b.z + c.z) / 3.0;
                area_sum += area;
            }
            if area_sum > 0.0 {
                return Some(Coord3::new(acc.x / area_sum, acc.y / area_sum, acc.z / area_sum));
            }
        }
        // No faces (point / line): mean of vertices.
        let mut sum = Coord3::new(0.0, 0.0, 0.0);
        let mut n = 0u64;
        self.for_each_coord(&mut |c| {
            sum.x += c.x;
            sum.y += c.y;
            sum.z += c.z;
            n += 1;
        });
        (n > 0).then(|| Coord3::new(sum.x / n as f64, sum.y / n as f64, sum.z / n as f64))
    }

    /// Every triangle of the geometry's surfaces (fan-triangulated faces). Used
    /// by `area3d`/`volume`. Fan triangulation is exact for convex/planar faces
    /// (the CityJSON/3D BAG common case).
    pub fn triangles(&self) -> Vec<[Coord3; 3]> {
        let mut out = Vec::new();
        match self {
            Geometry3D::Triangle(t) => out.push(*t),
            Geometry3D::Tin(ts) => out.extend_from_slice(ts),
            Geometry3D::Polygon(p) => fan(&p.exterior, &mut out),
            Geometry3D::MultiPolygon(ps) | Geometry3D::PolyhedralSurface(ps) => {
                ps.iter().for_each(|p| fan(&p.exterior, &mut out))
            }
            Geometry3D::GeometryCollection(gs) => {
                gs.iter().for_each(|g| out.extend(g.triangles()))
            }
            _ => {}
        }
        out
    }

    /// Indexed triangle mesh: a deduplicated vertex list plus a triangle index
    /// list (`[u32; 3]` into the vertex list). This is the buffer layout the
    /// exact `parry3d` kernel (`TriMesh`, `convex_hull`) and any glTF/3D-Tiles
    /// exporter want. Returns `None` when the geometry has no faces (a point or
    /// line). Pure-Rust — no `parry3d` import lives here.
    ///
    /// Vertices are deduplicated by exact bit pattern (`to_bits`), which is what
    /// shared ring/face corners produce in practice; it does not weld vertices
    /// that merely round to the same value.
    ///
    /// Indexed-mesh form for a `parry3d` `TriMesh`; the exact narrow phase is
    /// currently a self-contained Möller test, so this is wired in only once the
    /// parry 0.17 `TriMesh::new` signature is pinned (see functions3d TODO).
    #[allow(dead_code)]
    pub fn trimesh(&self) -> Option<(Vec<[f64; 3]>, Vec<[u32; 3]>)> {
        use std::collections::HashMap;
        let tris = self.triangles();
        if tris.is_empty() {
            return None;
        }
        let mut verts: Vec<[f64; 3]> = Vec::new();
        let mut idx: Vec<[u32; 3]> = Vec::with_capacity(tris.len());
        // Key vertices by their exact f64 bit patterns so identical corners fold.
        let mut seen: HashMap<[u64; 3], u32> = HashMap::new();
        let mut intern = |c: Coord3| -> u32 {
            let key = [c.x.to_bits(), c.y.to_bits(), c.z.to_bits()];
            if let Some(&i) = seen.get(&key) {
                return i;
            }
            let i = verts.len() as u32;
            verts.push([c.x, c.y, c.z]);
            seen.insert(key, i);
            i
        };
        for [a, b, c] in &tris {
            idx.push([intern(*a), intern(*b), intern(*c)]);
        }
        Some((verts, idx))
    }

    /// Total surface area (sum of triangle areas), metres² in a metric CRS.
    pub fn area3d(&self) -> f64 {
        self.triangles()
            .iter()
            .map(|[a, b, c]| 0.5 * b.sub(*a).cross(c.sub(*a)).norm())
            .sum()
    }

    /// Volume of a closed surface via the divergence theorem (signed tetrahedra
    /// from the origin). Assumes consistently-oriented faces (a CityJSON Solid
    /// guarantee); returns the absolute value, so a uniformly inward orientation
    /// is fine too. Open/non-solid geometry yields a meaningless small number —
    /// callers gate on geometry type.
    pub fn volume(&self) -> f64 {
        let v: f64 = self
            .triangles()
            .iter()
            .map(|[a, b, c]| a.dot(b.cross(*c)) / 6.0)
            .sum();
        v.abs()
    }

    /// 2D footprint: the convex hull of all vertices projected to the XY plane,
    /// as a closed ring of `(x, y)`. Bridges back to the GEOS 2D predicates
    /// (spec §3.4). Convex hull is an approximation of a true (possibly concave)
    /// footprint — adequate for proximity/containment pre-filtering.
    pub fn footprint_xy(&self) -> Vec<(f64, f64)> {
        let mut pts = Vec::new();
        self.for_each_coord(&mut |c| pts.push((c.x, c.y)));
        convex_hull_2d(&mut pts)
    }
}

fn poly_coords(p: &Polygon3, f: &mut impl FnMut(Coord3)) {
    p.exterior.iter().for_each(|c| f(*c));
    p.interiors.iter().flatten().for_each(|c| f(*c));
}

/// Fan-triangulate a ring from its first vertex.
fn fan(ring: &[Coord3], out: &mut Vec<[Coord3; 3]>) {
    if ring.len() < 3 {
        return;
    }
    // Drop the closing duplicate vertex if present.
    let n = if ring.first() == ring.last() && ring.len() > 3 {
        ring.len() - 1
    } else {
        ring.len()
    };
    for k in 1..n - 1 {
        out.push([ring[0], ring[k], ring[k + 1]]);
    }
}

/// Andrew's monotone-chain convex hull over XY points. Returns a closed CCW ring
/// (first vertex repeated at the end), or the input degenerately for < 3 points.
pub fn convex_hull_2d(pts: &mut [(f64, f64)]) -> Vec<(f64, f64)> {
    let mut p: Vec<(f64, f64)> = pts.to_vec();
    p.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    p.dedup();
    let n = p.len();
    if n < 3 {
        return p;
    }
    let cross = |o: (f64, f64), a: (f64, f64), b: (f64, f64)| {
        (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
    };
    let mut hull: Vec<(f64, f64)> = Vec::with_capacity(2 * n);
    // lower
    for &pt in &p {
        while hull.len() >= 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], pt) <= 0.0 {
            hull.pop();
        }
        hull.push(pt);
    }
    // upper
    let lower = hull.len() + 1;
    for &pt in p.iter().rev() {
        while hull.len() >= lower && cross(hull[hull.len() - 2], hull[hull.len() - 1], pt) <= 0.0 {
            hull.pop();
        }
        hull.push(pt);
    }
    hull.pop(); // last == first of the upper start
    if let Some(&first) = hull.first() {
        hull.push(first);
    }
    hull
}

/// Squared distance from point `p` to triangle `(a,b,c)` (Ericson, *Real-Time
/// Collision Detection* §5.1.5). Exact closest-point-on-triangle.
pub fn point_tri_dist2(p: Coord3, a: Coord3, b: Coord3, c: Coord3) -> f64 {
    let ab = b.sub(a);
    let ac = c.sub(a);
    let ap = p.sub(a);
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return ap.dot(ap);
    }
    let bp = p.sub(b);
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return bp.dot(bp);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        let q = Coord3::new(a.x + v * ab.x, a.y + v * ab.y, a.z + v * ab.z);
        return p.sub(q).dot(p.sub(q));
    }
    let cp = p.sub(c);
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return cp.dot(cp);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        let q = Coord3::new(a.x + w * ac.x, a.y + w * ac.y, a.z + w * ac.z);
        return p.sub(q).dot(p.sub(q));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        let bc = c.sub(b);
        let q = Coord3::new(b.x + w * bc.x, b.y + w * bc.y, b.z + w * bc.z);
        return p.sub(q).dot(p.sub(q));
    }
    // inside face region — project onto the plane
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    let q = Coord3::new(
        a.x + ab.x * v + ac.x * w,
        a.y + ab.y * v + ac.y * w,
        a.z + ab.z * v + ac.z * w,
    );
    p.sub(q).dot(p.sub(q))
}

/// Approximate minimum distance between two 3D geometries: the smaller of
/// "each vertex of A to the nearest triangle of B" and the symmetric case,
/// falling back to vertex–vertex when neither has triangles. This is exact for
/// point↔mesh and point↔point and a tight upper bound for mesh↔mesh (it can
/// miss an edge–edge closest pair); the exact narrow phase arrives with the
/// `parry3d` kernel.
pub fn distance3d(g1: &Geometry3D, g2: &Geometry3D) -> Option<f64> {
    let mut v1 = Vec::new();
    g1.for_each_coord(&mut |c| v1.push(c));
    let mut v2 = Vec::new();
    g2.for_each_coord(&mut |c| v2.push(c));
    if v1.is_empty() || v2.is_empty() {
        return None;
    }
    let t1 = g1.triangles();
    let t2 = g2.triangles();
    let mut best = f64::INFINITY;
    // vertices of A vs triangles of B
    if !t2.is_empty() {
        for &p in &v1 {
            for t in &t2 {
                best = best.min(point_tri_dist2(p, t[0], t[1], t[2]));
            }
        }
    }
    // vertices of B vs triangles of A
    if !t1.is_empty() {
        for &p in &v2 {
            for t in &t1 {
                best = best.min(point_tri_dist2(p, t[0], t[1], t[2]));
            }
        }
    }
    // vertex–vertex fallback (covers point/line geometries with no triangles)
    if t1.is_empty() && t2.is_empty() {
        for &a in &v1 {
            for &b in &v2 {
                best = best.min(a.sub(b).dot(a.sub(b)));
            }
        }
    }
    best.is_finite().then(|| best.sqrt())
}

/// Extrude a footprint ring (closed `(x,y[,z])`) upward by `height`, producing a
/// closed `PolyhedralSurface` solid (bottom + top + side walls), consistently
/// oriented. The base sits at the ring's own Z (0 if 2D).
pub fn extrude(ring: &[Coord3], height: f64) -> Option<Geometry3D> {
    // normalise the ring: drop the closing duplicate, need ≥3 distinct vertices
    let mut base: Vec<Coord3> = ring.to_vec();
    if base.first() == base.last() && base.len() > 1 {
        base.pop();
    }
    if base.len() < 3 {
        return None;
    }
    let top: Vec<Coord3> = base
        .iter()
        .map(|c| Coord3::new(c.x, c.y, c.z + height))
        .collect();
    let close = |mut v: Vec<Coord3>| {
        if let Some(&f) = v.first() {
            v.push(f);
        }
        v
    };
    let mut faces = Vec::new();
    // bottom (reverse for downward normal) and top
    let mut bottom_ring = base.clone();
    bottom_ring.reverse();
    faces.push(Polygon3 { exterior: close(bottom_ring), interiors: vec![] });
    faces.push(Polygon3 { exterior: close(top.clone()), interiors: vec![] });
    // side walls
    let n = base.len();
    for i in 0..n {
        let j = (i + 1) % n;
        let wall = vec![base[i], base[j], top[j], top[i], base[i]];
        faces.push(Polygon3 { exterior: wall, interiors: vec![] });
    }
    Some(Geometry3D::PolyhedralSurface(faces))
}

// ─── WKT serialisation ────────────────────────────────────────────────────────

fn fmt_coord(c: &Coord3) -> String {
    format!("{} {} {}", trim(c.x), trim(c.y), trim(c.z))
}
fn trim(v: f64) -> String {
    // compact but lossless-enough numeric formatting
    let s = format!("{v}");
    s
}
fn fmt_ring(ring: &[Coord3]) -> String {
    let inner: Vec<String> = ring.iter().map(fmt_coord).collect();
    format!("({})", inner.join(","))
}
fn fmt_poly(p: &Polygon3) -> String {
    let mut rings = vec![fmt_ring(&p.exterior)];
    rings.extend(p.interiors.iter().map(|r| fmt_ring(r)));
    format!("({})", rings.join(","))
}

/// Serialise a 3D geometry to ISO-13249 WKT-Z (`POINT Z (..)`, `POLYHEDRALSURFACE
/// Z (..)`, …). The output is a valid `geo:wktLiteral` body for max interop.
pub fn to_wkt3d(g: &Geometry3D) -> String {
    match g {
        Geometry3D::Point(c) => format!("POINT Z ({})", fmt_coord(c)),
        Geometry3D::LineString(cs) => format!("LINESTRING Z {}", fmt_ring(cs)),
        Geometry3D::Polygon(p) => format!("POLYGON Z {}", fmt_poly(p)),
        Geometry3D::Triangle(t) => {
            format!("TRIANGLE Z (({}))", t.iter().map(fmt_coord).chain(std::iter::once(fmt_coord(&t[0]))).collect::<Vec<_>>().join(","))
        }
        Geometry3D::MultiPoint(cs) => {
            let inner: Vec<String> = cs.iter().map(|c| format!("({})", fmt_coord(c))).collect();
            format!("MULTIPOINT Z ({})", inner.join(","))
        }
        Geometry3D::MultiLineString(ls) => {
            let inner: Vec<String> = ls.iter().map(|l| fmt_ring(l)).collect();
            format!("MULTILINESTRING Z ({})", inner.join(","))
        }
        Geometry3D::MultiPolygon(ps) => {
            let inner: Vec<String> = ps.iter().map(fmt_poly).collect();
            format!("MULTIPOLYGON Z ({})", inner.join(","))
        }
        Geometry3D::PolyhedralSurface(ps) => {
            let inner: Vec<String> = ps.iter().map(fmt_poly).collect();
            format!("POLYHEDRALSURFACE Z ({})", inner.join(","))
        }
        Geometry3D::Tin(ts) => {
            let inner: Vec<String> = ts
                .iter()
                .map(|t| {
                    format!(
                        "(({}))",
                        t.iter().map(fmt_coord).chain(std::iter::once(fmt_coord(&t[0]))).collect::<Vec<_>>().join(",")
                    )
                })
                .collect();
            format!("TIN Z ({})", inner.join(","))
        }
        Geometry3D::GeometryCollection(gs) => {
            let inner: Vec<String> = gs.iter().map(to_wkt3d).collect();
            format!("GEOMETRYCOLLECTION Z ({})", inner.join(","))
        }
    }
}

/// AABB as a closed `POLYHEDRALSURFACE Z` box (6 quad faces).
pub fn aabb_to_wkt(b: &Aabb3) -> String {
    let [x0, y0, z0] = b.min;
    let [x1, y1, z1] = b.max;
    let c = |x: f64, y: f64, z: f64| Coord3::new(x, y, z);
    let faces = vec![
        // bottom z0, top z1, and four walls — outward-oriented
        Polygon3 { exterior: vec![c(x0, y0, z0), c(x0, y1, z0), c(x1, y1, z0), c(x1, y0, z0), c(x0, y0, z0)], interiors: vec![] },
        Polygon3 { exterior: vec![c(x0, y0, z1), c(x1, y0, z1), c(x1, y1, z1), c(x0, y1, z1), c(x0, y0, z1)], interiors: vec![] },
        Polygon3 { exterior: vec![c(x0, y0, z0), c(x0, y0, z1), c(x0, y1, z1), c(x0, y1, z0), c(x0, y0, z0)], interiors: vec![] },
        Polygon3 { exterior: vec![c(x1, y0, z0), c(x1, y1, z0), c(x1, y1, z1), c(x1, y0, z1), c(x1, y0, z0)], interiors: vec![] },
        Polygon3 { exterior: vec![c(x0, y0, z0), c(x1, y0, z0), c(x1, y0, z1), c(x0, y0, z1), c(x0, y0, z0)], interiors: vec![] },
        Polygon3 { exterior: vec![c(x0, y1, z0), c(x0, y1, z1), c(x1, y1, z1), c(x1, y1, z0), c(x0, y1, z0)], interiors: vec![] },
    ];
    to_wkt3d(&Geometry3D::PolyhedralSurface(faces))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit cube [0,1]³ with all faces consistently outward-oriented.
    const CUBE: &str = "POLYHEDRALSURFACE Z (\
        ((0 0 0,0 1 0,1 1 0,1 0 0,0 0 0)),\
        ((0 0 1,1 0 1,1 1 1,0 1 1,0 0 1)),\
        ((0 0 0,0 0 1,0 1 1,0 1 0,0 0 0)),\
        ((1 0 0,1 1 0,1 1 1,1 0 1,1 0 0)),\
        ((0 0 0,1 0 0,1 0 1,0 0 1,0 0 0)),\
        ((0 1 0,0 1 1,1 1 1,1 1 0,0 1 0)))";

    #[test]
    fn detects_3d() {
        assert!(wkt_is_3d("POINT Z (1 2 3)"));
        assert!(wkt_is_3d("POLYHEDRALSURFACE Z (((0 0 0,1 0 0,1 1 0,0 0 0)))"));
        assert!(wkt_is_3d("TIN Z (((0 0 0,1 0 0,0 1 0,0 0 0)))"));
        assert!(!wkt_is_3d("POINT(1 2)"));
        assert!(!wkt_is_3d("POLYGON((0 0,1 0,1 1,0 0))"));
    }

    #[test]
    fn parses_point_z() {
        let g = parse_wkt3d("POINT Z (1 2 3)").unwrap();
        assert_eq!(g, Geometry3D::Point(Coord3::new(1.0, 2.0, 3.0)));
        // tagless 3-ordinate point also parses
        let g2 = parse_wkt3d("POINT(4 5 6)").unwrap();
        assert_eq!(g2, Geometry3D::Point(Coord3::new(4.0, 5.0, 6.0)));
    }

    #[test]
    fn cube_measures() {
        let cube = parse_wkt3d(CUBE).unwrap();
        let b = cube.aabb().unwrap();
        assert_eq!(b.min, [0.0, 0.0, 0.0]);
        assert_eq!(b.max, [1.0, 1.0, 1.0]);
        assert!((cube.area3d() - 6.0).abs() < 1e-9, "area {}", cube.area3d());
        assert!((cube.volume() - 1.0).abs() < 1e-9, "volume {}", cube.volume());
        assert!((cube.height().unwrap() - 1.0).abs() < 1e-9);
        assert_eq!(cube.z_min(), Some(0.0));
        assert_eq!(cube.z_max(), Some(1.0));
        let cen = cube.centroid().unwrap();
        assert!((cen.x - 0.5).abs() < 1e-9 && (cen.y - 0.5).abs() < 1e-9 && (cen.z - 0.5).abs() < 1e-9);
    }

    #[test]
    fn extrude_unit_square_is_unit_cube_volume() {
        let square = [
            Coord3::new(0.0, 0.0, 0.0),
            Coord3::new(1.0, 0.0, 0.0),
            Coord3::new(1.0, 1.0, 0.0),
            Coord3::new(0.0, 1.0, 0.0),
        ];
        let solid = extrude(&square, 1.0).unwrap();
        assert!((solid.volume() - 1.0).abs() < 1e-9, "vol {}", solid.volume());
        assert!((solid.height().unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn distance_point_to_cube() {
        let cube = parse_wkt3d(CUBE).unwrap();
        let p = parse_wkt3d("POINT Z (3 0.5 0.5)").unwrap();
        let d = distance3d(&p, &cube).unwrap();
        assert!((d - 2.0).abs() < 1e-9, "distance {d}");
    }

    #[test]
    fn footprint_of_cube_is_unit_square_hull() {
        let cube = parse_wkt3d(CUBE).unwrap();
        let fp = cube.footprint_xy();
        // hull of the 8 projected corners = the unit square ring (5 pts closed)
        assert_eq!(fp.first(), fp.last());
        assert_eq!(fp.len(), 5);
    }

    #[test]
    fn parses_polyhedralsurface_roundtrip() {
        let cube = parse_wkt3d(CUBE).unwrap();
        let wkt = to_wkt3d(&cube);
        let reparsed = parse_wkt3d(&wkt).unwrap();
        assert!((reparsed.volume() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn trimesh_of_cube_dedups_corners() {
        let cube = parse_wkt3d(CUBE).unwrap();
        let (verts, idx) = cube.trimesh().unwrap();
        // A cube has 8 distinct corners and 12 triangles (6 quad faces × 2).
        assert_eq!(verts.len(), 8, "deduped vertices {}", verts.len());
        assert_eq!(idx.len(), 12, "triangles {}", idx.len());
        // Every index is in range.
        for t in &idx {
            for &i in t {
                assert!((i as usize) < verts.len());
            }
        }
    }

    #[test]
    fn trimesh_of_point_is_none() {
        let p = parse_wkt3d("POINT Z (1 2 3)").unwrap();
        assert!(p.trimesh().is_none());
    }

    #[test]
    fn tin_area() {
        // two unit right-triangles in the z=0 plane → area 1.0
        let tin = parse_wkt3d("TIN Z (((0 0 0,1 0 0,0 1 0,0 0 0)),((1 0 0,1 1 0,0 1 0,1 0 0)))").unwrap();
        assert!((tin.area3d() - 1.0).abs() < 1e-9, "area {}", tin.area3d());
    }
}
