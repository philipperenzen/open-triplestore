//! `SERVICE ?var`: evaluate a variable endpoint once per binding of the
//! variable, by rewriting the algebra into a lateral join.
//!
//! The evaluator hands a `SERVICE` only the tuple flowing *into* it, and the
//! optimiser never turns a join with a `SERVICE` into a for-loop, so in
//! `{ ?x :endpoint ?ep . SERVICE ?ep { … } }` the variable is still unbound when
//! the `SERVICE` runs and the query fails with "the variable encoding the
//! service name is unbound". Under an explicit `LATERAL` the left tuple is passed
//! through, so the same query works when written as `LATERAL { SERVICE ?ep { … } }`.
//! [`rewrite_service_var`] makes that rewrite for the caller (SPARQL 1.1
//! Federated Query §4, informative: the variable is bound by the part of the
//! group that comes before the `SERVICE`):
//!
//! * `Join(X, Service(?v, P))` → `Lateral(X, Service(?v, P))`
//! * `LeftJoin(X, Service(?v, P), e)` → `Lateral(X, LeftJoin(Ω0, Service(?v, P), e))`
//!   (an `OPTIONAL { SERVICE ?v { … } }`; the evaluator runs this shape as a
//!   for-loop left join)
//!
//! only when `X` can bind `?v` and `P` does not: the remote gets `P` unchanged
//! (no bindings are pushed), so the lateral form and the join agree on every
//! row where `?v` is bound, and the rewrite only changes rows the join could
//! not evaluate at all. A `SERVICE ?v` the rule does not match (on the left of
//! its join, under `MINUS` or `UNION` with nothing before it, or with `?v` bound
//! inside `P`) is left as it is and keeps the evaluator's behaviour.

use spargebra::algebra::GraphPattern;
use spargebra::term::NamedNodePattern;
use spargebra::Query;

/// Cheap text gate: whether `sparql` may contain `SERVICE ?var` (or
/// `SERVICE SILENT ?var`, or `$var`). A false positive (the word in a string
/// or IRI) only costs a parse; the parsed algebra decides.
pub fn mentions_service_variable(sparql: &str) -> bool {
    let b = sparql.as_bytes();
    let word = |c: &u8| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'?' | b'$' | b':');
    let mut i = 0;
    while let Some(off) = find_ci(&b[i..], b"service") {
        let (start, end) = (i + off, i + off + b"service".len());
        i = start + 1;
        // The keyword itself, not `?service` or `:serviceName`.
        let ident = |c: &u8| c.is_ascii_alphanumeric() || *c == b'_';
        if (start > 0 && word(&b[start - 1])) || b.get(end).is_some_and(ident) {
            continue;
        }
        let mut j = skip_blank(b, end);
        if b.len() >= j + 6 && b[j..j + 6].eq_ignore_ascii_case(b"silent") {
            j = skip_blank(b, j + 6);
        }
        if matches!(b.get(j), Some(b'?' | b'$')) {
            return true;
        }
    }
    false
}

fn find_ci(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle))
}

/// Skip whitespace and `#` comments.
fn skip_blank(b: &[u8], mut j: usize) -> usize {
    loop {
        match b.get(j) {
            Some(c) if c.is_ascii_whitespace() => j += 1,
            Some(b'#') => {
                while b.get(j).is_some_and(|c| *c != b'\n') {
                    j += 1;
                }
            }
            _ => return j,
        }
    }
}

/// Whether the query's pattern contains a `SERVICE` anywhere (outside
/// `EXISTS` expressions).
pub fn has_service(query: &Query) -> bool {
    fn walk(p: &GraphPattern) -> bool {
        match p {
            GraphPattern::Service { .. } => true,
            GraphPattern::Bgp { .. } | GraphPattern::Path { .. } | GraphPattern::Values { .. } => {
                false
            }
            GraphPattern::Join { left, right }
            | GraphPattern::LeftJoin { left, right, .. }
            | GraphPattern::Lateral { left, right }
            | GraphPattern::Union { left, right }
            | GraphPattern::Minus { left, right } => walk(left) || walk(right),
            GraphPattern::Filter { inner, .. }
            | GraphPattern::Graph { inner, .. }
            | GraphPattern::Extend { inner, .. }
            | GraphPattern::OrderBy { inner, .. }
            | GraphPattern::Project { inner, .. }
            | GraphPattern::Distinct { inner }
            | GraphPattern::Reduced { inner }
            | GraphPattern::Slice { inner, .. }
            | GraphPattern::Group { inner, .. } => walk(inner),
        }
    }
    walk(pattern_of(query))
}

fn pattern_of(query: &Query) -> &GraphPattern {
    match query {
        Query::Select { pattern, .. }
        | Query::Construct { pattern, .. }
        | Query::Describe { pattern, .. }
        | Query::Ask { pattern, .. } => pattern,
    }
}

/// Rewrite every eligible `SERVICE ?var` in `query` into a lateral join (see
/// the module docs). Returns whether anything changed.
pub fn rewrite_service_var(query: &mut Query) -> bool {
    let pattern = match query {
        Query::Select { pattern, .. }
        | Query::Construct { pattern, .. }
        | Query::Describe { pattern, .. }
        | Query::Ask { pattern, .. } => pattern,
    };
    let mut changed = false;
    let taken = std::mem::replace(pattern, GraphPattern::Bgp { patterns: vec![] });
    *pattern = rewrite(taken, &mut changed);
    changed
}

/// The variable of a `SERVICE ?v { P }` that `left` may bind and `P` does not.
fn eligible(left: &GraphPattern, right: &GraphPattern) -> bool {
    let GraphPattern::Service {
        name: NamedNodePattern::Variable(v),
        inner,
        ..
    } = right
    else {
        return false;
    };
    let mut in_left = false;
    left.on_in_scope_variable(|x| in_left |= x == v);
    let mut in_inner = false;
    inner.on_in_scope_variable(|x| in_inner |= x == v);
    in_left && !in_inner
}

fn rewrite(p: GraphPattern, changed: &mut bool) -> GraphPattern {
    let mut go = |p: Box<GraphPattern>| Box::new(rewrite(*p, changed));
    match p {
        GraphPattern::Join { left, right } => {
            let (left, right) = (go(left), go(right));
            if eligible(&left, &right) {
                *changed = true;
                GraphPattern::Lateral { left, right }
            } else {
                GraphPattern::Join { left, right }
            }
        }
        GraphPattern::LeftJoin {
            left,
            right,
            expression,
        } => {
            let (left, right) = (go(left), go(right));
            if eligible(&left, &right) {
                *changed = true;
                GraphPattern::Lateral {
                    left,
                    right: Box::new(GraphPattern::LeftJoin {
                        left: Box::new(GraphPattern::Bgp { patterns: vec![] }),
                        right,
                        expression,
                    }),
                }
            } else {
                GraphPattern::LeftJoin {
                    left,
                    right,
                    expression,
                }
            }
        }
        GraphPattern::Lateral { left, right } => GraphPattern::Lateral {
            left: go(left),
            right: go(right),
        },
        GraphPattern::Union { left, right } => GraphPattern::Union {
            left: go(left),
            right: go(right),
        },
        GraphPattern::Minus { left, right } => GraphPattern::Minus {
            left: go(left),
            right: go(right),
        },
        GraphPattern::Filter { expr, inner } => GraphPattern::Filter {
            expr,
            inner: go(inner),
        },
        GraphPattern::Graph { name, inner } => GraphPattern::Graph {
            name,
            inner: go(inner),
        },
        GraphPattern::Extend {
            inner,
            variable,
            expression,
        } => GraphPattern::Extend {
            inner: go(inner),
            variable,
            expression,
        },
        GraphPattern::OrderBy { inner, expression } => GraphPattern::OrderBy {
            inner: go(inner),
            expression,
        },
        GraphPattern::Project { inner, variables } => GraphPattern::Project {
            inner: go(inner),
            variables,
        },
        GraphPattern::Distinct { inner } => GraphPattern::Distinct { inner: go(inner) },
        GraphPattern::Reduced { inner } => GraphPattern::Reduced { inner: go(inner) },
        GraphPattern::Slice {
            inner,
            start,
            length,
        } => GraphPattern::Slice {
            inner: go(inner),
            start,
            length,
        },
        GraphPattern::Group {
            inner,
            variables,
            aggregates,
        } => GraphPattern::Group {
            inner: go(inner),
            variables,
            aggregates,
        },
        GraphPattern::Service {
            name,
            inner,
            silent,
        } => GraphPattern::Service {
            name,
            inner: go(inner),
            silent,
        },
        p
        @ (GraphPattern::Bgp { .. } | GraphPattern::Path { .. } | GraphPattern::Values { .. }) => p,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(q: &str) -> Query {
        spargebra::SparqlParser::new().parse_query(q).unwrap()
    }

    /// The rewritten query, as the algebra's own SPARQL serialisation.
    fn rewritten(q: &str) -> Option<String> {
        let mut query = parse(q);
        rewrite_service_var(&mut query).then(|| query.to_string())
    }

    #[test]
    fn text_gate_finds_variable_endpoints_only() {
        assert!(mentions_service_variable(
            "SELECT * { SERVICE ?ep { ?s ?p ?o } }"
        ));
        assert!(mentions_service_variable("select * { service   $ep {} }"));
        assert!(mentions_service_variable(
            "SELECT * { SERVICE SILENT\n ?ep { ?s ?p ?o } }"
        ));
        assert!(mentions_service_variable(
            "SELECT * { SERVICE # endpoint from the data\n ?ep { } }"
        ));
        assert!(!mentions_service_variable(
            "SELECT * { SERVICE <http://example.org/sparql> { ?s ?p ?o } }"
        ));
        assert!(mentions_service_variable("SELECT * { SERVICE?ep { } }"));
        assert!(!mentions_service_variable(
            "SELECT ?service { ?service ?p ?o }"
        ));
        assert!(!mentions_service_variable(
            "SELECT * { ?s :service ?o . ?o :services ?x }"
        ));
        assert!(!mentions_service_variable("SELECT * { ?s ?p ?o }"));
        assert!(!mentions_service_variable("SERVICE"));
    }

    #[test]
    fn join_with_a_bound_endpoint_becomes_lateral() {
        let q = "SELECT * { ?x <urn:ep> ?ep SERVICE ?ep { ?s <urn:p> ?o } }";
        let out = rewritten(q).expect("rewritten");
        assert!(out.contains("LATERAL"), "{out}");
        // The SERVICE pattern itself is sent unchanged.
        let query = {
            let mut query = parse(q);
            rewrite_service_var(&mut query);
            query
        };
        let Query::Select { pattern, .. } = &query else {
            unreachable!()
        };
        let mut found = false;
        fn find(p: &GraphPattern, found: &mut bool) {
            if let GraphPattern::Lateral { left, right } = p {
                assert!(matches!(left.as_ref(), GraphPattern::Bgp { .. }));
                assert!(matches!(right.as_ref(), GraphPattern::Service { .. }));
                *found = true;
            }
            if let GraphPattern::Project { inner, .. } = p {
                find(inner, found)
            }
        }
        find(pattern, &mut found);
        assert!(found, "{pattern:?}");
    }

    #[test]
    fn optional_service_becomes_lateral_left_join_from_the_empty_pattern() {
        let mut query =
            parse("SELECT * { ?x <urn:ep> ?ep OPTIONAL { SERVICE ?ep { ?x <urn:p> ?o } } }");
        assert!(rewrite_service_var(&mut query));
        let Query::Select { pattern, .. } = &query else {
            unreachable!()
        };
        let GraphPattern::Project { inner, .. } = pattern else {
            panic!("{pattern:?}")
        };
        let GraphPattern::Lateral { right, .. } = inner.as_ref() else {
            panic!("{inner:?}")
        };
        let GraphPattern::LeftJoin { left, right, .. } = right.as_ref() else {
            panic!("{right:?}")
        };
        assert_eq!(left.as_ref(), &GraphPattern::Bgp { patterns: vec![] });
        assert!(matches!(right.as_ref(), GraphPattern::Service { .. }));
    }

    #[test]
    fn nested_groups_and_filters_are_rewritten() {
        let q = "SELECT ?t { { ?p <urn:subject> ?subj ; <urn:ep> ?svc FILTER regex(?subj, \"remote\") } \
                 SERVICE ?svc { ?project <urn:name> ?t } }";
        assert!(rewritten(q).is_some());
        let q = "SELECT * { GRAPH ?g { ?x <urn:ep> ?ep SERVICE SILENT ?ep { ?s ?p ?o } } }";
        assert!(rewritten(q).is_some());
    }

    #[test]
    fn ineligible_shapes_are_left_alone() {
        // A constant endpoint.
        assert!(rewritten("SELECT * { ?x ?y ?z SERVICE <urn:e> { ?s ?p ?o } }").is_none());
        // The variable is bound inside the SERVICE pattern.
        assert!(rewritten("SELECT * { ?x <urn:ep> ?ep SERVICE ?ep { ?ep ?p ?o } }").is_none());
        // Nothing before the SERVICE binds it.
        assert!(rewritten("SELECT * { ?x ?y ?z SERVICE ?ep { ?s ?p ?o } }").is_none());
        assert!(rewritten("SELECT * { SERVICE ?ep { ?s ?p ?o } }").is_none());
        // The SERVICE comes first: the endpoint is not bound before it.
        assert!(rewritten("SELECT * { SERVICE ?ep { ?s ?p ?o } ?x <urn:ep> ?ep }").is_none());
    }

    #[test]
    fn has_service_sees_nested_services() {
        assert!(has_service(&parse(
            "SELECT * { ?s ?p ?o OPTIONAL { SERVICE <urn:e> { ?s ?q ?r } } }"
        )));
        assert!(!has_service(&parse("SELECT * { ?s <urn:service> ?o }")));
    }
}
