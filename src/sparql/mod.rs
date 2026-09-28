pub mod federation;
pub mod rdf12_functions;
pub mod service_description;

/// The SPARQL parser for everything this store parses outside its evaluator —
/// scoped and batch queries, the update ACL check, update-target analysis,
/// syntax checks. It knows the custom aggregates the engine evaluates
/// (`geof:aggUnion`), so a query parses here exactly as the evaluator reads it;
/// a plain `spargebra::SparqlParser` would read `geof:aggUnion(?g)` as a
/// function call, and reject it outright under `GROUP BY`.
pub fn parser() -> spargebra::SparqlParser {
    crate::geo::aggregates::register_with_parser();
    opengraph::sparql_parser()
}
