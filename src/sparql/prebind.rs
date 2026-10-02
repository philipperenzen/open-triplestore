//! SHACL pre-binding (SHACL §5.3.1, Appendix A) for queries the store evaluates:
//! `sh:sparql` constraints, constraint-component validators and SHACL-AF
//! SPARQL rules, whose `$this` (and `$value`, and the parameters) are values
//! the engine supplies.
//!
//! Appendix A defines evaluating a query with pre-bound variables μ as
//! evaluating `Replace(E, μ)`: every basic graph pattern, property path and
//! `GRAPH ?var` pattern `Y` of the algebra becomes `join(Y, Table(μ))`, so the
//! value reaches every scope — a `FILTER` in a nested group, a `UNION` branch,
//! `bound($this)`. [`rewrite`] does exactly that on the parsed algebra.
//! `VALUES` would be the natural `Table(μ)`, but `VALUES` cannot hold a blank
//! node and a focus or value node often is one, so the table is
//! `BIND(<urn:ots:prebound:v>() AS ?v)` over the empty group, one per variable:
//! a call to a function that [`prepare`] registers on the evaluator of the one
//! query, returning the term.
//!
//! Two simpler routes fail:
//! * pasting the value into the query text cannot name a stored blank node,
//!   so blank-node focus and value nodes were skipped and conformed unchecked;
//! * oxigraph's `substitute_variable` seeds the evaluation, but the optimizer
//!   rewrites the algebra first without knowing about the seed: it drops
//!   `{ FILTER ($this = …) }` as a filter over a variable nothing binds and
//!   folds `bound($this)` to false. With the optimizer off instead, every basic
//!   graph pattern becomes a chain of cartesian products.
//!
//! The rewrite alone is correct but slow: the optimizer types a custom
//! function's result as possibly unbound, so it finds no join key between the
//! table and a triple pattern, and evaluates the pattern with `$this` free —
//! a scan of every triple with that predicate, per focus node. [`prepare`]
//! therefore *also* seeds the evaluation with `substitute_variable`. The two
//! carry the same term: the rewrite tells the optimizer the variable is bound
//! in every scope, the seed is what the triple-pattern lookups below the table
//! see, so they look up by the value. They differ only for a query whose top
//! level is nothing but a sub-select that does not project the variable — a
//! query SHACL forbids under pre-binding — where the seed binds it at the top
//! level and the rewrite alone left it unbound there.

use oxigraph::model::{NamedNode, Term};
use oxigraph::sparql::{PreparedSparqlQuery, SparqlEvaluator};
use spargebra::algebra::{Expression, Function, GraphPattern};
use spargebra::term::{NamedNodePattern, Variable};
use spargebra::Query;

/// The namespace of the functions that return pre-bound values.
const FUNCTION_NS: &str = "urn:ots:prebound:";

fn function(var: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("{FUNCTION_NS}{var}"))
}

/// Rewrite `query` so that every variable in `vars` is pre-bound when it is
/// evaluated through [`prepare`]: `Replace(E, μ)` of SHACL
/// Appendix A. An error names a variable that is no valid SPARQL variable.
pub fn rewrite(query: &mut Query, vars: &[&str]) -> Result<(), String> {
    if vars.is_empty() {
        return Ok(());
    }
    let mut table = GraphPattern::Bgp {
        patterns: Vec::new(),
    };
    for v in vars {
        let variable = Variable::new(*v).map_err(|e| format!("pre-bound variable ${v}: {e}"))?;
        table = GraphPattern::Extend {
            inner: Box::new(table),
            variable,
            expression: Expression::FunctionCall(Function::Custom(function(v)), Vec::new()),
        };
    }
    let pattern = match query {
        Query::Select { pattern, .. }
        | Query::Construct { pattern, .. }
        | Query::Describe { pattern, .. }
        | Query::Ask { pattern, .. } => pattern,
    };
    replace(pattern, &table);
    // The parser wraps the pattern in a projection of the variables in scope
    // of the text as written. A pre-bound variable that only a `FILTER` (or
    // nothing) mentions is not among them, so a CONSTRUCT template, or a
    // message template reading a SELECT solution, would find it projected
    // away. Project it too.
    let mut p = pattern;
    loop {
        match p {
            GraphPattern::Distinct { inner }
            | GraphPattern::Reduced { inner }
            | GraphPattern::Slice { inner, .. }
            | GraphPattern::OrderBy { inner, .. } => p = inner,
            GraphPattern::Project { variables, .. } => {
                for v in vars {
                    let v =
                        Variable::new(*v).map_err(|e| format!("pre-bound variable ${v}: {e}"))?;
                    if !variables.contains(&v) {
                        variables.push(v);
                    }
                }
                break;
            }
            _ => break,
        }
    }
    Ok(())
}

/// Prepare a [`rewrite`]-ten `query` on `evaluator` with `bindings`
/// pre-bound: the functions the rewritten query calls, each returning its
/// variable's value, and the same values seeded into the evaluation so that
/// triple patterns are looked up by them (see the module docs). An error
/// names a variable that is no valid SPARQL variable.
pub fn prepare(
    mut evaluator: SparqlEvaluator,
    query: Query,
    bindings: &[(&str, &Term)],
) -> Result<PreparedSparqlQuery, String> {
    for (name, term) in bindings {
        let term = (*term).clone();
        evaluator = evaluator.with_custom_function(function(name), move |_| Some(term.clone()));
    }
    let mut prepared = evaluator.for_query(query);
    for (name, term) in bindings {
        let variable =
            Variable::new(*name).map_err(|e| format!("pre-bound variable ${name}: {e}"))?;
        prepared = prepared.substitute_variable(variable, (*term).clone());
    }
    Ok(prepared)
}

fn join_table(pattern: &mut GraphPattern, table: &GraphPattern) {
    let y = std::mem::replace(
        pattern,
        GraphPattern::Bgp {
            patterns: Vec::new(),
        },
    );
    *pattern = GraphPattern::Join {
        left: Box::new(y),
        right: Box::new(table.clone()),
    };
}

fn replace(pattern: &mut GraphPattern, table: &GraphPattern) {
    match pattern {
        GraphPattern::Bgp { .. } | GraphPattern::Path { .. } => join_table(pattern, table),
        GraphPattern::Graph { name, inner } => {
            replace(inner, table);
            if matches!(name, NamedNodePattern::Variable(_)) {
                join_table(pattern, table);
            }
        }
        GraphPattern::Join { left, right }
        | GraphPattern::Lateral { left, right }
        | GraphPattern::Union { left, right }
        | GraphPattern::Minus { left, right } => {
            replace(left, table);
            replace(right, table);
        }
        GraphPattern::LeftJoin {
            left,
            right,
            expression,
        } => {
            replace(left, table);
            replace(right, table);
            if let Some(e) = expression {
                replace_in_expression(e, table);
            }
        }
        GraphPattern::Filter { expr, inner } => {
            replace(inner, table);
            replace_in_expression(expr, table);
        }
        GraphPattern::Extend {
            inner, expression, ..
        } => {
            replace(inner, table);
            replace_in_expression(expression, table);
        }
        GraphPattern::OrderBy { inner, expression } => {
            replace(inner, table);
            for e in expression {
                match e {
                    spargebra::algebra::OrderExpression::Asc(e)
                    | spargebra::algebra::OrderExpression::Desc(e) => {
                        replace_in_expression(e, table)
                    }
                }
            }
        }
        GraphPattern::Group {
            inner, aggregates, ..
        } => {
            replace(inner, table);
            for (_, aggregate) in aggregates {
                if let spargebra::algebra::AggregateExpression::FunctionCall { expr, .. } =
                    aggregate
                {
                    replace_in_expression(expr, table);
                }
            }
        }
        GraphPattern::Project { inner, .. }
        | GraphPattern::Distinct { inner }
        | GraphPattern::Reduced { inner }
        | GraphPattern::Slice { inner, .. }
        | GraphPattern::Service { inner, .. } => replace(inner, table),
        GraphPattern::Values { .. } => {}
    }
}

fn replace_in_expression(expression: &mut Expression, table: &GraphPattern) {
    match expression {
        Expression::Exists(pattern) => replace(pattern, table),
        Expression::Or(a, b)
        | Expression::And(a, b)
        | Expression::Equal(a, b)
        | Expression::SameTerm(a, b)
        | Expression::Greater(a, b)
        | Expression::GreaterOrEqual(a, b)
        | Expression::Less(a, b)
        | Expression::LessOrEqual(a, b)
        | Expression::Add(a, b)
        | Expression::Subtract(a, b)
        | Expression::Multiply(a, b)
        | Expression::Divide(a, b) => {
            replace_in_expression(a, table);
            replace_in_expression(b, table);
        }
        Expression::In(a, list) => {
            replace_in_expression(a, table);
            for e in list {
                replace_in_expression(e, table);
            }
        }
        Expression::UnaryPlus(a) | Expression::UnaryMinus(a) | Expression::Not(a) => {
            replace_in_expression(a, table)
        }
        Expression::If(a, b, c) => {
            replace_in_expression(a, table);
            replace_in_expression(b, table);
            replace_in_expression(c, table);
        }
        Expression::Coalesce(list) | Expression::FunctionCall(_, list) => {
            for e in list {
                replace_in_expression(e, table);
            }
        }
        Expression::NamedNode(_)
        | Expression::Literal(_)
        | Expression::Variable(_)
        | Expression::Bound(_) => {}
    }
}
