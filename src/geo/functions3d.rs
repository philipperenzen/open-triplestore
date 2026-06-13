//! The `ots-geof:` 3D function surface (spec §3.4) — additive volumetric
//! predicates and measures registered beside the GeoSPARQL 1.1 `geof:` surface.
//!
//! Namespace: `https://open-triplestore.org/def/function/geo3d/`. These never
//! touch the 2D GEOS path, so GeoSPARQL 1.1 stays byte-for-byte conformant.
//! Each function parses ISO-13249 WKT-Z literals via [`super::geom3d`] and
//! returns an `xsd:double` / `xsd:boolean` / `geo:wktLiteral` term.
//!
//! This baseline is pure-Rust: metric (`distance3d`, `volume`, `area3d`,
//! `zMin/zMax/height`), constructive (`boundingBox3d`, `centroid3d`,
//! `footprint2d`, `extrude`) and AABB-broad-phase topology (`sf3dIntersects`,
//! `sf3dDisjoint`). Exact solid topology (`sf3dContains`/`Within`/…) and the
//! constructive booleans (`union3d`/`intersection3d`/`difference3d`,
//! `convexHull3d`, certified `volume`) arrive with the `parry3d`/SFCGAL kernel.

use std::sync::Arc;

use oxrdf::{Literal, NamedNode, Term};

use super::datatypes::{boolean_literal, double_literal, extract_wkt};
use super::geom3d::{self, Geometry3D};
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
        // ─── 3D topological (broad-phase) ───
        make_fn(vocab::OTS3D_SF_INTERSECTS, fn_sf3d_intersects),
        make_fn(vocab::OTS3D_SF_DISJOINT, fn_sf3d_disjoint),
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

// ─── topological (broad phase) ───

fn fn_sf3d_intersects(args: &[Term]) -> Option<Term> {
    let (a, b) = parse_two(args)?;
    let (ba, bb) = (a.aabb()?, b.aabb()?);
    Some(boolean_literal(ba.overlaps(&bb)))
}

fn fn_sf3d_disjoint(args: &[Term]) -> Option<Term> {
    let (a, b) = parse_two(args)?;
    let (ba, bb) = (a.aabb()?, b.aabb()?);
    Some(boolean_literal(!ba.overlaps(&bb)))
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
        assert_eq!(fns.len(), 12);
        let iris: Vec<&str> = fns.iter().map(|(i, _)| i.as_str()).collect();
        assert!(iris.contains(&vocab::OTS3D_DISTANCE3D));
        assert!(iris.contains(&vocab::OTS3D_VOLUME));
        assert!(iris.iter().all(|i| i.starts_with("https://open-triplestore.org/def/function/geo3d/")));
    }
}
