//! The `ots-geof:` 3D function surface (spec §3.4) — additive volumetric
//! predicates and measures registered beside the GeoSPARQL 1.1 `geof:` surface.
//!
//! Namespace: `https://open-triplestore.org/def/function/geo3d/`. These never
//! touch the 2D GEOS path, so GeoSPARQL 1.1 stays byte-for-byte conformant.
//! Each function parses ISO-13249 WKT-Z literals via [`super::geom3d`] and
//! returns an `xsd:double` / `xsd:boolean` / `geo:wktLiteral` term.
//!
//! Two tiers sit on top of the pure-Rust parsing/measures in [`super::geom3d`]:
//!
//! - **Always on** (metric/constructive/broad-phase): `distance3d`, `volume`,
//!   `area3d`, `zMin/zMax/height`, `boundingBox3d`, `centroid3d`, `footprint2d`,
//!   `extrude`.
//! - **Exact narrow phase** (`geometry3d` feature, backed by `parry3d-f64`):
//!   `sf3dIntersects`/`sf3dDisjoint` now run an exact triangle-mesh narrow phase
//!   after the AABB fast-reject; `convexHull3d` builds the hull via parry3d;
//!   `sf3dContains`/`sf3dWithin` are an exact point-in-solid test.
//! - **Certified CSG** (`sfcgal3d` feature, NOT in `full`): `union3d`,
//!   `intersection3d`, `difference3d` and certified `volumeExact` via SFCGAL.
//!   This needs the native libSFCGAL C library and is excluded from the default
//!   build, so all of it is guarded by `#[cfg(feature = "sfcgal3d")]`.

use std::sync::Arc;

use oxrdf::{Literal, NamedNode, Term};

use super::datatypes::{boolean_literal, double_literal, extract_wkt};
use super::geom3d::{self, Coord3, Geometry3D};
use super::vocabulary as vocab;

type FnHandler = Arc<dyn Fn(&[Term]) -> Option<Term> + Send + Sync>;

/// All `ots-geof:` 3D functions as (IRI, handler) pairs for registration.
pub fn all_functions_3d() -> Vec<(NamedNode, FnHandler)> {
    vec![
        // ─── 3D metric ───
        make_fn(vocab::OTS3D_DISTANCE3D, fn_distance3d),
        make_fn(vocab::OTS3D_VOLUME, fn_volume),
        make_fn(vocab::OTS3D_AREA3D, fn_area3d),
        make_fn(vocab::OTS3D_ZMIN, fn_zmin),
        make_fn(vocab::OTS3D_ZMAX, fn_zmax),
        make_fn(vocab::OTS3D_HEIGHT, fn_height),
        // ─── 3D constructive ───
        make_fn(vocab::OTS3D_BBOX3D, fn_bbox3d),
        make_fn(vocab::OTS3D_CENTROID3D, fn_centroid3d),
        make_fn(vocab::OTS3D_FOOTPRINT2D, fn_footprint2d),
        make_fn(vocab::OTS3D_EXTRUDE, fn_extrude),
        make_fn(vocab::OTS3D_CONVEXHULL3D, fn_convex_hull3d),
        // ─── 3D topological (AABB fast-reject + exact triangle narrow phase) ───
        make_fn(vocab::OTS3D_SF_INTERSECTS, fn_sf3d_intersects),
        make_fn(vocab::OTS3D_SF_DISJOINT, fn_sf3d_disjoint),
        make_fn(vocab::OTS3D_SF_CONTAINS, fn_sf3d_contains),
        make_fn(vocab::OTS3D_SF_WITHIN, fn_sf3d_within),
        // ─── certified CSG (SFCGAL; only present in an `sfcgal3d` build) ───
        #[cfg(feature = "sfcgal3d")]
        make_fn(vocab::OTS3D_UNION3D, fn_union3d),
        #[cfg(feature = "sfcgal3d")]
        make_fn(vocab::OTS3D_INTERSECTION3D, fn_intersection3d),
        #[cfg(feature = "sfcgal3d")]
        make_fn(vocab::OTS3D_DIFFERENCE3D, fn_difference3d),
        #[cfg(feature = "sfcgal3d")]
        make_fn(vocab::OTS3D_VOLUME_EXACT, fn_volume_exact),
    ]
}

fn make_fn(iri: &str, f: fn(&[Term]) -> Option<Term>) -> (NamedNode, FnHandler) {
    (NamedNode::new_unchecked(iri), Arc::new(f))
}

/// Parse a 3D geometry from a `geo:wktLiteral` / `xsd:string` term (CRS prefix
/// stripped). Returns `None` for a non-literal or unparseable shape.
fn parse_geom3d(term: &Term) -> Option<Geometry3D> {
    let lit = match term {
        Term::Literal(l) => l,
        _ => return None,
    };
    // WKT-Z (and plain strings, for convenience). The `ots:cityjsonGeometryLiteral`
    // path lands with the P2 CityJSON ingest, alongside its loss-free parser.
    let dt = lit.datatype();
    let ok = dt.as_str() == vocab::WKT_LITERAL
        || dt.as_str() == "http://www.w3.org/2001/XMLSchema#string";
    if !ok {
        return None;
    }
    let body = extract_wkt(lit.value());
    geom3d::parse_wkt3d(body)
}

fn parse_two(args: &[Term]) -> Option<(Geometry3D, Geometry3D)> {
    Some((parse_geom3d(args.first()?)?, parse_geom3d(args.get(1)?)?))
}

fn wkt_literal(wkt: String) -> Term {
    Term::Literal(Literal::new_typed_literal(
        wkt,
        NamedNode::new_unchecked(vocab::WKT_LITERAL),
    ))
}

// ─── metric ───

fn fn_distance3d(args: &[Term]) -> Option<Term> {
    let (a, b) = parse_two(args)?;
    geom3d::distance3d(&a, &b).map(double_literal)
}

fn fn_volume(args: &[Term]) -> Option<Term> {
    let g = parse_geom3d(args.first()?)?;
    Some(double_literal(g.volume()))
}

fn fn_area3d(args: &[Term]) -> Option<Term> {
    let g = parse_geom3d(args.first()?)?;
    Some(double_literal(g.area3d()))
}

fn fn_zmin(args: &[Term]) -> Option<Term> {
    parse_geom3d(args.first()?)?.z_min().map(double_literal)
}

fn fn_zmax(args: &[Term]) -> Option<Term> {
    parse_geom3d(args.first()?)?.z_max().map(double_literal)
}

fn fn_height(args: &[Term]) -> Option<Term> {
    parse_geom3d(args.first()?)?.height().map(double_literal)
}

// ─── constructive ───

fn fn_bbox3d(args: &[Term]) -> Option<Term> {
    let g = parse_geom3d(args.first()?)?;
    let b = g.aabb()?;
    Some(wkt_literal(geom3d::aabb_to_wkt(&b)))
}

fn fn_centroid3d(args: &[Term]) -> Option<Term> {
    let g = parse_geom3d(args.first()?)?;
    let c = g.centroid()?;
    Some(wkt_literal(geom3d::to_wkt3d(&Geometry3D::Point(c))))
}

fn fn_footprint2d(args: &[Term]) -> Option<Term> {
    let g = parse_geom3d(args.first()?)?;
    let ring = g.footprint_xy();
    if ring.len() < 4 {
        return None;
    }
    let inner: Vec<String> = ring.iter().map(|(x, y)| format!("{x} {y}")).collect();
    Some(wkt_literal(format!("POLYGON(({}))", inner.join(","))))
}

fn fn_extrude(args: &[Term]) -> Option<Term> {
    let g = parse_geom3d(args.first()?)?;
    let height = match args.get(1)? {
        Term::Literal(l) => l.value().parse::<f64>().ok()?,
        _ => return None,
    };
    // Extrude the exterior ring of a polygon, or the footprint of anything else.
    let ring = match &g {
        Geometry3D::Polygon(p) => p.exterior.clone(),
        other => other
            .footprint_xy()
            .into_iter()
            .map(|(x, y)| geom3d::Coord3::new(x, y, other.z_min().unwrap_or(0.0)))
            .collect(),
    };
    let solid = geom3d::extrude(&ring, height)?;
    Some(wkt_literal(geom3d::to_wkt3d(&solid)))
}

// ─── constructive: exact convex hull (parry3d) ───

fn fn_convex_hull3d(args: &[Term]) -> Option<Term> {
    let g = parse_geom3d(args.first()?)?;
    let hull = convex_hull3d(&g)?;
    Some(wkt_literal(geom3d::to_wkt3d(&hull)))
}

/// Convex hull of a 3D geometry as a closed `PolyhedralSurface` solid, via
/// `parry3d_f64::transformation::convex_hull` (returns hull vertices + a triangle
/// index buffer). Returns `None` for fewer than four non-coplanar points (no 3D
/// hull). `parry3d_f64::na` is parry's re-exported nalgebra — we do not depend on
/// nalgebra directly, to avoid version skew.
fn convex_hull3d(g: &Geometry3D) -> Option<Geometry3D> {
    use parry3d_f64::na::Point3;

    let mut pts: Vec<Point3<f64>> = Vec::new();
    g.for_each_coord(&mut |c| pts.push(Point3::new(c.x, c.y, c.z)));
    if pts.len() < 4 {
        return None;
    }
    // (vertices, triangle index buffer of the hull faces)
    let (verts, indices) = parry3d_f64::transformation::convex_hull(&pts);
    if verts.is_empty() || indices.is_empty() {
        return None;
    }
    let faces = indices
        .iter()
        .map(|[i, j, k]| {
            let p = |n: u32| {
                let v = verts[n as usize];
                Coord3::new(v.x, v.y, v.z)
            };
            let (a, b, c) = (p(*i), p(*j), p(*k));
            // Closed triangular ring (first vertex repeated).
            geom3d::Polygon3 {
                exterior: vec![a, b, c, a],
                interiors: vec![],
            }
        })
        .collect();
    Some(Geometry3D::PolyhedralSurface(faces))
}

// ─── topological: AABB fast-reject + exact triangle narrow phase ───

fn fn_sf3d_intersects(args: &[Term]) -> Option<Term> {
    let (a, b) = parse_two(args)?;
    Some(boolean_literal(exact_intersects(&a, &b)?))
}

fn fn_sf3d_disjoint(args: &[Term]) -> Option<Term> {
    let (a, b) = parse_two(args)?;
    Some(boolean_literal(!exact_intersects(&a, &b)?))
}

/// Exact intersection test. The AABBs are a fast reject; on overlap we run an
/// exact triangle-mesh narrow phase. Geometries without triangles (points,
/// lines) fall back to `distance3d == 0`, and a point-vs-solid pair also checks
/// containment so a point strictly inside a closed solid counts as intersecting.
///
/// The triangle–triangle narrow phase is a robust pure-Rust Möller test (see
/// [`tri_tri_intersect`]). `parry3d`'s `TriMesh` + `query::intersection_test`
/// would be an equivalent drop-in; we keep the well-defined pure-Rust path here
/// to avoid relying on the exact `TriMesh::new`/`intersection_test` signatures
/// (which shifted across parry releases) while still getting an exact answer.
/// TODO(parry): swap to `parry3d_f64::query::intersection_test(&Isometry::identity(),
/// &mesh1, &Isometry::identity(), &mesh2)` once we pin the 0.17 TriMesh API.
fn exact_intersects(a: &Geometry3D, b: &Geometry3D) -> Option<bool> {
    let (ba, bb) = (a.aabb()?, b.aabb()?);
    // Fast reject: disjoint bounding boxes ⇒ disjoint geometries.
    if !ba.overlaps(&bb) {
        return Some(false);
    }
    let ta = a.triangles();
    let tb = b.triangles();
    // Both have surfaces: exact triangle–triangle narrow phase, plus a
    // containment check so a fully-enclosed solid (no crossing faces) is caught.
    if !ta.is_empty() && !tb.is_empty() {
        for u in &ta {
            for v in &tb {
                if tri_tri_intersect(u, v) {
                    return Some(true);
                }
            }
        }
        // No face crossing — one solid may still wholly enclose the other.
        if let Some(p) = tb.first().map(|t| t[0]) {
            if point_in_solid(p, &ta) {
                return Some(true);
            }
        }
        if let Some(p) = ta.first().map(|t| t[0]) {
            if point_in_solid(p, &tb) {
                return Some(true);
            }
        }
        return Some(false);
    }
    // At least one geometry has no triangles (point/line). A point inside a
    // closed solid counts as intersecting; otherwise fall back to coincidence.
    if ta.is_empty() && !tb.is_empty() {
        let mut hit = false;
        a.for_each_coord(&mut |c| hit = hit || point_in_solid(c, &tb));
        if hit {
            return Some(true);
        }
    }
    if tb.is_empty() && !ta.is_empty() {
        let mut hit = false;
        b.for_each_coord(&mut |c| hit = hit || point_in_solid(c, &ta));
        if hit {
            return Some(true);
        }
    }
    Some(geom3d::distance3d(a, b).map(|d| d <= 1e-9).unwrap_or(false))
}

// ─── topological: exact point-in-solid containment ───

fn fn_sf3d_contains(args: &[Term]) -> Option<Term> {
    let (a, b) = parse_two(args)?;
    Some(boolean_literal(solid_contains(&a, &b)?))
}

fn fn_sf3d_within(args: &[Term]) -> Option<Term> {
    let (a, b) = parse_two(args)?;
    // a within b  ⇔  b contains a
    Some(boolean_literal(solid_contains(&b, &a)?))
}

/// Exact "solid `a` contains geometry `b`": every vertex of `b` lies inside (or
/// on) the closed solid `a`. `a` must have triangulated faces (a closed
/// `PolyhedralSurface`/`Solid`/`TIN`); without them there is no volume and the
/// answer is `None`.
fn solid_contains(a: &Geometry3D, b: &Geometry3D) -> Option<bool> {
    let ta = a.triangles();
    if ta.is_empty() {
        return None; // not a solid — no interior to contain anything
    }
    // AABB fast reject: anything outside a's box can't be contained.
    let ba = a.aabb()?;
    let mut all_inside = true;
    let mut any = false;
    b.for_each_coord(&mut |c| {
        any = true;
        if !point_in_aabb(c, &ba) || !point_in_solid(c, &ta) {
            all_inside = false;
        }
    });
    Some(any && all_inside)
}

fn point_in_aabb(c: Coord3, b: &geom3d::Aabb3) -> bool {
    c.x >= b.min[0] - 1e-9
        && c.x <= b.max[0] + 1e-9
        && c.y >= b.min[1] - 1e-9
        && c.y <= b.max[1] + 1e-9
        && c.z >= b.min[2] - 1e-9
        && c.z <= b.max[2] + 1e-9
}

/// Robust point-in-closed-mesh test by ray-cast parity (odd crossings ⇒ inside).
/// Casts a ray along +X and counts triangle crossings; a point lying on a face
/// counts as inside. Pure-Rust and orientation-independent (it does not rely on
/// consistent face winding), which suits the parry-free narrow phase above.
///
/// The +X ray can graze a shared edge/vertex and double- or zero-count; to stay
/// robust we retry along a few perturbed directions and take the majority — for
/// the watertight CityJSON solids this targets, the directions agree.
fn point_in_solid(p: Coord3, tris: &[[Coord3; 3]]) -> bool {
    // On-surface ⇒ inside (closed solid includes its boundary).
    for t in tris {
        if geom3d::point_tri_dist2(p, t[0], t[1], t[2]) <= 1e-18 {
            return true;
        }
    }
    // Asymmetric, non-axis-aligned directions: an axis-aligned ray from a
    // symmetric point (e.g. a cube's centre) grazes the fan-diagonal of every
    // face and would be discarded. These irrational-ish directions don't align
    // with axis-aligned solids' triangulation seams.
    let dirs = [
        [0.351_021, 0.642_117, 0.681_309],
        [0.713_402, -0.402_193, 0.573_021],
        [-0.488_113, 0.611_902, 0.622_417],
        [0.902_113, 0.211_307, -0.376_402],
        [-0.276_401, -0.821_330, 0.499_201],
        [0.602_113, 0.733_402, -0.314_207],
        [-0.711_023, 0.213_402, -0.670_119],
    ];
    let mut votes_inside = 0u32;
    let mut clean = 0u32; // directions that did not graze an edge/vertex
    for d in &dirs {
        let mut crossings = 0u32;
        let mut degenerate = false;
        for t in tris {
            match ray_tri_cross(p, *d, t) {
                RayHit::Cross => crossings += 1,
                RayHit::Miss => {}
                RayHit::Grazing => {
                    degenerate = true;
                    break;
                }
            }
        }
        if degenerate {
            continue; // this direction grazed an edge — ignore its vote
        }
        clean += 1;
        if crossings % 2 == 1 {
            votes_inside += 1;
        }
    }
    // Majority of the *clean* directions (all of which agree for a watertight
    // solid); no clean direction at all ⇒ treat as outside.
    clean > 0 && votes_inside * 2 > clean
}

enum RayHit {
    Cross,
    Miss,
    /// The ray passes through an edge/vertex or lies in the triangle's plane —
    /// parity is ambiguous for this direction.
    Grazing,
}

/// Möller–Trumbore ray/triangle intersection for the forward (t > 0) half-line,
/// classifying boundary hits as `Grazing` so the caller can pick another ray.
fn ray_tri_cross(orig: Coord3, dir: [f64; 3], tri: &[Coord3; 3]) -> RayHit {
    const EPS: f64 = 1e-12;
    let d = Coord3::new(dir[0], dir[1], dir[2]);
    let e1 = sub(tri[1], tri[0]);
    let e2 = sub(tri[2], tri[0]);
    let pvec = cross(d, e2);
    let det = dot(e1, pvec);
    if det.abs() < EPS {
        // Ray parallel to the triangle plane.
        return RayHit::Miss;
    }
    let inv_det = 1.0 / det;
    let tvec = sub(orig, tri[0]);
    let u = dot(tvec, pvec) * inv_det;
    if u < -EPS || u > 1.0 + EPS {
        return RayHit::Miss;
    }
    let qvec = cross(tvec, e1);
    let v = dot(d, qvec) * inv_det;
    if v < -EPS || u + v > 1.0 + EPS {
        return RayHit::Miss;
    }
    let t = dot(e2, qvec) * inv_det;
    if t <= EPS {
        return RayHit::Miss; // behind or at the origin
    }
    // Hit on an edge/vertex of the barycentric domain ⇒ ambiguous parity.
    if u < EPS || v < EPS || u + v > 1.0 - EPS {
        return RayHit::Grazing;
    }
    RayHit::Cross
}

/// Exact triangle–triangle intersection (Möller, "A Fast Triangle-Triangle
/// Intersection Test", 1997): the interval-overlap form on the line of the two
/// supporting planes, with coplanar triangles handled by a 2D edge/point test.
/// Pure-Rust; the narrow phase for `sf3dIntersects`.
fn tri_tri_intersect(t1: &[Coord3; 3], t2: &[Coord3; 3]) -> bool {
    const EPS: f64 = 1e-12;
    let n1 = cross(sub(t1[1], t1[0]), sub(t1[2], t1[0]));
    let d1 = -dot(n1, t1[0]);
    // signed distances of t2's vertices to t1's plane
    let dv2: [f64; 3] = [
        dot(n1, t2[0]) + d1,
        dot(n1, t2[1]) + d1,
        dot(n1, t2[2]) + d1,
    ];
    if dv2[0] > EPS && dv2[1] > EPS && dv2[2] > EPS {
        return false;
    }
    if dv2[0] < -EPS && dv2[1] < -EPS && dv2[2] < -EPS {
        return false;
    }

    let n2 = cross(sub(t2[1], t2[0]), sub(t2[2], t2[0]));
    let d2 = -dot(n2, t2[0]);
    let dv1: [f64; 3] = [
        dot(n2, t1[0]) + d2,
        dot(n2, t1[1]) + d2,
        dot(n2, t1[2]) + d2,
    ];
    if dv1[0] > EPS && dv1[1] > EPS && dv1[2] > EPS {
        return false;
    }
    if dv1[0] < -EPS && dv1[1] < -EPS && dv1[2] < -EPS {
        return false;
    }

    // Direction of the line of intersection of the two planes.
    let line_dir = cross(n1, n2);
    let line_len2 = dot(line_dir, line_dir);
    if line_len2 < EPS {
        // Planes are parallel; both pass the distance tests only when coplanar.
        return coplanar_tri_tri(t1, t2, n1);
    }
    // Project triangle vertices onto the line direction and form the
    // intersection interval each triangle makes with the line; overlap ⇒ hit.
    let i1 = tri_line_interval(t1, &dv1, line_dir);
    let i2 = tri_line_interval(t2, &dv2, line_dir);
    match (i1, i2) {
        (Some((a0, a1)), Some((b0, b1))) => {
            let (a0, a1) = (a0.min(a1), a0.max(a1));
            let (b0, b1) = (b0.min(b1), b0.max(b1));
            a0 <= b1 + EPS && b0 <= a1 + EPS
        }
        _ => false,
    }
}

/// The 1D interval (along `line_dir`) where a triangle crosses the other plane,
/// given each vertex's signed distance `dv`. Returns the two parameter values
/// where the two straddling edges pierce the plane. `None` if the triangle lies
/// entirely on one side (already excluded by the caller) or is degenerate.
fn tri_line_interval(
    tri: &[Coord3; 3],
    dv: &[f64; 3],
    line_dir: Coord3,
) -> Option<(f64, f64)> {
    let proj = |c: Coord3| dot(c, line_dir);
    let p = [proj(tri[0]), proj(tri[1]), proj(tri[2])];
    // Find the vertex on its own side of the plane; the other two straddle.
    let signs = [dv[0].signum(), dv[1].signum(), dv[2].signum()];
    let lone = if signs[0] != signs[1] && signs[0] != signs[2] {
        0
    } else if signs[1] != signs[0] && signs[1] != signs[2] {
        1
    } else {
        2
    };
    let (a, b, c) = ((lone + 1) % 3, (lone + 2) % 3, lone);
    let t_ca = isect_param(p[c], p[a], dv[c], dv[a])?;
    let t_cb = isect_param(p[c], p[b], dv[c], dv[b])?;
    Some((t_ca, t_cb))
}

/// Parameter on the line where the edge (with plane-distances `da`,`db` and
/// line-projections `pa`,`pb`) crosses the plane (`d == 0`).
fn isect_param(pa: f64, pb: f64, da: f64, db: f64) -> Option<f64> {
    let denom = da - db;
    if denom.abs() < 1e-15 {
        return None;
    }
    Some(pa + (pb - pa) * (da / denom))
}

/// Coplanar triangle/triangle overlap: project onto the dominant plane axis and
/// run a 2D test (edge crossings or a vertex of one inside the other).
fn coplanar_tri_tri(t1: &[Coord3; 3], t2: &[Coord3; 3], n: Coord3) -> bool {
    // Drop the axis with the largest |normal| component to project to 2D.
    let (ax, ay) = {
        let (nx, ny, nz) = (n.x.abs(), n.y.abs(), n.z.abs());
        if nx >= ny && nx >= nz {
            (1usize, 2usize)
        } else if ny >= nx && ny >= nz {
            (0usize, 2usize)
        } else {
            (0usize, 1usize)
        }
    };
    let xy = |c: Coord3| {
        let arr = [c.x, c.y, c.z];
        (arr[ax], arr[ay])
    };
    let a: [(f64, f64); 3] = [xy(t1[0]), xy(t1[1]), xy(t1[2])];
    let b: [(f64, f64); 3] = [xy(t2[0]), xy(t2[1]), xy(t2[2])];
    // Any edge crossing, or either triangle's vertex inside the other.
    for i in 0..3 {
        for j in 0..3 {
            if seg_seg_cross(a[i], a[(i + 1) % 3], b[j], b[(j + 1) % 3]) {
                return true;
            }
        }
    }
    pt_in_tri2(a[0], &b) || pt_in_tri2(b[0], &a)
}

fn seg_seg_cross(p1: (f64, f64), p2: (f64, f64), p3: (f64, f64), p4: (f64, f64)) -> bool {
    let d = |o: (f64, f64), a: (f64, f64), b: (f64, f64)| {
        (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
    };
    let d1 = d(p3, p4, p1);
    let d2 = d(p3, p4, p2);
    let d3 = d(p1, p2, p3);
    let d4 = d(p1, p2, p4);
    ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
}

fn pt_in_tri2(p: (f64, f64), t: &[(f64, f64); 3]) -> bool {
    let sign = |a: (f64, f64), b: (f64, f64), c: (f64, f64)| {
        (a.0 - c.0) * (b.1 - c.1) - (b.0 - c.0) * (a.1 - c.1)
    };
    let d1 = sign(p, t[0], t[1]);
    let d2 = sign(p, t[1], t[2]);
    let d3 = sign(p, t[2], t[0]);
    let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(has_neg && has_pos)
}

// ─── small Coord3 vector helpers (kept local to the narrow phase) ───

fn sub(a: Coord3, b: Coord3) -> Coord3 {
    Coord3::new(a.x - b.x, a.y - b.y, a.z - b.z)
}
fn cross(a: Coord3, b: Coord3) -> Coord3 {
    Coord3::new(
        a.y * b.z - a.z * b.y,
        a.z * b.x - a.x * b.z,
        a.x * b.y - a.y * b.x,
    )
}
fn dot(a: Coord3, b: Coord3) -> f64 {
    a.x * b.x + a.y * b.y + a.z * b.z
}

// ─── certified CSG via SFCGAL (sfcgal3d feature only) ───
//
// All of the following is compiled ONLY with `--features sfcgal3d`, which links
// the native libSFCGAL C library. The default build (and the integrator's
// `--features full`) excludes it, so these never affect that compile. The bodies
// below are a thin, plausible mapping onto the `sfcgal` crate's WKT-in/WKT-out
// surface; they are not exercised by the default test suite.

#[cfg(feature = "sfcgal3d")]
fn fn_union3d(args: &[Term]) -> Option<Term> {
    sfcgal_binary_op(args, SfcgalOp::Union)
}

#[cfg(feature = "sfcgal3d")]
fn fn_intersection3d(args: &[Term]) -> Option<Term> {
    sfcgal_binary_op(args, SfcgalOp::Intersection)
}

#[cfg(feature = "sfcgal3d")]
fn fn_difference3d(args: &[Term]) -> Option<Term> {
    sfcgal_binary_op(args, SfcgalOp::Difference)
}

#[cfg(feature = "sfcgal3d")]
enum SfcgalOp {
    Union,
    Intersection,
    Difference,
}

/// Run a certified boolean on the two WKT-Z solids via SFCGAL and return the
/// resulting solid as a `geo:wktLiteral`. Needs the native libSFCGAL C library.
#[cfg(feature = "sfcgal3d")]
fn sfcgal_binary_op(args: &[Term], op: SfcgalOp) -> Option<Term> {
    use sfcgal::SFCGeometry;

    // SFCGAL parses ISO-13249 WKT-Z directly; extract the body (CRS stripped).
    let wa = wkt_body(args.first()?)?;
    let wb = wkt_body(args.get(1)?)?;
    let ga = SFCGeometry::new(&wa).ok()?;
    let gb = SFCGeometry::new(&wb).ok()?;
    let result = match op {
        SfcgalOp::Union => ga.union(&gb),
        SfcgalOp::Intersection => ga.intersection(&gb),
        SfcgalOp::Difference => ga.difference(&gb),
    }
    .ok()?;
    let wkt = result.to_wkt().ok()?;
    Some(wkt_literal(wkt))
}

/// Certified exact volume of a closed solid via SFCGAL's exact arithmetic.
#[cfg(feature = "sfcgal3d")]
fn fn_volume_exact(args: &[Term]) -> Option<Term> {
    use sfcgal::SFCGeometry;
    let w = wkt_body(args.first()?)?;
    let g = SFCGeometry::new(&w).ok()?;
    let vol = g.volume().ok()?;
    Some(double_literal(vol))
}

/// Extract the raw WKT body (CRS prefix stripped) of a literal term, for SFCGAL.
#[cfg(feature = "sfcgal3d")]
fn wkt_body(term: &Term) -> Option<String> {
    match term {
        Term::Literal(l) => Some(extract_wkt(l.value()).to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wkt(s: &str) -> Term {
        Term::Literal(Literal::new_typed_literal(
            s,
            NamedNode::new_unchecked(vocab::WKT_LITERAL),
        ))
    }
    fn as_f64(t: Option<Term>) -> f64 {
        match t {
            Some(Term::Literal(l)) => l.value().parse().unwrap(),
            other => panic!("expected double, got {other:?}"),
        }
    }
    fn as_bool(t: Option<Term>) -> bool {
        match t {
            Some(Term::Literal(l)) => l.value() == "true",
            other => panic!("expected bool, got {other:?}"),
        }
    }

    const CUBE: &str = "POLYHEDRALSURFACE Z (\
        ((0 0 0,0 1 0,1 1 0,1 0 0,0 0 0)),\
        ((0 0 1,1 0 1,1 1 1,0 1 1,0 0 1)),\
        ((0 0 0,0 0 1,0 1 1,0 1 0,0 0 0)),\
        ((1 0 0,1 1 0,1 1 1,1 0 1,1 0 0)),\
        ((0 0 0,1 0 0,1 0 1,0 0 1,0 0 0)),\
        ((0 1 0,0 1 1,1 1 1,1 1 0,0 1 0)))";

    #[test]
    fn volume_and_area_of_cube() {
        assert!((as_f64(fn_volume(&[wkt(CUBE)])) - 1.0).abs() < 1e-9);
        assert!((as_f64(fn_area3d(&[wkt(CUBE)])) - 6.0).abs() < 1e-9);
        assert!((as_f64(fn_height(&[wkt(CUBE)])) - 1.0).abs() < 1e-9);
        assert!((as_f64(fn_zmax(&[wkt(CUBE)])) - 1.0).abs() < 1e-9);
        assert!((as_f64(fn_zmin(&[wkt(CUBE)])) - 0.0).abs() < 1e-9);
    }

    #[test]
    fn distance_between_two_points() {
        let d = as_f64(fn_distance3d(&[wkt("POINT Z (0 0 0)"), wkt("POINT Z (1 2 2)")]));
        assert!((d - 3.0).abs() < 1e-9);
    }

    #[test]
    fn intersects_and_disjoint() {
        let a = wkt("POINT Z (0.5 0.5 0.5)");
        assert!(as_bool(fn_sf3d_intersects(&[wkt(CUBE), a])));
        let far = wkt("POINT Z (100 100 100)");
        assert!(as_bool(fn_sf3d_disjoint(&[wkt(CUBE), far])));
    }

    #[test]
    fn extrude_polygon_volume() {
        let v = as_f64(fn_volume(&[fn_extrude(&[
            wkt("POLYGON Z ((0 0 0,2 0 0,2 3 0,0 3 0,0 0 0))"),
            Term::Literal(Literal::new_typed_literal(
                "5",
                NamedNode::new_unchecked("http://www.w3.org/2001/XMLSchema#double"),
            )),
        ])
        .unwrap()]));
        // 2 × 3 × 5 = 30
        assert!((v - 30.0).abs() < 1e-9, "vol {v}");
    }

    #[test]
    fn registry_has_all() {
        let fns = all_functions_3d();
        // Default (no sfcgal3d): 10 always-on + convexHull3d + intersects/disjoint
        // + contains/within = 15. The 4 SFCGAL CSG functions are only present in
        // an `sfcgal3d` build.
        #[cfg(not(feature = "sfcgal3d"))]
        assert_eq!(fns.len(), 15);
        #[cfg(feature = "sfcgal3d")]
        assert_eq!(fns.len(), 19);
        let iris: Vec<&str> = fns.iter().map(|(i, _)| i.as_str()).collect();
        assert!(iris.contains(&vocab::OTS3D_DISTANCE3D));
        assert!(iris.contains(&vocab::OTS3D_VOLUME));
        assert!(iris.contains(&vocab::OTS3D_CONVEXHULL3D));
        assert!(iris.contains(&vocab::OTS3D_SF_CONTAINS));
        assert!(iris.contains(&vocab::OTS3D_SF_WITHIN));
        assert!(iris.iter().all(|i| i.starts_with("https://open-triplestore.org/def/function/geo3d/")));
    }

    #[test]
    fn convex_hull_of_cube_corners_is_a_unit_volume_solid() {
        // Hull of the cube's 8 corners (here, all 24 face vertices) ⇒ the cube.
        let hull = fn_convex_hull3d(&[wkt(CUBE)]).expect("hull");
        let hull_wkt = match &hull {
            Term::Literal(l) => l.value().to_string(),
            other => panic!("expected wkt literal, got {other:?}"),
        };
        assert!(hull_wkt.starts_with("POLYHEDRALSURFACE Z"), "wkt {hull_wkt}");
        // The hull is a closed solid of unit volume (parry triangulates 6→12 faces).
        let v = as_f64(fn_volume(&[hull]));
        assert!((v - 1.0).abs() < 1e-6, "hull volume {v}");
    }

    #[test]
    fn convex_hull_needs_four_points() {
        // A single point has no 3D hull.
        assert!(fn_convex_hull3d(&[wkt("POINT Z (0 0 0)")]).is_none());
    }

    #[test]
    fn exact_contains_point_inside_vs_outside_cube() {
        let inside = wkt("POINT Z (0.5 0.5 0.5)");
        let outside = wkt("POINT Z (1.5 0.5 0.5)");
        // CUBE contains the interior point but not the exterior one.
        assert!(as_bool(fn_sf3d_contains(&[wkt(CUBE), inside])));
        assert!(!as_bool(fn_sf3d_contains(&[wkt(CUBE), outside])));
        // within is the inverse argument order.
        assert!(as_bool(fn_sf3d_within(&[wkt("POINT Z (0.5 0.5 0.5)"), wkt(CUBE)])));
        assert!(!as_bool(fn_sf3d_within(&[wkt("POINT Z (1.5 0.5 0.5)"), wkt(CUBE)])));
    }

    #[test]
    fn contains_a_corner_and_a_face_point() {
        // Boundary points (a corner and a face centre) count as contained.
        assert!(as_bool(fn_sf3d_contains(&[wkt(CUBE), wkt("POINT Z (0 0 0)")])));
        assert!(as_bool(fn_sf3d_contains(&[wkt(CUBE), wkt("POINT Z (0 0.5 0.5)")])));
    }

    #[test]
    fn contains_rejects_a_non_solid_first_arg() {
        // A bare point has no volume — contains is undefined (None ⇒ no term).
        assert!(fn_sf3d_contains(&[wkt("POINT Z (0 0 0)"), wkt("POINT Z (0 0 0)")]).is_none());
    }

    #[test]
    fn exact_intersects_overlapping_and_touching_cubes() {
        // A second unit cube shifted by 0.5 in x overlaps the first.
        let cube2 = "POLYHEDRALSURFACE Z (\
            ((0.5 0 0,0.5 1 0,1.5 1 0,1.5 0 0,0.5 0 0)),\
            ((0.5 0 1,1.5 0 1,1.5 1 1,0.5 1 1,0.5 0 1)),\
            ((0.5 0 0,0.5 0 1,0.5 1 1,0.5 1 0,0.5 0 0)),\
            ((1.5 0 0,1.5 1 0,1.5 1 1,1.5 0 1,1.5 0 0)),\
            ((0.5 0 0,1.5 0 0,1.5 0 1,0.5 0 1,0.5 0 0)),\
            ((0.5 1 0,0.5 1 1,1.5 1 1,1.5 1 0,0.5 1 0)))";
        assert!(as_bool(fn_sf3d_intersects(&[wkt(CUBE), wkt(cube2)])));
        assert!(!as_bool(fn_sf3d_disjoint(&[wkt(CUBE), wkt(cube2)])));
    }

    #[test]
    fn exact_intersects_separated_cubes_are_disjoint() {
        // A cube far away in x — AABB fast-reject returns disjoint.
        let cube2 = "POLYHEDRALSURFACE Z (\
            ((10 0 0,10 1 0,11 1 0,11 0 0,10 0 0)),\
            ((10 0 1,11 0 1,11 1 1,10 1 1,10 0 1)),\
            ((10 0 0,10 0 1,10 1 1,10 1 0,10 0 0)),\
            ((11 0 0,11 1 0,11 1 1,11 0 1,11 0 0)),\
            ((10 0 0,11 0 0,11 0 1,10 0 1,10 0 0)),\
            ((10 1 0,10 1 1,11 1 1,11 1 0,10 1 0)))";
        assert!(!as_bool(fn_sf3d_intersects(&[wkt(CUBE), wkt(cube2)])));
        assert!(as_bool(fn_sf3d_disjoint(&[wkt(CUBE), wkt(cube2)])));
    }
}
