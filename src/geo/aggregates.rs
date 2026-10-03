//! GeoSPARQL 1.1 spatial aggregates (Req 42): `geof:aggUnion`,
//! `aggBoundingBox`, `aggBoundingCircle`, `aggCentroid`, `aggConvexHull` and
//! `aggConcaveHull`.
//!
//! A custom aggregate is two registrations: the evaluator's
//! (`SparqlEvaluator::with_custom_aggregate_function`, done in
//! `TripleStore::query_options`, which declares the name to that evaluator's
//! parser too) and the *parser's* for every other place a query is parsed —
//! the accelerator's planners, the scoped and batch paths, the update ACL
//! check. Without it `geof:aggUnion(?g)` parses as a plain function call: a
//! different query, a row-local BIND instead of a group. [`register_with_parser`]
//! declares the aggregates to [`opengraph::sparql_parser`], which all of them use.
//!
//! Every aggregate follows SPARQL's aggregate error rules: a value that is not
//! a geometry (or an unbound one) makes the group's result unbound, as a
//! non-number does to `SUM`. The aggregate of no geometries is the empty
//! geometry — the identity of the union, as 0 is `SUM`'s. The values are sorted
//! before they are combined, so the result is the same whatever order the
//! solutions arrive in: the in-memory copy and the persistent store iterate
//! differently, and must still answer byte-identically. All of them harmonise a
//! group's CRSs and choose its result serialisation as `aggUnion` does
//! ([`union_of`]).
//!
//! `geof:aggConcaveHull` takes one argument: GeoSPARQL 1.1 gives it a second,
//! `targetPercent`, but the SPARQL parser (spargebra 0.4) allows one expression
//! in a custom aggregate call and the evaluator passes the accumulator one value.
//! It uses [`DEFAULT_CONCAVE_HULL_RATIO`], as `geof:concaveHull` does without a
//! ratio; `geof:concaveHull(geof:aggUnion(?g), r)` gives any other ratio.

use std::sync::{Arc, Once};

use geos::{Geom, Geometry as GeosGeometry};
use oxigraph::sparql::AggregateFunctionAccumulator;
use oxrdf::{Literal, NamedNode, Term};

use super::crs::Crs;
use super::datatypes::{
    geometry_to_literal, literal_crs, literal_crs_uri, parse_wkt_literal, reproject_geometry,
    Serialisation,
};
use super::functions::{concave_hull, minimum_bounding_circle, DEFAULT_CONCAVE_HULL_RATIO};
use super::vocabulary as vocab;

/// A factory for a fresh accumulator, as the evaluator wants it.
pub type AccumulatorFactory =
    Arc<dyn Fn() -> Box<dyn AggregateFunctionAccumulator + Send + Sync> + Send + Sync>;

/// Every GeoSPARQL aggregate as `(IRI, accumulator factory)`.
pub fn all_aggregates() -> Vec<(NamedNode, AccumulatorFactory)> {
    [
        (vocab::AGG_UNION, Op::Union),
        (vocab::AGG_BOUNDING_BOX, Op::BoundingBox),
        (vocab::AGG_BOUNDING_CIRCLE, Op::BoundingCircle),
        (vocab::AGG_CENTROID, Op::Centroid),
        (vocab::AGG_CONCAVE_HULL, Op::ConcaveHull),
        (vocab::AGG_CONVEX_HULL, Op::ConvexHull),
    ]
    .into_iter()
    .map(|(iri, op)| -> (NamedNode, AccumulatorFactory) {
        (
            NamedNode::new_unchecked(iri),
            Arc::new(
                move || -> Box<dyn AggregateFunctionAccumulator + Send + Sync> {
                    Box::new(Collect {
                        op,
                        values: Vec::new(),
                    })
                },
            ),
        )
    })
    .collect()
}

/// What an aggregate makes of its group's geometries, gathered into one
/// geometry collection.
#[derive(Debug, Clone, Copy)]
enum Op {
    /// Their union (GEOS unary union).
    Union,
    /// Their envelope.
    BoundingBox,
    /// Their minimum bounding circle (see `geof:boundingCircle`).
    BoundingCircle,
    /// Their centroid — of the collection, so each geometry counts as often as
    /// the group holds it, and only the highest dimension counts.
    Centroid,
    /// Their concave hull, at [`DEFAULT_CONCAVE_HULL_RATIO`].
    ConcaveHull,
    /// Their convex hull.
    ConvexHull,
}

impl Op {
    fn apply(self, collection: GeosGeometry) -> Option<GeosGeometry> {
        match self {
            Op::Union => collection.unary_union().ok(),
            Op::BoundingBox => collection.envelope().ok(),
            Op::BoundingCircle => minimum_bounding_circle(&collection),
            Op::Centroid => collection.get_centroid().ok(),
            Op::ConcaveHull => concave_hull(&collection, DEFAULT_CONCAVE_HULL_RATIO),
            Op::ConvexHull => collection.convex_hull().ok(),
        }
    }
}

/// Declare the aggregates to the shared SPARQL parser (idempotent, cheap after
/// the first call). Called when a store is built, before any query is parsed.
pub fn register_with_parser() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        for (iri, _) in all_aggregates() {
            opengraph::register_custom_aggregate(iri);
        }
    });
}

/// A spatial aggregate: collects the group's values, combines them in `finish`.
struct Collect {
    op: Op,
    values: Vec<Term>,
}

impl AggregateFunctionAccumulator for Collect {
    fn accumulate(&mut self, element: Term) {
        self.values.push(element);
    }

    fn finish(&mut self) -> Option<Term> {
        let values = std::mem::take(&mut self.values);
        match self.op {
            Op::Union => union_of(values),
            op => aggregate(values, op),
        }
    }
}

/// The empty geometry, as a WKT literal.
fn empty_geometry() -> Term {
    Term::Literal(Literal::new_typed_literal(
        "GEOMETRYCOLLECTION EMPTY",
        NamedNode::new_unchecked(vocab::WKT_LITERAL),
    ))
}

/// The union of a group's geometry literals as one geometry literal: of the
/// group's serialisation when every value shares one (a group of GML literals
/// unions into a GML literal), a `geo:wktLiteral` otherwise.
///
/// One CRS in, the same CRS out — a GML literal's `srsName` included (see
/// [`literal_crs_uri`]). Values in different CRSs are unioned in CRS84
/// (GeoSPARQL's default), each reprojected first; a value in a CRS this build
/// cannot reproject, or outside its CRS's domain, then makes the result unbound.
pub fn union_of(values: Vec<Term>) -> Option<Term> {
    aggregate(values, Op::Union)
}

/// A group's geometry literals, harmonised into one CRS as [`union_of`]
/// describes, gathered into one collection and combined by `op`.
fn aggregate(mut values: Vec<Term>, op: Op) -> Option<Term> {
    if values.is_empty() {
        return Some(empty_geometry());
    }
    // Order-independence: sort by lexical form, then datatype.
    let key = |t: &Term| match t {
        Term::Literal(l) => (l.value().to_string(), l.datatype().as_str().to_string()),
        other => (other.to_string(), String::new()),
    };
    values.sort_by_cached_key(key);

    let parse_all = |values: &[Term]| -> Option<Vec<GeosGeometry>> {
        values.iter().map(parse_wkt_literal).collect()
    };
    let first_uri = literal_crs_uri(&values[0]).map(|uri| uri.into_owned());
    let (geoms, crs_out) = if values
        .iter()
        .all(|v| literal_crs_uri(v).as_deref() == first_uri.as_deref())
    {
        // One CRS: nothing to reproject — which also keeps a CRS this build
        // does not know usable, as long as every value shares it.
        (parse_all(&values)?, first_uri)
    } else {
        let crss = values
            .iter()
            .map(literal_crs)
            .collect::<Option<Vec<Crs>>>()?;
        if crss.iter().all(|c| *c == crss[0]) {
            // One CRS spelled two ways (`EPSG/0/28992`, `EPSG::28992`).
            let uri = (crss[0] != Crs::Wgs84).then(|| crss[0].to_uri().to_string());
            (parse_all(&values)?, uri)
        } else {
            let geoms = values.iter().map(in_crs84).collect::<Option<Vec<_>>>()?;
            (geoms, None)
        }
    };
    let result = op.apply(GeosGeometry::create_geometry_collection(geoms).ok()?)?;
    let serialisation = Serialisation::of(&values[0]);
    let shared = values.iter().all(|v| Serialisation::of(v) == serialisation);
    let serialisation = if shared {
        serialisation
    } else {
        Serialisation::Wkt
    };
    geometry_to_literal(&result, serialisation, crs_out.as_deref())
}

/// A geometry literal reprojected into CRS84, as GEOS.
fn in_crs84(term: &Term) -> Option<GeosGeometry> {
    reproject_geometry(&parse_wkt_literal(term)?, literal_crs(term)?, Crs::Wgs84)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wkt(v: &str) -> Term {
        Term::Literal(Literal::new_typed_literal(
            v,
            NamedNode::new_unchecked(vocab::WKT_LITERAL),
        ))
    }

    fn area(t: &Term) -> f64 {
        parse_wkt_literal(t).unwrap().area().unwrap()
    }

    #[test]
    fn overlapping_squares_union_once() {
        let u = union_of(vec![
            wkt("POLYGON((0 0, 2 0, 2 2, 0 2, 0 0))"),
            wkt("POLYGON((1 0, 3 0, 3 2, 1 2, 1 0))"),
        ])
        .unwrap();
        assert!((area(&u) - 6.0).abs() < 1e-12);
    }

    #[test]
    fn the_result_does_not_depend_on_the_order() {
        let a = wkt("POLYGON((0 0, 2 0, 2 2, 0 2, 0 0))");
        let b = wkt("POLYGON((1 1, 3 1, 3 3, 1 3, 1 1))");
        let c = wkt("POINT(10 10)");
        let one = union_of(vec![a.clone(), b.clone(), c.clone()]);
        let two = union_of(vec![c, b, a]);
        assert_eq!(one, two);
    }

    #[test]
    fn no_values_is_the_empty_geometry() {
        assert_eq!(union_of(Vec::new()), Some(empty_geometry()));
    }

    #[test]
    fn a_value_that_is_not_a_geometry_fails_the_group() {
        let not_geometry = Term::Literal(Literal::new_simple_literal("hello"));
        assert_eq!(
            union_of(vec![wkt("POINT(0 0)"), not_geometry]),
            None,
            "a non-geometry must make the union unbound"
        );
        assert_eq!(union_of(vec![wkt("POLYGON((bad))")]), None);
    }

    #[test]
    fn every_aggregate_is_registered() {
        let iris: Vec<String> = all_aggregates()
            .into_iter()
            .map(|(iri, _)| iri.as_str().to_string())
            .collect();
        for iri in [
            vocab::AGG_UNION,
            vocab::AGG_BOUNDING_BOX,
            vocab::AGG_BOUNDING_CIRCLE,
            vocab::AGG_CENTROID,
            vocab::AGG_CONCAVE_HULL,
            vocab::AGG_CONVEX_HULL,
        ] {
            assert!(iris.contains(&iri.to_string()), "{iri}");
        }
    }

    #[test]
    fn bounding_box_centroid_and_hulls_of_a_group() {
        let square = vec![
            wkt("POINT(0 0)"),
            wkt("POINT(2 0)"),
            wkt("POINT(2 2)"),
            wkt("POINT(0 2)"),
        ];
        let bbox = aggregate(square.clone(), Op::BoundingBox).unwrap();
        assert_eq!(area(&bbox), 4.0);
        let hull = aggregate(square.clone(), Op::ConvexHull).unwrap();
        assert_eq!(area(&hull), 4.0);
        let concave = aggregate(square.clone(), Op::ConcaveHull).unwrap();
        assert!(area(&concave) <= 4.0 + 1e-12);
        let centroid =
            parse_wkt_literal(&aggregate(square.clone(), Op::Centroid).unwrap()).unwrap();
        assert_eq!(
            (centroid.get_x().unwrap(), centroid.get_y().unwrap()),
            (1.0, 1.0)
        );
        let circle = parse_wkt_literal(&aggregate(square, Op::BoundingCircle).unwrap()).unwrap();
        let corner = parse_wkt_literal(&wkt("POINT(2 2)")).unwrap();
        assert!(
            circle.covers(&corner).unwrap(),
            "the circle covers every point"
        );
    }

    #[test]
    fn the_accumulator_resets_after_finish() {
        let mut acc = Collect {
            op: Op::Union,
            values: Vec::new(),
        };
        acc.accumulate(wkt("POINT(1 1)"));
        assert!(acc.finish().is_some());
        assert_eq!(acc.finish(), Some(empty_geometry()));
    }
}
