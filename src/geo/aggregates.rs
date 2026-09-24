//! GeoSPARQL 1.1 spatial aggregates — `geof:aggUnion`.
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
//! `geof:aggUnion` follows SPARQL's aggregate error rules: a value that is not
//! a geometry (or an unbound one) makes the group's result unbound, as a
//! non-number does to `SUM`. The union of no geometries is the empty geometry —
//! the identity of the union, as 0 is `SUM`'s. The values are sorted before the
//! union, so the result is the same whatever order the solutions arrive in: the
//! in-memory copy and the persistent store iterate differently, and must still
//! answer byte-identically.

use std::sync::{Arc, Once};

use geos::{Geom, Geometry as GeosGeometry};
use oxigraph::sparql::AggregateFunctionAccumulator;
use oxrdf::{Literal, NamedNode, Term};

use super::crs::Crs;
use super::datatypes::{
    geometry_to_wkt_literal_in, literal_crs_uri, literal_wkt, parse_wkt_literal,
};
use super::geodesic::{literal_crs, reproject};
use super::vocabulary as vocab;

/// A factory for a fresh accumulator, as the evaluator wants it.
pub type AccumulatorFactory =
    Arc<dyn Fn() -> Box<dyn AggregateFunctionAccumulator + Send + Sync> + Send + Sync>;

/// Every GeoSPARQL aggregate as `(IRI, accumulator factory)`.
pub fn all_aggregates() -> Vec<(NamedNode, AccumulatorFactory)> {
    vec![(
        NamedNode::new_unchecked(vocab::AGG_UNION),
        Arc::new(|| -> Box<dyn AggregateFunctionAccumulator + Send + Sync> {
            Box::new(AggUnion::default())
        }),
    )]
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

/// `geof:aggUnion`: collects the group's values, unions them in `finish`.
#[derive(Default)]
struct AggUnion {
    values: Vec<Term>,
}

impl AggregateFunctionAccumulator for AggUnion {
    fn accumulate(&mut self, element: Term) {
        self.values.push(element);
    }

    fn finish(&mut self) -> Option<Term> {
        union_of(std::mem::take(&mut self.values))
    }
}

/// The empty geometry, as a WKT literal.
fn empty_geometry() -> Term {
    Term::Literal(Literal::new_typed_literal(
        "GEOMETRYCOLLECTION EMPTY",
        NamedNode::new_unchecked(vocab::WKT_LITERAL),
    ))
}

/// The union of a group's geometry literals as one `geo:wktLiteral`.
///
/// One CRS in, the same CRS out. Values in different CRSs are unioned in CRS84
/// (GeoSPARQL's default), each reprojected first; a value in a CRS this build
/// cannot reproject then makes the result unbound.
pub fn union_of(mut values: Vec<Term>) -> Option<Term> {
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
    let first_uri = literal_crs_uri(&values[0]).map(str::to_string);
    let (geoms, crs_out) = if values
        .iter()
        .all(|v| literal_crs_uri(v) == first_uri.as_deref())
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
    let union = GeosGeometry::create_geometry_collection(geoms)
        .ok()?
        .unary_union()
        .ok()?;
    geometry_to_wkt_literal_in(&union, crs_out.as_deref())
}

/// A geometry literal reprojected into CRS84, as GEOS.
fn in_crs84(term: &Term) -> Option<GeosGeometry> {
    use wkt::{ToWkt, TryFromWkt};
    match literal_crs(term)? {
        Crs::Wgs84 => parse_wkt_literal(term),
        from => {
            let body = literal_wkt(term)?;
            let g: geo::Geometry<f64> = geo::Geometry::try_from_wkt_str(&body).ok()?;
            GeosGeometry::new_from_wkt(&reproject(&g, from, Crs::Wgs84)?.wkt_string()).ok()
        }
    }
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
    fn the_accumulator_resets_after_finish() {
        let mut acc = AggUnion::default();
        acc.accumulate(wkt("POINT(1 1)"));
        assert!(acc.finish().is_some());
        assert_eq!(acc.finish(), Some(empty_geometry()));
    }
}
