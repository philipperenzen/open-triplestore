//! A columnar, dictionary-encoded copy of the store with its own SPARQL
//! evaluator (P5).
//!
//! The in-memory oxigraph copies the mirror keeps are the same engine on
//! the same key-encoded storage, only in RAM; what a columnar engine such as
//! QLever does better is the representation: terms as dense ids, quads as
//! sorted permutations of `u32`s, a triple pattern as a binary search and a
//! join as a walk over sorted ranges, with terms decoded only for the rows
//! that reach an expression or the output. This module is that
//! representation and an evaluator of the SPARQL algebra over it —
//! [`index::Columnar`] and [`eval`] — behind one rule inherited from the
//! mirror: **decline rather than differ**. A query using anything the
//! evaluator does not implement is declined before evaluation
//! ([`accepts`]) and the engine answers it, so an answer from this path is
//! the answer the engine would give, as a set of solutions.
//!
//! Implemented: basic graph patterns over the default graph, `FROM` unions
//! and `GRAPH` (constant or variable); joins, `OPTIONAL` (with its filter),
//! `UNION`, `MINUS`, `FILTER`, `BIND`, `VALUES`; fixed-length property
//! paths (`^`, `/`, `|`); `GROUP BY` with `COUNT`, `SUM`, `AVG`, `MIN`,
//! `MAX`, `GROUP_CONCAT`, `SAMPLE` (and `DISTINCT` in them); `ORDER BY`,
//! `DISTINCT`, `LIMIT`/`OFFSET`, subqueries; `SELECT`, `ASK`, `CONSTRUCT`
//! (templates without blank nodes); the SPARQL expression language with
//! numeric promotion, string, boolean and dateTime comparisons and the
//! string, numeric, dateTime and type-test functions, `REGEX` and `REPLACE`.
//! Declined: unbounded and negated property paths, `EXISTS`, `SERVICE`,
//! `LATERAL`, `DESCRIBE`, quoted triples, hash and UUID functions, `NOW`,
//! `RAND`, `BNODE`, casts and custom functions, custom aggregates.

pub mod eval;
pub mod index;
pub mod value;

pub use eval::{accepts, evaluate, evaluate_semantics};
pub use index::{Columnar, Dictionary, DEFAULT_GRAPH};

use crate::parallel::ParAnswer;

impl Columnar {
    /// Evaluate `sparql`. `Ok(None)` when the query is declined (the caller
    /// evaluates elsewhere); `Err` on a parse or evaluation failure.
    /// Evaluate `sparql` ignoring the cost-based declines — the shapes the
    /// engine simply answers faster. Those are a routing policy, not a limit
    /// on what this evaluator can answer, so the parity suite goes through
    /// here and holds it to the engine's answer for them too.
    pub fn query_semantics(&self, sparql: &str) -> Result<Option<ParAnswer>, String> {
        if uses_reserved_names(sparql) {
            return Ok(None);
        }
        let query = crate::sparql_parser()
            .parse_query(sparql)
            .map_err(|e| e.to_string())?;
        evaluate_semantics(self, &query)
    }

    pub fn query(&self, sparql: &str) -> Result<Option<ParAnswer>, String> {
        if uses_reserved_names(sparql) {
            return Ok(None);
        }
        let query = crate::sparql_parser()
            .parse_query(sparql)
            .map_err(|e| e.to_string())?;
        evaluate(self, &query)
    }
}

/// Whether `sparql` parses and uses only what the evaluator implements — a
/// parse and a walk, no data touched.
pub fn accepts_text(sparql: &str) -> bool {
    if uses_reserved_names(sparql) {
        return false;
    }
    crate::sparql_parser()
        .parse_query(sparql)
        .ok()
        .map(|q| accepts(&q).is_ok())
        .unwrap_or(false)
}

/// The evaluator names the variables it invents for blank-node patterns
/// `__og_bnode_*`, and strips those columns before returning. A query that
/// used a variable of its own by that name would lose it, so such a query is
/// declined rather than answered.
pub fn uses_reserved_names(sparql: &str) -> bool {
    sparql.contains("__og_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reserved_variable_namespace_is_declined() {
        assert!(!accepts_text(
            "SELECT ?__og_bnode_x WHERE { ?__og_bnode_x ?p ?o }"
        ));
        assert!(accepts_text("SELECT ?s WHERE { ?s ?p ?o }"));
    }

    /// The evaluator has no custom aggregates: a query using a registered one
    /// is declined (and reaches the engine), with or without GROUP BY.
    #[test]
    fn a_registered_custom_aggregate_is_declined() {
        let agg = "http://example.org/test/columnar/agg";
        crate::register_custom_aggregate(oxrdf::NamedNode::new_unchecked(agg));
        assert!(!accepts_text(&format!(
            "SELECT (<{agg}>(?o) AS ?u) WHERE {{ ?s ?p ?o }}"
        )));
        assert!(!accepts_text(&format!(
            "SELECT ?s (<{agg}>(?o) AS ?u) WHERE {{ ?s ?p ?o }} GROUP BY ?s"
        )));
    }
}
