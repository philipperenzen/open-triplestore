//! The SPARQL parser every query path shares.
//!
//! A custom **aggregate** has to be declared to the parser, not only to the
//! evaluator: undeclared, `<iri>(?x)` parses as an ordinary function call — a
//! different query, a row-local `BIND` where the engine sees a group. The
//! accelerators in [`crate::parallel`] and [`crate::columnar`] decide from the
//! syntax tree whether they may answer a query, so they must read it the way the
//! engine will: misread as a `BIND`, the sub-select in
//! `SELECT (COUNT(*) AS ?n) { { SELECT (<agg>(?x) AS ?u) { ?s ?p ?x } } }` is
//! shard-local, and its `COUNT` would be summed over the shards — one row per
//! shard instead of one.
//!
//! The embedding application registers its aggregates once, with the same
//! IRIs it gives `SparqlEvaluator::with_custom_aggregate_function`, before it
//! parses anything; every [`sparql_parser`] after that declares them.

use std::sync::RwLock;

use oxrdf::NamedNode;
use spargebra::SparqlParser;

static CUSTOM_AGGREGATES: RwLock<Vec<NamedNode>> = RwLock::new(Vec::new());

/// Declare a custom aggregate function to every parser built from now on.
/// Registering the same IRI twice is a no-op.
pub fn register_custom_aggregate(name: NamedNode) {
    let mut names = CUSTOM_AGGREGATES
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !names.contains(&name) {
        names.push(name);
    }
}

/// The custom aggregates registered so far.
pub fn custom_aggregates() -> Vec<NamedNode> {
    CUSTOM_AGGREGATES
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// A SPARQL parser that knows every registered custom aggregate.
pub fn sparql_parser() -> SparqlParser {
    CUSTOM_AGGREGATES
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .cloned()
        .fold(SparqlParser::new(), |parser, name| {
            parser.with_custom_aggregate_function(name)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use spargebra::algebra::{AggregateExpression, AggregateFunction, GraphPattern};
    use spargebra::Query;

    const AGG: &str = "http://example.org/test/parser/agg";

    fn aggregates_of(q: &Query) -> Vec<AggregateExpression> {
        fn walk(p: &GraphPattern, out: &mut Vec<AggregateExpression>) {
            match p {
                GraphPattern::Group {
                    inner, aggregates, ..
                } => {
                    out.extend(aggregates.iter().map(|(_, a)| a.clone()));
                    walk(inner, out);
                }
                GraphPattern::Project { inner, .. }
                | GraphPattern::Extend { inner, .. }
                | GraphPattern::Filter { inner, .. } => walk(inner, out),
                _ => {}
            }
        }
        let Query::Select { pattern, .. } = q else {
            return Vec::new();
        };
        let mut out = Vec::new();
        walk(pattern, &mut out);
        out
    }

    #[test]
    fn a_registered_aggregate_parses_as_an_aggregate() {
        let q = format!("SELECT (<{AGG}>(?x) AS ?u) WHERE {{ ?s ?p ?x }}");
        // Undeclared: a function call, and no group at all.
        let plain = SparqlParser::new().parse_query(&q).unwrap();
        assert!(aggregates_of(&plain).is_empty());
        register_custom_aggregate(NamedNode::new_unchecked(AGG));
        register_custom_aggregate(NamedNode::new_unchecked(AGG));
        assert_eq!(
            custom_aggregates()
                .iter()
                .filter(|n| n.as_str() == AGG)
                .count(),
            1,
            "registration is idempotent"
        );
        let parsed = sparql_parser().parse_query(&q).unwrap();
        assert!(matches!(
            aggregates_of(&parsed).as_slice(),
            [AggregateExpression::FunctionCall {
                name: AggregateFunction::Custom(n),
                ..
            }] if n.as_str() == AGG
        ));
        // …and under GROUP BY, where the plain parser rejects the query.
        let grouped = format!("SELECT ?s (<{AGG}>(?x) AS ?u) WHERE {{ ?s ?p ?x }} GROUP BY ?s");
        assert!(SparqlParser::new().parse_query(&grouped).is_err());
        assert!(sparql_parser().parse_query(&grouped).is_ok());
    }
}
