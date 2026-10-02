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

/// The graph patterns a node of the algebra evaluates through its own
/// expressions: the bodies of every `EXISTS` / `NOT EXISTS` in a `FILTER`, a
/// `BIND`, an `OPTIONAL`'s condition, an `ORDER BY` key or an aggregate.
/// A walker that bounds what a pattern reads must descend into these as well
/// as into the node's sub-patterns: `FILTER EXISTS { GRAPH <g> { … } }`
/// reads `<g>` as surely as `GRAPH <g> { … }` does.
pub fn exists_patterns(
    p: &spargebra::algebra::GraphPattern,
) -> Vec<&spargebra::algebra::GraphPattern> {
    use spargebra::algebra::{AggregateExpression, GraphPattern as GP, OrderExpression};
    let mut out = Vec::new();
    match p {
        GP::Filter { expr, .. } => in_expression(expr, &mut out),
        GP::Extend { expression, .. } => in_expression(expression, &mut out),
        GP::LeftJoin {
            expression: Some(expression),
            ..
        } => in_expression(expression, &mut out),
        GP::OrderBy { expression, .. } => {
            for key in expression {
                match key {
                    OrderExpression::Asc(e) | OrderExpression::Desc(e) => {
                        in_expression(e, &mut out)
                    }
                }
            }
        }
        GP::Group { aggregates, .. } => {
            for (_, aggregate) in aggregates {
                if let AggregateExpression::FunctionCall { expr, .. } = aggregate {
                    in_expression(expr, &mut out);
                }
            }
        }
        _ => {}
    }
    out
}

fn in_expression<'a>(
    e: &'a spargebra::algebra::Expression,
    out: &mut Vec<&'a spargebra::algebra::GraphPattern>,
) {
    use spargebra::algebra::Expression as E;
    match e {
        E::Exists(p) => out.push(p),
        E::NamedNode(_) | E::Literal(_) | E::Variable(_) | E::Bound(_) => {}
        E::Or(a, b)
        | E::And(a, b)
        | E::Equal(a, b)
        | E::SameTerm(a, b)
        | E::Greater(a, b)
        | E::GreaterOrEqual(a, b)
        | E::Less(a, b)
        | E::LessOrEqual(a, b)
        | E::Add(a, b)
        | E::Subtract(a, b)
        | E::Multiply(a, b)
        | E::Divide(a, b) => {
            in_expression(a, out);
            in_expression(b, out);
        }
        E::UnaryPlus(a) | E::UnaryMinus(a) | E::Not(a) => in_expression(a, out),
        E::In(a, list) => {
            in_expression(a, out);
            for x in list {
                in_expression(x, out);
            }
        }
        E::If(a, b, c) => {
            in_expression(a, out);
            in_expression(b, out);
            in_expression(c, out);
        }
        E::Coalesce(list) | E::FunctionCall(_, list) => {
            for x in list {
                in_expression(x, out);
            }
        }
    }
}

#[cfg(test)]
mod exists_tests {
    use super::exists_patterns;
    use spargebra::algebra::GraphPattern as GP;

    /// Every place an expression sits in the algebra hands its EXISTS bodies
    /// over, however deeply the EXISTS is nested in the expression.
    #[test]
    fn every_expression_position_yields_its_exists_bodies() {
        fn count(p: &GP) -> usize {
            let here = exists_patterns(p).len();
            here + match p {
                GP::Join { left, right }
                | GP::Union { left, right }
                | GP::Minus { left, right }
                | GP::Lateral { left, right }
                | GP::LeftJoin { left, right, .. } => count(left) + count(right),
                GP::Filter { inner, .. }
                | GP::Extend { inner, .. }
                | GP::OrderBy { inner, .. }
                | GP::Project { inner, .. }
                | GP::Distinct { inner, .. }
                | GP::Reduced { inner, .. }
                | GP::Slice { inner, .. }
                | GP::Group { inner, .. }
                | GP::Graph { inner, .. }
                | GP::Service { inner, .. } => count(inner),
                GP::Bgp { .. } | GP::Path { .. } | GP::Values { .. } => 0,
            }
        }
        let e = "EXISTS { GRAPH <urn:g> { ?a ?b ?c } }";
        for (query, n) in [
            (format!("SELECT * {{ ?s ?p ?o FILTER({e}) }}"), 1),
            (
                format!("SELECT * {{ ?s ?p ?o FILTER(!(1 + IF({e}, 1, 0) > 0)) }}"),
                1,
            ),
            (
                format!("SELECT * {{ BIND(COALESCE({e}, NOT {e}) AS ?x) }}"),
                2,
            ),
            (
                format!("SELECT * {{ ?s ?p ?o OPTIONAL {{ ?o ?q ?r FILTER({e}) }} }}"),
                1,
            ),
            (format!("SELECT ?s {{ ?s ?p ?o }} ORDER BY DESC({e})"), 1),
            (
                format!("SELECT (SUM(IF({e}, 1, 0)) AS ?n) {{ ?s ?p ?o }}"),
                1,
            ),
            (
                format!("SELECT * {{ ?s ?p ?o FILTER(?o IN (1, STR({e}))) }}"),
                1,
            ),
            ("SELECT * { ?s ?p ?o FILTER(?o > 1) }".to_string(), 0),
        ] {
            let parsed = spargebra::SparqlParser::new().parse_query(&query).unwrap();
            let spargebra::Query::Select { pattern, .. } = parsed else {
                unreachable!()
            };
            assert_eq!(count(&pattern), n, "{query}");
        }
    }
}
