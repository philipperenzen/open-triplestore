//! Cost-based SPARQL BGP optimizer.
//!
//! Rewrites incoming SPARQL queries by reordering triple patterns in Basic
//! Graph Patterns (BGPs) according to a selectivity model:
//!
//! | Pattern type | Selectivity score (lower = more selective) |
//! |---|---|
//! | `<S> <P> <O>` — fully ground | 0 (execute first) |
//! | `<S> <P> ?o` — bound subject + predicate | 1 |
//! | `?s <P> <O>` — bound predicate + object | 2 |
//! | `<S> ?p <O>` — bound subject + object | 3 |
//! | `<S> ?p ?o` — bound subject only | 4 |
//! | `?s <P> ?o` — bound predicate only | 5 |
//! | `?s ?p <O>` — bound object only | 6 |
//! | `?s ?p ?o` — fully unbound | 7 (execute last) |
//!
//! A second pass propagates variable bindings: after a pattern is placed, any
//! variable it binds is treated as "known" for subsequent patterns, which
//! improves the ordering further.
//!
//! # Usage
//!
//! ```rust
//! use opengraph::optimizer::QueryOptimizer;
//!
//! let sparql = "SELECT ?name ?age WHERE { ?p <:age> ?age . ?p <:name> ?name . <:alice> <:knows> ?p . }";
//! let optimized = QueryOptimizer::reorder_bgp(sparql);
//! // The fully-bound pattern <:alice> <:knows> ?p is moved first.
//! assert!(optimized.contains("alice"));
//! ```

use spargebra::algebra::GraphPattern;
use spargebra::term::{NamedNodePattern, TermPattern, TriplePattern};
use spargebra::Query;
use tracing::debug;

/// Selectivity score for a triple pattern given a set of already-bound variables.
fn selectivity(tp: &TriplePattern, bound: &std::collections::HashSet<String>) -> u8 {
    let s_bound = match &tp.subject {
        TermPattern::NamedNode(_) | TermPattern::BlankNode(_) | TermPattern::Literal(_) => true,
        TermPattern::Variable(v) => bound.contains(v.as_str()),
        #[cfg(feature = "sparql-12")]
        TermPattern::Triple(_) => false,
    };
    let p_bound = match &tp.predicate {
        NamedNodePattern::NamedNode(_) => true,
        NamedNodePattern::Variable(v) => bound.contains(v.as_str()),
    };
    let o_bound = match &tp.object {
        TermPattern::NamedNode(_) | TermPattern::BlankNode(_) => true,
        #[cfg(feature = "sparql-12")]
        TermPattern::Triple(_) => true,
        TermPattern::Variable(v) => bound.contains(v.as_str()),
        _ => false,
    };

    match (s_bound, p_bound, o_bound) {
        (true, true, true) => 0,
        (true, true, false) => 1,
        (false, true, true) => 2,
        (true, false, true) => 3,
        (true, false, false) => 4,
        (false, true, false) => 5,
        (false, false, true) => 6,
        (false, false, false) => 7,
    }
}

/// Extract variables bound by a triple pattern.
fn bound_by(tp: &TriplePattern) -> Vec<String> {
    let mut vars = Vec::new();
    if let TermPattern::Variable(v) = &tp.subject {
        vars.push(v.as_str().to_string());
    }
    if let NamedNodePattern::Variable(v) = &tp.predicate {
        vars.push(v.as_str().to_string());
    }
    if let TermPattern::Variable(v) = &tp.object {
        vars.push(v.as_str().to_string());
    }
    vars
}

/// Reorder triple patterns in a BGP using a greedy variable-propagation algorithm.
///
/// At each step, pick the remaining pattern with the lowest selectivity score
/// given the current set of bound variables, then add its variables to the bound set.
fn reorder_patterns(mut patterns: Vec<TriplePattern>) -> Vec<TriplePattern> {
    let mut bound: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut ordered = Vec::with_capacity(patterns.len());

    while !patterns.is_empty() {
        let best_idx = patterns
            .iter()
            .enumerate()
            .min_by_key(|(_, tp)| selectivity(tp, &bound))
            .map(|(i, _)| i)
            .unwrap();

        let tp = patterns.remove(best_idx);
        for v in bound_by(&tp) {
            bound.insert(v);
        }
        ordered.push(tp);
    }
    ordered
}

/// Recursively reorder all BGPs in a `GraphPattern`.
fn optimize_pattern(pattern: GraphPattern) -> GraphPattern {
    match pattern {
        GraphPattern::Bgp { patterns } => {
            let reordered = reorder_patterns(patterns);
            GraphPattern::Bgp {
                patterns: reordered,
            }
        }
        GraphPattern::Join { left, right } => GraphPattern::Join {
            left: Box::new(optimize_pattern(*left)),
            right: Box::new(optimize_pattern(*right)),
        },
        GraphPattern::LeftJoin {
            left,
            right,
            expression,
        } => GraphPattern::LeftJoin {
            left: Box::new(optimize_pattern(*left)),
            right: Box::new(optimize_pattern(*right)),
            expression,
        },
        GraphPattern::Filter { expr, inner } => GraphPattern::Filter {
            expr,
            inner: Box::new(optimize_pattern(*inner)),
        },
        GraphPattern::Union { left, right } => GraphPattern::Union {
            left: Box::new(optimize_pattern(*left)),
            right: Box::new(optimize_pattern(*right)),
        },
        GraphPattern::Graph { name, inner } => GraphPattern::Graph {
            name,
            inner: Box::new(optimize_pattern(*inner)),
        },
        GraphPattern::Extend {
            inner,
            variable,
            expression,
        } => GraphPattern::Extend {
            inner: Box::new(optimize_pattern(*inner)),
            variable,
            expression,
        },
        GraphPattern::Minus { left, right } => GraphPattern::Minus {
            left: Box::new(optimize_pattern(*left)),
            right: Box::new(optimize_pattern(*right)),
        },
        GraphPattern::OrderBy { inner, expression } => GraphPattern::OrderBy {
            inner: Box::new(optimize_pattern(*inner)),
            expression,
        },
        GraphPattern::Project { inner, variables } => GraphPattern::Project {
            inner: Box::new(optimize_pattern(*inner)),
            variables,
        },
        GraphPattern::Distinct { inner } => GraphPattern::Distinct {
            inner: Box::new(optimize_pattern(*inner)),
        },
        GraphPattern::Reduced { inner } => GraphPattern::Reduced {
            inner: Box::new(optimize_pattern(*inner)),
        },
        GraphPattern::Slice {
            inner,
            start,
            length,
        } => GraphPattern::Slice {
            inner: Box::new(optimize_pattern(*inner)),
            start,
            length,
        },
        GraphPattern::Group {
            inner,
            variables,
            aggregates,
        } => GraphPattern::Group {
            inner: Box::new(optimize_pattern(*inner)),
            variables,
            aggregates,
        },
        other => other,
    }
}

/// Cost-based SPARQL query optimizer.
pub struct QueryOptimizer;

impl QueryOptimizer {
    /// Reorder BGP triple patterns in a SPARQL query for optimal execution.
    ///
    /// Returns the rewritten SPARQL string.  If parsing fails (e.g. UPDATE
    /// statements), the original query is returned unchanged.
    pub fn reorder_bgp(sparql: &str) -> String {
        let query = match spargebra::SparqlParser::new().parse_query(sparql) {
            Ok(q) => q,
            Err(_) => return sparql.to_string(),
        };

        let rewritten = match query {
            Query::Select {
                pattern,
                dataset,
                base_iri,
            } => {
                let optimized = optimize_pattern(pattern);
                debug!("BGP reordering applied to SELECT query");
                Query::Select {
                    pattern: optimized,
                    dataset,
                    base_iri,
                }
            }
            Query::Construct {
                template,
                pattern,
                dataset,
                base_iri,
            } => {
                let optimized = optimize_pattern(pattern);
                Query::Construct {
                    template,
                    pattern: optimized,
                    dataset,
                    base_iri,
                }
            }
            Query::Ask {
                pattern,
                dataset,
                base_iri,
            } => {
                let optimized = optimize_pattern(pattern);
                Query::Ask {
                    pattern: optimized,
                    dataset,
                    base_iri,
                }
            }
            other => other,
        };

        rewritten.to_string()
    }

    /// Estimate the cardinality of a triple pattern (number of matching triples).
    ///
    /// This is a lightweight heuristic based on pattern boundedness.  For a
    /// production optimizer, replace this with RocksDB prefix key counts:
    ///
    /// ```text
    /// store.quads_for_pattern(None, Some(pred), None, None).count()
    /// ```
    ///
    /// Returns a score where lower = more selective (fewer expected results).
    pub fn estimate_cardinality(
        s_bound: bool,
        p_bound: bool,
        o_bound: bool,
        total_triples: usize,
    ) -> usize {
        let factor: f64 = match (s_bound, p_bound, o_bound) {
            (true, true, true) => 0.0,
            (true, true, false) => 0.001,
            (false, true, true) => 0.001,
            (true, false, true) => 0.01,
            (true, false, false) => 0.1,
            (false, true, false) => 0.05,
            (false, false, true) => 0.3,
            (false, false, false) => 1.0,
        };
        ((total_triples as f64) * factor) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reorder_moves_bound_first() {
        // The fully-bound pattern <:alice> <:knows> ?p should move before ?p <:age> ?age
        let sparql = "SELECT ?name ?age WHERE { \
            ?p <http://example.org/age> ?age . \
            ?p <http://example.org/name> ?name . \
            <http://example.org/alice> <http://example.org/knows> ?p . }";
        let optimized = QueryOptimizer::reorder_bgp(sparql);
        // After reordering, alice/knows should appear before age and name
        let alice_pos = optimized.find("alice").unwrap_or(usize::MAX);
        let age_pos = optimized.find("age>").unwrap_or(usize::MAX);
        assert!(
            alice_pos < age_pos,
            "Bound pattern should appear before unbound ones"
        );
    }

    #[test]
    fn test_reorder_passthrough_on_update() {
        let update = "INSERT DATA { <s:s> <p:p> <o:o> }";
        let result = QueryOptimizer::reorder_bgp(update);
        assert_eq!(
            result, update,
            "UPDATE statements should be returned unchanged"
        );
    }

    #[test]
    fn test_cardinality_estimates() {
        let n = 1_000_000usize;
        assert_eq!(QueryOptimizer::estimate_cardinality(true, true, true, n), 0);
        assert!(QueryOptimizer::estimate_cardinality(false, false, false, n) > 100_000);
        assert!(
            QueryOptimizer::estimate_cardinality(true, true, false, n)
                < QueryOptimizer::estimate_cardinality(false, true, false, n)
        );
    }

    #[test]
    fn test_selectivity_scoring() {
        use std::collections::HashSet;
        let bound: HashSet<String> = HashSet::new();
        let bound_with_x: HashSet<String> = ["x".to_string()].into();

        // Build a triple pattern ?x <p> ?y
        let tp = TriplePattern {
            subject: TermPattern::Variable(spargebra::term::Variable::new("x").unwrap()),
            predicate: NamedNodePattern::NamedNode(
                oxrdf::NamedNode::new("http://example.org/p").unwrap(),
            ),
            object: TermPattern::Variable(spargebra::term::Variable::new("y").unwrap()),
        };

        // With no bound vars: only predicate bound → score 5
        assert_eq!(selectivity(&tp, &bound), 5);
        // With x bound: subject+predicate bound → score 1
        assert_eq!(selectivity(&tp, &bound_with_x), 1);
    }

    #[test]
    fn test_empty_bgp() {
        let sparql = "SELECT * WHERE {}";
        let result = QueryOptimizer::reorder_bgp(sparql);
        // Should not panic on empty BGP
        assert!(result.contains("SELECT") || result.contains("select"));
    }
}
