//! Matching literals by value under an entailment regime (D-entailment,
//! RDF 1.1 Semantics §7).
//!
//! The store keeps every literal exactly as written, so `"010"^^xsd:integer`
//! and `"10"^^xsd:integer` are different terms. Under D-entailment they
//! denote one value, and a graph holding either entails the triple with the
//! other — as do `"10"^^xsd:integer` and `"10.0"^^xsd:decimal`, or
//! `"1E400"^^xsd:double` and `"1E401"^^xsd:double` (both +∞). A materialiser
//! cannot write every lexical form of a value, so the regimes match by value
//! at query time instead: [`rewrite`] turns each literal constant of a
//! triple pattern whose datatype is in the datatype map into a fresh
//! variable that must hold the same value.
//!
//! - A value with finitely many forms (a string of the `xsd:string` family,
//!   a boolean) becomes a `VALUES` table of those forms, so the triple
//!   pattern keeps its index lookup.
//! - Any other value (numbers, floating point, date-times, binaries, URIs,
//!   XML literals) becomes `FILTER(<sameValue>(?v, literal))`.
//!
//! Value equality is [`super::datatypes`]'s: the XSD 1.1 value spaces as the
//! OWL 2 datatype map has them (`xsd:double` and `xsd:decimal` are disjoint,
//! so `"1"^^xsd:integer` never matches `"1.0E0"^^xsd:double`, although SPARQL
//! `=` says they are equal; `+0` and `−0` stay apart). Language-tagged
//! strings, literals of datatypes outside the map and ill-typed literals are
//! matched as terms.

use oxigraph::model::{Literal, NamedNode, Term};
use spargebra::algebra::{
    AggregateExpression, Expression, Function, GraphPattern, OrderExpression,
};
use spargebra::term::{GroundTerm, TermPattern, Variable};
use spargebra::Query;

use super::datatypes::{self, Dt, Value};

/// `sameValue(?a, ?b [, dt…])`: true when both are literals of one data
/// value; with datatype IRIs after them, only when `?a`'s datatype is one of
/// those. Registered with every evaluator (`store::engine`).
pub const SAME_VALUE: &str = "https://open-triplestore.org/def/function/rdf/sameValue";

/// The handler for [`SAME_VALUE`].
pub fn same_value(args: &[Term]) -> Option<Term> {
    let [a, b, recognized @ ..] = args else {
        return None;
    };
    let (Term::Literal(a), Term::Literal(b)) = (a, b) else {
        return Some(Literal::from(false).into());
    };
    if !recognized.is_empty()
        && !recognized
            .iter()
            .any(|d| matches!(d, Term::NamedNode(n) if n.as_str() == a.datatype().as_str()))
    {
        return Some(Literal::from(false).into());
    }
    let same = a == b
        || (datatypes::literal_value(a).is_some()
            && datatypes::value_key(a) == datatypes::value_key(b));
    Some(Literal::from(same).into())
}

/// `sparql` with every literal constant of a triple pattern matched by value,
/// or `None` when nothing changes (or it does not parse: the store reports
/// that). `recognized`: the datatypes whose values are known (the `D` of
/// D-entailment); `None` is the whole datatype map, which the server's
/// regimes use.
pub fn rewrite(sparql: &str, recognized: Option<&[String]>) -> Option<String> {
    // Cheap pre-check: a query without a quote, a digit or a boolean has no
    // literal constant to rewrite.
    if !sparql.contains(['"', '\''])
        && !sparql.contains(|c: char| c.is_ascii_digit())
        && !sparql.contains("true")
        && !sparql.contains("false")
    {
        return None;
    }
    let mut query = crate::sparql::parser().parse_query(sparql).ok()?;
    let mut rw = Rewriter {
        recognized,
        prefix: fresh_prefix(sparql),
        next: 0,
    };
    match &mut query {
        Query::Select { pattern, .. }
        | Query::Construct { pattern, .. }
        | Query::Describe { pattern, .. }
        | Query::Ask { pattern, .. } => rw.pattern(pattern),
    }
    (rw.next > 0).then(|| query.to_string())
}

fn fresh_prefix(sparql: &str) -> String {
    let mut prefix = "_ots_value".to_string();
    while sparql.contains(prefix.as_str()) {
        prefix.push('_');
    }
    prefix
}

/// How a constant is matched.
enum Match {
    /// Exactly these terms hold its value.
    Forms(Vec<Literal>),
    /// Infinitely many forms: test each candidate's value.
    Value,
}

struct Rewriter<'a> {
    recognized: Option<&'a [String]>,
    prefix: String,
    next: usize,
}

impl Rewriter<'_> {
    fn fresh(&mut self) -> Variable {
        self.next += 1;
        Variable::new(format!("{}{}", self.prefix, self.next)).expect("a valid variable name")
    }

    fn recognizes(&self, datatype: &str) -> bool {
        self.recognized
            .is_none_or(|d| d.iter().any(|r| r == datatype))
    }

    fn how(&self, lit: &Literal) -> Option<Match> {
        if lit.language().is_some() {
            return None;
        }
        let datatype = lit.datatype().as_str();
        Dt::from_any_iri(datatype)?;
        if !self.recognizes(datatype) {
            return None;
        }
        let value = datatypes::literal_value(lit)?;
        let forms: Vec<Literal> = match &value {
            Value::Str(s) => {
                let mut forms: Vec<Literal> = Dt::ANY
                    .iter()
                    .filter(|d| d.root() == Dt::PlainLiteral && **d != Dt::PlainLiteral)
                    .filter(|d| self.recognizes(d.iri()))
                    .filter(|d| datatypes::value_of(s, d.iri(), None).as_ref() == Some(&value))
                    .map(|d| {
                        Literal::new_typed_literal(s.clone(), NamedNode::new_unchecked(d.iri()))
                    })
                    .collect();
                let plain = Dt::PlainLiteral.iri();
                if self.recognizes(plain) {
                    forms.push(Literal::new_typed_literal(
                        format!("{s}@"),
                        NamedNode::new_unchecked(plain),
                    ));
                }
                forms
            }
            Value::Boolean(b) => {
                let lexicals: &[&str] = if *b { &["true", "1"] } else { &["false", "0"] };
                lexicals
                    .iter()
                    .map(|l| {
                        Literal::new_typed_literal(*l, NamedNode::new_unchecked(Dt::Boolean.iri()))
                    })
                    .collect()
            }
            _ => return Some(Match::Value),
        };
        if forms.len() <= 1 && forms.first() == Some(lit) {
            return None;
        }
        Some(Match::Forms(forms))
    }

    /// `object` as a fresh variable, with what binds or tests it, when it is
    /// a literal matched by value.
    fn object(&mut self, object: &mut TermPattern) -> Option<(Variable, Literal, Match)> {
        let TermPattern::Literal(lit) = object else {
            return None;
        };
        let how = self.how(lit)?;
        let lit = lit.clone();
        let v = self.fresh();
        *object = TermPattern::Variable(v.clone());
        Some((v, lit, how))
    }

    /// Wrap `inner` in the tables and filters its rewritten constants need.
    fn wrap(
        &self,
        mut inner: GraphPattern,
        matches: Vec<(Variable, Literal, Match)>,
    ) -> GraphPattern {
        let mut tests: Option<Expression> = None;
        for (v, lit, how) in matches {
            match how {
                Match::Forms(forms) => {
                    inner = GraphPattern::Join {
                        left: Box::new(GraphPattern::Values {
                            variables: vec![v],
                            bindings: forms
                                .into_iter()
                                .map(|f| vec![Some(GroundTerm::Literal(f))])
                                .collect(),
                        }),
                        right: Box::new(inner),
                    };
                }
                Match::Value => {
                    let mut args = vec![Expression::Variable(v), Expression::Literal(lit)];
                    if let Some(d) = self.recognized {
                        args.extend(
                            d.iter()
                                .filter_map(|i| NamedNode::new(i.clone()).ok())
                                .map(Expression::NamedNode),
                        );
                    }
                    let test = Expression::FunctionCall(
                        Function::Custom(NamedNode::new_unchecked(SAME_VALUE)),
                        args,
                    );
                    tests = Some(match tests {
                        Some(t) => Expression::And(Box::new(t), Box::new(test)),
                        None => test,
                    });
                }
            }
        }
        match tests {
            Some(expr) => GraphPattern::Filter {
                expr,
                inner: Box::new(inner),
            },
            None => inner,
        }
    }

    fn pattern(&mut self, p: &mut GraphPattern) {
        match p {
            GraphPattern::Bgp { patterns } => {
                let mut matches = Vec::new();
                for t in patterns.iter_mut() {
                    matches.extend(self.object(&mut t.object));
                }
                if !matches.is_empty() {
                    let bgp = std::mem::replace(p, GraphPattern::Bgp { patterns: vec![] });
                    *p = self.wrap(bgp, matches);
                }
            }
            GraphPattern::Path { object, .. } => {
                if let Some(m) = self.object(object) {
                    let path = std::mem::replace(p, GraphPattern::Bgp { patterns: vec![] });
                    *p = self.wrap(path, vec![m]);
                }
            }
            GraphPattern::Join { left, right }
            | GraphPattern::Union { left, right }
            | GraphPattern::Minus { left, right }
            | GraphPattern::Lateral { left, right } => {
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
            GraphPattern::Extend {
                inner, expression, ..
            } => {
                self.expression(expression);
                self.pattern(inner);
            }
            GraphPattern::OrderBy { inner, expression } => {
                for key in expression {
                    match key {
                        OrderExpression::Asc(e) | OrderExpression::Desc(e) => self.expression(e),
                    }
                }
                self.pattern(inner);
            }
            GraphPattern::Group {
                inner, aggregates, ..
            } => {
                for (_, aggregate) in aggregates {
                    if let AggregateExpression::FunctionCall { expr, .. } = aggregate {
                        self.expression(expr);
                    }
                }
                self.pattern(inner);
            }
            GraphPattern::Graph { inner, .. }
            | GraphPattern::Project { inner, .. }
            | GraphPattern::Distinct { inner }
            | GraphPattern::Reduced { inner }
            | GraphPattern::Slice { inner, .. } => self.pattern(inner),
            // A remote endpoint evaluates its own pattern.
            GraphPattern::Service { .. } | GraphPattern::Values { .. } => {}
        }
    }

    /// Descend into the `EXISTS` bodies of an expression.
    fn expression(&mut self, e: &mut Expression) {
        match e {
            Expression::Exists(p) => self.pattern(p),
            Expression::NamedNode(_)
            | Expression::Literal(_)
            | Expression::Variable(_)
            | Expression::Bound(_) => {}
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
            Expression::UnaryPlus(a) | Expression::UnaryMinus(a) | Expression::Not(a) => {
                self.expression(a)
            }
            Expression::In(a, list) => {
                self.expression(a);
                for x in list {
                    self.expression(x);
                }
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::TripleStore;
    use oxigraph::io::RdfFormat;

    fn store(ttl: &str) -> TripleStore {
        let s = TripleStore::in_memory().unwrap();
        s.load_str(ttl, RdfFormat::Turtle, None).unwrap();
        s
    }

    fn ask(s: &TripleStore, q: &str, recognized: Option<&[String]>) -> bool {
        let q = rewrite(q, recognized).unwrap_or_else(|| q.to_string());
        match s.query(&q).unwrap() {
            oxigraph::sparql::QueryResults::Boolean(b) => b,
            _ => panic!("not an ASK"),
        }
    }

    const P: &str =
        "PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> PREFIX : <http://example.org/> ";

    #[test]
    fn equal_values_match_across_lexical_forms_and_datatypes() {
        let s = store(&format!(
            "{P} :a :int \"010\"^^xsd:integer ; :dbl \"1E400\"^^xsd:double ; \
             :flt \"16777206.5\"^^xsd:float ; :tok \"abc\"^^xsd:token ; :b \"1\"^^xsd:boolean ."
        ));
        for q in [
            "ASK { :a :int 10 }",
            "ASK { :a :int \"10.0\"^^xsd:decimal }",
            "ASK { :a :int \"+10\"^^xsd:int }",
            "ASK { :a :dbl \"1E401\"^^xsd:double }",
            "ASK { :a :dbl \"INF\"^^xsd:double }",
            "ASK { :a :flt \"16777205.5\"^^xsd:float }",
            "ASK { :a :tok \"abc\" }",
            "ASK { :a :b true }",
            "ASK { FILTER EXISTS { :a :int 10 } }",
            "ASK { OPTIONAL { :a :int 10 } :a :int ?x }",
            "ASK { :a :int/^:int/:int 10 }",
        ] {
            assert!(ask(&s, &format!("{P}{q}"), None), "{q}");
        }
        for q in [
            // Disjoint value spaces: a decimal is no double.
            "ASK { :a :int \"10\"^^xsd:double }",
            "ASK { :a :int 11 }",
            "ASK { :a :dbl \"-INF\"^^xsd:double }",
            "ASK { :a :flt \"16777207.5\"^^xsd:float }",
            "ASK { :a :b false }",
            "ASK { :a :tok \"abc\"@en }",
        ] {
            assert!(!ask(&s, &format!("{P}{q}"), None), "{q}");
        }
    }

    #[test]
    fn only_recognized_datatypes_match_by_value() {
        let s = store(
            "<http://example.org/a> <http://example.org/p> \"010\"^^<http://www.w3.org/2001/XMLSchema#integer> .",
        );
        let q = format!("{P}ASK {{ :a :p 10 }}");
        let int = ["http://www.w3.org/2001/XMLSchema#integer".to_string()];
        assert!(ask(&s, &q, Some(&int)));
        assert!(!ask(&s, &q, Some(&[])));
    }

    #[test]
    fn fresh_variables_stay_out_of_the_results() {
        let q = format!("{P}SELECT * WHERE {{ ?s :p 10 }}");
        let out = rewrite(&q, None).unwrap();
        let parsed = crate::sparql::parser().parse_query(&out).unwrap();
        let Query::Select { pattern, .. } = parsed else {
            panic!()
        };
        let GraphPattern::Project { variables, .. } = pattern else {
            panic!("{out}")
        };
        assert_eq!(variables, vec![Variable::new_unchecked("s")]);
        // Nothing to match by value: no rewrite.
        assert!(rewrite(&format!("{P}SELECT * {{ ?s :p :o }}"), None).is_none());
        assert!(rewrite(&format!("{P}SELECT * {{ ?s :p \"x\"@en }}"), None).is_none());
    }
}
