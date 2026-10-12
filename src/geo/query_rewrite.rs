//! GeoSPARQL Query Rewrite Extension (OGC 11-052r4 Req 28–30, 22-047r1 §13).
//!
//! The extension asks that a triple pattern with a topological relation as
//! its predicate — `?f geo:sfWithin ?r`, any of the 24 Simple Features,
//! Egenhofer and RCC8 relations — also matches when the relation is not
//! asserted but follows from the geometries, as if the standard's RIF rules
//! had been materialised. Each rule reads, for the relation `rel`:
//!
//! ```text
//! ?a geo:rel ?b  :-  ?a P ?l1 . ?b P ?l2 . FILTER(geof:rel(?l1, ?l2))
//! ```
//!
//! where each side is either a feature, reached through its default geometry
//! (`geo:hasDefaultGeometry`), or a geometry itself, and `P` reads that
//! geometry's serialisation. That is the four rule shapes of the standard
//! (feature–feature, feature–geometry, geometry–feature, geometry–geometry)
//! in one property path:
//!
//! ```text
//! P = (geo:hasDefaultGeometry/S) | S      S = geo:asWKT | geo:asGML | geo:asGeoJSON | geo:asKML
//! ```
//!
//! This is spec-strict: a feature that has a geometry only through
//! `geo:hasGeometry` (no default geometry) is not related by the rewrite.
//!
//! The rewrite works on the query's syntax tree. Every triple pattern with a
//! constant relation predicate becomes
//!
//! ```text
//! { SELECT DISTINCT vars WHERE { { s rel o } UNION { s P ?l1 . o P ?l2 FILTER(geof:rel(?l1, ?l2)) } } }
//! ```
//!
//! so an asserted relation still matches, a derived one matches too, and a
//! pair related both ways (or through two serialisations of one geometry)
//! matches once: the result is the set a materialised relation graph would
//! give. A pattern with no variable becomes `FILTER EXISTS { … }`. Inside a
//! `GRAPH` pattern the rewrite stays inside that graph. A variable predicate
//! (`?s ?p ?o`) is left alone, as the standard allows, and so are property
//! paths, `SERVICE` blocks (a remote endpoint evaluates those) and triple
//! patterns whose subject or object is a literal or a triple term. Blank
//! nodes in a rewritten pattern become fresh variables throughout their basic
//! graph pattern, so the pattern still joins with its neighbours.
//!
//! It applies to queries and to the `WHERE` clause of `DELETE`/`INSERT`
//! updates. It is on by default; `OTS_GEOSPARQL_QUERY_REWRITE=off` (or `0`,
//! `false`, `no`) turns it off for a server ([`enabled_from_env`]).
//!
//! Cost: a query that names no relation pays one substring scan
//! ([`mentions_relation`]). A pattern whose subject and object are both
//! unbound compares every pair of serialisations in scope (n² function calls)
//! until a spatial index restricts the candidates.

use std::collections::HashMap;

use spargebra::algebra::{
    AggregateExpression, Expression, Function, GraphPattern, OrderExpression,
    PropertyPathExpression,
};
use spargebra::term::{
    BlankNode, NamedNode, NamedNodePattern, TermPattern, TriplePattern, Variable,
};
use spargebra::{GraphUpdateOperation, Query, Update};

/// The GeoSPARQL ontology namespace, which names the relation properties.
const GEO: &str = "http://www.opengis.net/ont/geosparql#";
/// The GeoSPARQL function namespace, which names the matching functions.
const GEOF: &str = "http://www.opengis.net/def/function/geosparql/";

/// The environment variable that turns the rewrite off.
pub const ENV_VAR: &str = "OTS_GEOSPARQL_QUERY_REWRITE";

/// The 24 topological relations of the Simple Features, Egenhofer and RCC8
/// families: `geo:<name>` is the property, `geof:<name>` the function.
pub const RELATIONS: [&str; 24] = [
    "sfEquals",
    "sfDisjoint",
    "sfIntersects",
    "sfTouches",
    "sfCrosses",
    "sfWithin",
    "sfContains",
    "sfOverlaps",
    "ehEquals",
    "ehDisjoint",
    "ehMeet",
    "ehOverlap",
    "ehCovers",
    "ehCoveredBy",
    "ehInside",
    "ehContains",
    "rcc8eq",
    "rcc8dc",
    "rcc8ec",
    "rcc8po",
    "rcc8tppi",
    "rcc8tpp",
    "rcc8ntpp",
    "rcc8ntppi",
];

/// The serialisation properties a rule reads a geometry's literal from.
const SERIALISATIONS: [&str; 4] = ["asWKT", "asGML", "asGeoJSON", "asKML"];

/// Whether the rewrite is on for this process: on unless
/// [`OTS_GEOSPARQL_QUERY_REWRITE`](ENV_VAR) says `off`, `0`, `false` or `no`.
pub fn enabled_from_env() -> bool {
    switch_value(std::env::var(ENV_VAR).ok().as_deref())
}

/// The switch's reading of a value: unset or anything but `off`, `0`,
/// `false`, `no` (any case) is on.
fn switch_value(v: Option<&str>) -> bool {
    v.is_none_or(|v| {
        !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "off" | "0" | "false" | "no"
        )
    })
}

/// A cheap test on the query text: whether it can contain a relation
/// pattern at all, so that only such a query is parsed again. It needs the
/// GeoSPARQL namespace and a relation name used other than as a function
/// call: `geof:sfWithin(…)` and `<…/geosparql/sfWithin>(…)` are skipped, so
/// a query that only filters with the functions pays a substring scan, not a
/// parse. A false positive costs one parse; there are no false negatives for
/// a relation written as a prefixed name or a full IRI.
pub fn mentions_relation(sparql: &str) -> bool {
    if !sparql.contains("geosparql") {
        return false;
    }
    RELATIONS.iter().any(|r| {
        sparql.match_indices(r).any(|(i, _)| {
            let rest = &sparql[i + r.len()..];
            // A longer name (`rcc8tpp` inside `rcc8tppi`) is that name's match.
            if rest
                .chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-')
            {
                return false;
            }
            let rest = rest.strip_prefix('>').unwrap_or(rest).trim_start();
            !rest.starts_with('(')
        })
    })
}

/// The relation a predicate names, if it is one of [`RELATIONS`].
fn relation_of(predicate: &NamedNodePattern) -> Option<&'static str> {
    let NamedNodePattern::NamedNode(n) = predicate else {
        return None;
    };
    let local = n.as_str().strip_prefix(GEO)?;
    RELATIONS.iter().copied().find(|r| *r == local)
}

/// Subject and object terms the rewrite can expand: a variable, an IRI or a
/// blank node. A literal subject never has a geometry, and a triple term is
/// not a spatial object.
fn expandable(term: &TermPattern) -> bool {
    matches!(
        term,
        TermPattern::Variable(_) | TermPattern::NamedNode(_) | TermPattern::BlankNode(_)
    )
}

fn rewritable(t: &TriplePattern) -> bool {
    relation_of(&t.predicate).is_some() && expandable(&t.subject) && expandable(&t.object)
}

/// `(geo:hasDefaultGeometry/S) | S` with `S` the serialisation properties.
fn serialisation_path() -> PropertyPathExpression {
    let s = SERIALISATIONS
        .iter()
        .map(|l| PropertyPathExpression::NamedNode(NamedNode::new_unchecked(format!("{GEO}{l}"))))
        .reduce(|a, b| PropertyPathExpression::Alternative(Box::new(a), Box::new(b)))
        .expect("at least one serialisation");
    let default_geometry = PropertyPathExpression::NamedNode(NamedNode::new_unchecked(format!(
        "{GEO}hasDefaultGeometry"
    )));
    PropertyPathExpression::Alternative(
        Box::new(PropertyPathExpression::Sequence(
            Box::new(default_geometry),
            Box::new(s.clone()),
        )),
        Box::new(s),
    )
}

/// Rewrite the relation patterns of `query` in place. Returns whether
/// anything changed.
pub fn rewrite_query(query: &mut Query) -> bool {
    let mut r = Rewriter::default();
    match query {
        Query::Select { pattern, .. }
        | Query::Construct { pattern, .. }
        | Query::Describe { pattern, .. }
        | Query::Ask { pattern, .. } => r.pattern(pattern),
    }
    r.changed
}

/// Rewrite the relation patterns in the `WHERE` clauses of `update` in place.
/// Returns whether anything changed.
pub fn rewrite_update(update: &mut Update) -> bool {
    let mut r = Rewriter::default();
    for op in &mut update.operations {
        if let GraphUpdateOperation::DeleteInsert { pattern, .. } = op {
            r.pattern(pattern);
        }
    }
    r.changed
}

/// The rewritten text of a query, or `None` when it has no relation pattern
/// (or does not parse: the evaluator reports that error itself).
pub fn rewrite_query_text(sparql: &str) -> Option<String> {
    if !mentions_relation(sparql) {
        return None;
    }
    let mut q = crate::sparql::parser().parse_query(sparql).ok()?;
    rewrite_query(&mut q).then(|| q.to_string())
}

/// The rewritten text of an update, or `None` when no `WHERE` clause has a
/// relation pattern (or it does not parse).
pub fn rewrite_update_text(sparql: &str) -> Option<String> {
    if !mentions_relation(sparql) {
        return None;
    }
    let mut u = crate::sparql::parser().parse_update(sparql).ok()?;
    rewrite_update(&mut u).then(|| u.to_string())
}

#[derive(Default)]
struct Rewriter {
    /// Numbers the fresh variables, unique within one query.
    counter: usize,
    changed: bool,
}

impl Rewriter {
    fn fresh(&mut self, role: &str) -> Variable {
        self.counter += 1;
        Variable::new_unchecked(format!("__geor_{role}{}", self.counter))
    }

    fn pattern(&mut self, p: &mut GraphPattern) {
        match p {
            GraphPattern::Bgp { patterns } => {
                if patterns.iter().any(rewritable) {
                    let taken = std::mem::take(patterns);
                    *p = self.bgp(taken);
                }
            }
            GraphPattern::Join { left, right }
            | GraphPattern::Union { left, right }
            | GraphPattern::Minus { left, right } => {
                self.pattern(left);
                self.pattern(right);
            }
            GraphPattern::LeftJoin {
                left,
                right,
                expression,
            } => {
                self.pattern(left);
                self.pattern(right);
                if let Some(e) = expression {
                    self.expression(e);
                }
            }
            GraphPattern::Filter { expr, inner } => {
                self.expression(expr);
                self.pattern(inner);
            }
            GraphPattern::Graph { inner, .. } => self.pattern(inner),
            GraphPattern::Extend {
                inner, expression, ..
            } => {
                self.pattern(inner);
                self.expression(expression);
            }
            GraphPattern::OrderBy { inner, expression } => {
                self.pattern(inner);
                for o in expression {
                    match o {
                        OrderExpression::Asc(e) | OrderExpression::Desc(e) => self.expression(e),
                    }
                }
            }
            GraphPattern::Project { inner, .. }
            | GraphPattern::Distinct { inner }
            | GraphPattern::Reduced { inner }
            | GraphPattern::Slice { inner, .. } => self.pattern(inner),
            GraphPattern::Group {
                inner, aggregates, ..
            } => {
                self.pattern(inner);
                for (_, a) in aggregates {
                    if let AggregateExpression::FunctionCall { expr, .. } = a {
                        self.expression(expr);
                    }
                }
            }
            // A remote endpoint evaluates its SERVICE block; a property path
            // and an inline table hold no triple pattern.
            GraphPattern::Service { .. }
            | GraphPattern::Path { .. }
            | GraphPattern::Values { .. } => {}
            // LATERAL (spargebra's `sep-0006`), when compiled in.
            #[allow(unreachable_patterns)]
            _ => {}
        }
    }

    fn expression(&mut self, e: &mut Expression) {
        match e {
            Expression::Exists(p) => self.pattern(p),
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
                self.expression(a);
                self.expression(b);
            }
            Expression::In(a, list) => {
                self.expression(a);
                for x in list {
                    self.expression(x);
                }
            }
            Expression::UnaryPlus(a) | Expression::UnaryMinus(a) | Expression::Not(a) => {
                self.expression(a)
            }
            Expression::If(a, b, c) => {
                self.expression(a);
                self.expression(b);
                self.expression(c);
            }
            Expression::Coalesce(list) | Expression::FunctionCall(_, list) => {
                for x in list {
                    self.expression(x);
                }
            }
            Expression::NamedNode(_)
            | Expression::Literal(_)
            | Expression::Variable(_)
            | Expression::Bound(_) => {}
            #[allow(unreachable_patterns)]
            _ => {}
        }
    }

    /// A basic graph pattern with at least one relation pattern: the other
    /// triples stay one BGP, joined with one expansion per relation pattern.
    fn bgp(&mut self, patterns: Vec<TriplePattern>) -> GraphPattern {
        // Blank nodes of a rewritten pattern become variables in every triple
        // of the BGP: the expansion is a sub-query, and a blank-node label
        // cannot span two basic graph patterns.
        let mut bnodes: HashMap<BlankNode, Variable> = HashMap::new();
        for t in patterns.iter().filter(|t| rewritable(t)) {
            for term in [&t.subject, &t.object] {
                if let TermPattern::BlankNode(b) = term {
                    if !bnodes.contains_key(b) {
                        let v = self.fresh("b");
                        bnodes.insert(b.clone(), v);
                    }
                }
            }
        }
        let rename = |term: TermPattern| match term {
            TermPattern::BlankNode(b) => match bnodes.get(&b) {
                Some(v) => TermPattern::Variable(v.clone()),
                None => TermPattern::BlankNode(b),
            },
            other => other,
        };

        let mut kept = Vec::new();
        let mut parts = Vec::new();
        for t in patterns {
            let t = TriplePattern {
                subject: rename(t.subject),
                predicate: t.predicate,
                object: rename(t.object),
            };
            match relation_of(&t.predicate) {
                Some(rel) if rewritable(&t) => parts.push((t, rel)),
                _ => kept.push(t),
            }
        }
        self.changed = true;

        let mut out = (!kept.is_empty()).then_some(GraphPattern::Bgp { patterns: kept });
        for (t, rel) in parts {
            let part = self.expand(t, rel);
            out = Some(match out {
                None => part,
                Some(prev) => GraphPattern::Join {
                    left: Box::new(prev),
                    right: Box::new(part),
                },
            });
        }
        out.unwrap_or_default()
    }

    /// `{ SELECT DISTINCT vars { { s rel o } UNION { s P ?l1 . o P ?l2 FILTER(geof:rel(?l1, ?l2)) } } }`,
    /// or `FILTER EXISTS { … }` over the same union when `s` and `o` are both
    /// constants.
    fn expand(&mut self, t: TriplePattern, rel: &str) -> GraphPattern {
        let l1 = self.fresh("l");
        let l2 = self.fresh("l");
        let path = serialisation_path();
        let derived = GraphPattern::Filter {
            expr: Expression::FunctionCall(
                Function::Custom(NamedNode::new_unchecked(format!("{GEOF}{rel}"))),
                vec![
                    Expression::Variable(l1.clone()),
                    Expression::Variable(l2.clone()),
                ],
            ),
            inner: Box::new(GraphPattern::Join {
                left: Box::new(GraphPattern::Path {
                    subject: t.subject.clone(),
                    path: path.clone(),
                    object: TermPattern::Variable(l1),
                }),
                right: Box::new(GraphPattern::Path {
                    subject: t.object.clone(),
                    path,
                    object: TermPattern::Variable(l2),
                }),
            }),
        };
        let mut variables: Vec<Variable> = Vec::new();
        for term in [&t.subject, &t.object] {
            if let TermPattern::Variable(v) = term {
                if !variables.contains(v) {
                    variables.push(v.clone());
                }
            }
        }
        let union = GraphPattern::Union {
            left: Box::new(GraphPattern::Bgp { patterns: vec![t] }),
            right: Box::new(derived),
        };
        if variables.is_empty() {
            GraphPattern::Filter {
                expr: Expression::Exists(Box::new(union)),
                inner: Box::new(GraphPattern::Bgp {
                    patterns: Vec::new(),
                }),
            }
        } else {
            GraphPattern::Distinct {
                inner: Box::new(GraphPattern::Project {
                    inner: Box::new(union),
                    variables,
                }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PFX: &str = "PREFIX geo: <http://www.opengis.net/ont/geosparql#>\n\
                       PREFIX geof: <http://www.opengis.net/def/function/geosparql/>\n\
                       PREFIX ex: <http://example.org/>\n";

    fn rw(q: &str) -> Option<String> {
        rewrite_query_text(&format!("{PFX}{q}"))
    }

    /// The rewritten text parses again, so the evaluator and every
    /// accelerator that re-parses the text read the same query.
    fn reparses(text: &str) {
        crate::sparql::parser()
            .parse_query(text)
            .unwrap_or_else(|e| panic!("rewritten query does not parse: {e}\n{text}"));
    }

    #[test]
    fn a_query_without_a_relation_is_left_alone() {
        assert!(rw("SELECT * WHERE { ?f geo:hasGeometry ?g }").is_none());
        // The function of the same name is not a relation pattern.
        assert!(rw("SELECT * WHERE { ?g geo:asWKT ?w FILTER(geof:sfWithin(?w, ?w)) }").is_none());
        // A variable predicate is left alone, as the standard allows.
        assert!(rw("SELECT * WHERE { ?a ?p ?b }").is_none());
        // A property path is not a triple pattern.
        assert!(rw("SELECT * WHERE { ?a geo:sfWithin+ ?b }").is_none());
        assert!(!mentions_relation("SELECT * WHERE { ?s ?p ?o }"));
        // Function calls alone are not parsed again.
        assert!(!mentions_relation(&format!(
            "{PFX}SELECT * WHERE {{ ?g geo:asWKT ?w FILTER(geof:sfWithin(?w, ?w) && geof:rcc8tppi (?w, ?w)) }}"
        )));
        assert!(!mentions_relation(
            "SELECT * WHERE { FILTER(<http://www.opengis.net/def/function/geosparql/ehMeet>(?a, ?b)) }"
        ));
        assert!(mentions_relation(&format!(
            "{PFX}ASK {{ ?a geo:rcc8tpp ?b }}"
        )));
        assert!(mentions_relation(
            "ASK { ?a <http://www.opengis.net/ont/geosparql#ehMeet> ?b }"
        ));
    }

    #[test]
    fn a_relation_pattern_becomes_a_distinct_union() {
        let out = rw("SELECT ?f WHERE { ?f a ex:Park . ?f geo:sfWithin ?r }").unwrap();
        reparses(&out);
        assert!(out.contains("SELECT DISTINCT"), "{out}");
        assert!(out.contains("UNION"), "{out}");
        assert!(
            out.contains("<http://www.opengis.net/def/function/geosparql/sfWithin>"),
            "{out}"
        );
        assert!(
            out.contains("<http://www.opengis.net/ont/geosparql#hasDefaultGeometry>"),
            "{out}"
        );
        for s in SERIALISATIONS {
            assert!(out.contains(&format!("geosparql#{s}>")), "{s}: {out}");
        }
        // The other triple stays a plain pattern outside the sub-query.
        assert!(out.contains("<http://example.org/Park>"), "{out}");
    }

    #[test]
    fn every_relation_is_rewritten_to_its_own_function() {
        for rel in RELATIONS {
            let out = rw(&format!("ASK {{ ?a geo:{rel} ?b }}")).unwrap();
            reparses(&out);
            assert!(out.contains(&format!("geosparql/{rel}>(")), "{rel}: {out}");
        }
    }

    #[test]
    fn a_ground_pattern_becomes_filter_exists() {
        let out = rw("ASK { ex:A geo:sfWithin ex:B }").unwrap();
        reparses(&out);
        assert!(out.contains("EXISTS"), "{out}");
    }

    #[test]
    fn blank_nodes_become_variables_across_their_bgp() {
        let out = rw("SELECT ?n WHERE { _:x geo:sfWithin ?r . _:x ex:name ?n }").unwrap();
        reparses(&out);
        assert!(!out.contains("_:"), "{out}");
        assert!(out.matches("?__geor_b").count() >= 2, "{out}");
    }

    #[test]
    fn graph_exists_and_updates_are_rewritten() {
        let out = rw("SELECT * WHERE { GRAPH ?g { ?a geo:ehInside ?b } }").unwrap();
        reparses(&out);
        assert!(out.contains("GRAPH ?g"), "{out}");

        let out = rw("SELECT * WHERE { ?a a ex:T FILTER EXISTS { ?a geo:rcc8ntpp ?b } }").unwrap();
        reparses(&out);
        assert!(out.contains("rcc8ntpp>("), "{out}");

        let u = rewrite_update_text(&format!(
            "{PFX}INSERT {{ GRAPH ex:out {{ ?a geo:sfWithin ?b }} }} WHERE {{ ?a geo:sfWithin ?b }}"
        ))
        .unwrap();
        crate::sparql::parser()
            .parse_update(&u)
            .unwrap_or_else(|e| panic!("rewritten update does not parse: {e}\n{u}"));
        assert!(u.contains("sfWithin>("), "{u}");
        // Ground data carries no WHERE clause.
        assert!(
            rewrite_update_text(&format!("{PFX}INSERT DATA {{ ex:a geo:sfWithin ex:b }}"))
                .is_none()
        );
    }

    #[test]
    fn service_blocks_are_never_rewritten() {
        assert!(rw(
            "SELECT * WHERE { SERVICE <http://example.org/sparql> { ?a geo:sfWithin ?b } }"
        )
        .is_none());
    }

    #[test]
    fn the_switch_reads_off_values() {
        for (v, on) in [
            (None, true),
            (Some("off"), false),
            (Some("0"), false),
            (Some(" FALSE "), false),
            (Some("no"), false),
            (Some("on"), true),
        ] {
            assert_eq!(switch_value(v), on, "{v:?}");
        }
    }
}
