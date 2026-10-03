//! OPM calculations: `opm:Calculation` and the derived states it writes.
//!
//! A calculation names the property it infers (`opm:inferredProperty`), the
//! argument paths from a feature of interest to each argument
//! (`opm:argumentPaths`, e.g. `?foi ex:width ?w`), an expression over the
//! arguments (`opm:expression`, e.g. `?w * ?h`) and optional restrictions
//! (`opm:foiRestriction`, `opm:pathRestriction`). As the OPM specification
//! describes, it runs only on request:
//!
//! * `POST` derives the property for every feature of interest that has the
//!   arguments and does not have the property yet;
//! * `PUT` recomputes every derived state of the calculation one of whose
//!   argument states has since been outdated;
//! * `GET …/outdated` lists those.
//!
//! A derived state is an `opm:Derived` state with the `opm:expression` and a
//! `prov:wasDerivedFrom` `rdf:Seq` of the argument states, written through the
//! same state path as every other state, all of one run in one commit.
//!
//! **Security.** Paths, restrictions and expressions come from data any
//! writer of a dataset can store (and an import can bring), so none of their
//! text reaches the store. Each is parsed with spargebra and allow-listed:
//! a path or restriction may hold only triple patterns and property paths —
//! no `SERVICE`, `GRAPH`, `FILTER`, `EXISTS`, `OPTIONAL`, `UNION`, `VALUES`,
//! `BIND`, subqueries or solution modifiers — and only `?foi` plus its one
//! argument variable; an expression only arithmetic, comparison, logic and a
//! short list of deterministic built-ins over the declared arguments. The
//! queries are then built from the parsed algebra: matching runs confined to
//! the dataset's own data graphs with a row cap and a deadline, and the
//! expression is evaluated over `VALUES` in an empty scratch store, so it
//! cannot read anything at all. A stored calculation is re-validated every
//! time it is loaded.

use std::collections::{BTreeMap, HashMap, HashSet};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use oxigraph::model::{Literal, NamedNode, Term};
use serde::Deserialize;
use spargebra::algebra::{Expression, Function, GraphPattern, PropertyPathExpression};
use spargebra::term::{GroundTerm, NamedNodePattern, TermPattern, TriplePattern, Variable};
use spargebra::{Query, SparqlParser};

use super::*;

/// Most argument paths of one calculation.
const MAX_ARGS: usize = 16;
/// Longest path, restriction or expression text.
const MAX_TEXT: usize = 4096;
/// Deepest expression nesting.
const MAX_DEPTH: usize = 64;
/// Calculations listed per dataset.
const MAX_CALCULATIONS: usize = 1000;

/// Most rows a matching query may produce (`OTS_OPM_CALC_MAX_ROWS`, default
/// 10 000); more fails the run rather than deriving part of it.
fn max_rows() -> usize {
    std::env::var("OTS_OPM_CALC_MAX_ROWS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n: &usize| *n > 0)
        .unwrap_or(10_000)
}

const NUMERIC: [&str; 16] = [
    "integer",
    "decimal",
    "float",
    "double",
    "long",
    "int",
    "short",
    "byte",
    "nonNegativeInteger",
    "positiveInteger",
    "negativeInteger",
    "nonPositiveInteger",
    "unsignedLong",
    "unsignedInt",
    "unsignedShort",
    "unsignedByte",
];

fn is_numeric(t: &Term) -> bool {
    matches!(t, Term::Literal(l) if l.datatype().as_str().strip_prefix(XSD).is_some_and(|d| NUMERIC.contains(&d)))
}

// ─── Parsing and allow-listing ───────────────────────────────────────────────

/// One element of a parsed path: a triple pattern or a property path.
#[derive(Clone, Debug)]
enum Piece {
    Triple(TriplePattern),
    Path {
        subject: TermPattern,
        path: PropertyPathExpression,
        object: TermPattern,
    },
}

impl Piece {
    fn terms(&self) -> [&TermPattern; 2] {
        match self {
            Piece::Triple(t) => [&t.subject, &t.object],
            Piece::Path {
                subject, object, ..
            } => [subject, object],
        }
    }
    fn text(&self) -> String {
        match self {
            Piece::Triple(t) => t.to_string(),
            Piece::Path {
                subject,
                path,
                object,
            } => format!("{subject} {path} {object}"),
        }
    }
}

fn parser(prefixes: &[(String, String)]) -> Result<SparqlParser, String> {
    let mut p = SparqlParser::new();
    for (name, ns) in prefixes {
        p = p
            .with_prefix(name.as_str(), ns.as_str())
            .map_err(|e| format!("prefix `{name}`: {e}"))?;
    }
    Ok(p)
}

fn pattern_kind(gp: &GraphPattern) -> &'static str {
    match gp {
        GraphPattern::Filter { .. } => "FILTER",
        GraphPattern::Union { .. } => "UNION",
        GraphPattern::Graph { .. } => "GRAPH",
        GraphPattern::Service { .. } => "SERVICE",
        GraphPattern::LeftJoin { .. } => "OPTIONAL",
        GraphPattern::Extend { .. } => "BIND",
        GraphPattern::Values { .. } => "VALUES",
        GraphPattern::Minus { .. } => "MINUS",
        _ => "a subquery, solution modifier or other pattern",
    }
}

fn check_term(t: &TermPattern, what: &str) -> Result<(), String> {
    match t {
        TermPattern::Variable(_) | TermPattern::BlankNode(_) | TermPattern::NamedNode(_) => Ok(()),
        TermPattern::Literal(_) => Ok(()),
        #[allow(unreachable_patterns)]
        _ => Err(format!("{what}: quoted triples are not allowed")),
    }
}

fn flatten(gp: GraphPattern, out: &mut Vec<Piece>, what: &str) -> Result<(), String> {
    match gp {
        GraphPattern::Bgp { patterns } => {
            for t in patterns {
                check_term(&t.subject, what)?;
                check_term(&t.object, what)?;
                if !matches!(t.predicate, NamedNodePattern::NamedNode(_)) {
                    return Err(format!("{what}: the predicate of `{t}` must be an IRI"));
                }
                out.push(Piece::Triple(t));
            }
            Ok(())
        }
        GraphPattern::Path {
            subject,
            path,
            object,
        } => {
            check_term(&subject, what)?;
            check_term(&object, what)?;
            out.push(Piece::Path {
                subject,
                path,
                object,
            });
            Ok(())
        }
        GraphPattern::Join { left, right } => {
            flatten(*left, out, what)?;
            flatten(*right, out, what)
        }
        other => Err(format!(
            "{what} may hold only triple patterns and property paths, not {}",
            pattern_kind(&other)
        )),
    }
}

/// Parse `text` as the body of `SELECT * WHERE { … }` and accept it only
/// if it is triple patterns and property paths.
fn parse_pieces(
    text: &str,
    prefixes: &[(String, String)],
    what: &str,
) -> Result<Vec<Piece>, String> {
    if text.len() > MAX_TEXT {
        return Err(format!("{what} is longer than {MAX_TEXT} characters"));
    }
    let q = parser(prefixes)?
        .parse_query(&format!("SELECT * WHERE {{ {text}\n}}"))
        .map_err(|e| format!("{what} `{text}` does not parse: {e}"))?;
    let Query::Select {
        dataset: None,
        pattern: GraphPattern::Project { inner, .. },
        ..
    } = q
    else {
        return Err(format!(
            "{what} may hold only triple patterns and property paths"
        ));
    };
    let mut out = Vec::new();
    flatten(*inner, &mut out, what)?;
    if out.is_empty() {
        return Err(format!("{what} is empty"));
    }
    Ok(out)
}

fn variables(pieces: &[Piece]) -> HashSet<Variable> {
    pieces
        .iter()
        .flat_map(|p| p.terms())
        .filter_map(|t| match t {
            TermPattern::Variable(v) => Some(v.clone()),
            _ => None,
        })
        .collect()
}

fn foi() -> Variable {
    Variable::new_unchecked("foi")
}

/// One parsed argument path.
#[derive(Clone, Debug)]
pub(crate) struct ArgPath {
    pub var: Variable,
    pieces: Vec<Piece>,
    /// The subject of the step that binds the argument: whose property it is.
    owner: TermPattern,
    /// That step's predicate: which property.
    pub predicate: NamedNode,
    pub text: String,
}

fn parse_arg_path(text: &str, prefixes: &[(String, String)]) -> Result<ArgPath, String> {
    let what = format!("argument path `{text}`");
    let pieces = parse_pieces(text, prefixes, "an argument path")?;
    let vars = variables(&pieces);
    if !vars.contains(&foi()) {
        return Err(format!("{what} must start at ?foi"));
    }
    let others: Vec<&Variable> = vars.iter().filter(|v| **v != foi()).collect();
    let [var] = others.as_slice() else {
        return Err(format!(
            "{what} must bind exactly one variable besides ?foi (the argument), not {}",
            others.len()
        ));
    };
    let var = (*var).clone();
    if var.as_str().starts_with("__") {
        return Err(format!(
            "{what}: variable names starting with `__` are reserved"
        ));
    }
    let occurrences = pieces
        .iter()
        .flat_map(|p| p.terms())
        .filter(|t| **t == TermPattern::Variable(var.clone()))
        .count();
    let binding: Vec<&TriplePattern> = pieces
        .iter()
        .filter_map(|p| match p {
            Piece::Triple(t) if t.object == TermPattern::Variable(var.clone()) => Some(t),
            _ => None,
        })
        .collect();
    let ([t], 1) = (binding.as_slice(), occurrences) else {
        return Err(format!(
            "{what}: the argument ?{} must appear once, as the object of the path's last step, and that step must be a plain predicate",
            var.as_str()
        ));
    };
    let NamedNodePattern::NamedNode(predicate) = &t.predicate else {
        return Err(format!("{what}: the last step must be an IRI"));
    };
    if matches!(t.subject, TermPattern::Literal(_)) {
        return Err(format!("{what}: a literal has no properties"));
    }
    Ok(ArgPath {
        var,
        owner: t.subject.clone(),
        predicate: predicate.clone(),
        text: pieces
            .iter()
            .map(Piece::text)
            .collect::<Vec<_>>()
            .join(" . "),
        pieces,
    })
}

fn parse_path_restriction(
    text: &str,
    prefixes: &[(String, String)],
) -> Result<(Vec<Piece>, String), String> {
    let pieces = parse_pieces(text, prefixes, "the path restriction")?;
    let vars = variables(&pieces);
    if !vars.contains(&foi()) || vars.len() != 1 {
        return Err(format!(
            "the path restriction `{text}` may use ?foi and no other variable (blank nodes are fine)"
        ));
    }
    let canonical = pieces
        .iter()
        .map(Piece::text)
        .collect::<Vec<_>>()
        .join(" . ");
    Ok((pieces, canonical))
}

fn allowed_function(f: &Function) -> Result<(), String> {
    match f {
        Function::Abs
        | Function::Ceil
        | Function::Floor
        | Function::Round
        | Function::IsNumeric
        | Function::IsLiteral
        | Function::Str
        | Function::Datatype => Ok(()),
        Function::Custom(n)
            if n.as_str()
                .strip_prefix(XSD)
                .is_some_and(|l| NUMERIC.contains(&l) || l == "boolean" || l == "string") =>
        {
            Ok(())
        }
        other => Err(format!(
            "function {other} is not allowed in an expression (allowed: ABS, CEIL, FLOOR, ROUND, isNumeric, isLiteral, STR, DATATYPE, IF, COALESCE, BOUND and XSD numeric, boolean and string casts)"
        )),
    }
}

fn check_expr(e: &Expression, args: &HashSet<Variable>, depth: usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("the expression is nested too deeply".to_string());
    }
    let rec = |x: &Expression| check_expr(x, args, depth + 1);
    match e {
        Expression::NamedNode(_) | Expression::Literal(_) => Ok(()),
        Expression::Variable(v) | Expression::Bound(v) => {
            if args.contains(v) {
                Ok(())
            } else {
                Err(format!(
                    "?{} is not an argument of the calculation",
                    v.as_str()
                ))
            }
        }
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
            rec(a)?;
            rec(b)
        }
        Expression::UnaryPlus(a) | Expression::UnaryMinus(a) | Expression::Not(a) => rec(a),
        Expression::In(a, list) => {
            rec(a)?;
            list.iter().try_for_each(rec)
        }
        Expression::If(a, b, c) => {
            rec(a)?;
            rec(b)?;
            rec(c)
        }
        Expression::Coalesce(list) => list.iter().try_for_each(rec),
        Expression::FunctionCall(f, list) => {
            allowed_function(f)?;
            list.iter().try_for_each(rec)
        }
        Expression::Exists(_) => Err("EXISTS is not allowed in an expression".to_string()),
    }
}

/// The expression as fully parenthesised SPARQL. (spargebra's own Display
/// drops the parentheses around `+` and `-`, which changes `(a + b) * c`.)
fn expr_text(e: &Expression) -> String {
    let bin = |a: &Expression, op: &str, b: &Expression| {
        format!("({} {op} {})", expr_text(a), expr_text(b))
    };
    let list = |l: &[Expression]| l.iter().map(expr_text).collect::<Vec<_>>().join(", ");
    match e {
        Expression::NamedNode(n) => n.to_string(),
        Expression::Literal(l) => l.to_string(),
        Expression::Variable(v) => v.to_string(),
        Expression::Or(a, b) => bin(a, "||", b),
        Expression::And(a, b) => bin(a, "&&", b),
        Expression::Equal(a, b) => bin(a, "=", b),
        Expression::SameTerm(a, b) => format!("sameTerm({}, {})", expr_text(a), expr_text(b)),
        Expression::Greater(a, b) => bin(a, ">", b),
        Expression::GreaterOrEqual(a, b) => bin(a, ">=", b),
        Expression::Less(a, b) => bin(a, "<", b),
        Expression::LessOrEqual(a, b) => bin(a, "<=", b),
        Expression::Add(a, b) => bin(a, "+", b),
        Expression::Subtract(a, b) => bin(a, "-", b),
        Expression::Multiply(a, b) => bin(a, "*", b),
        Expression::Divide(a, b) => bin(a, "/", b),
        Expression::UnaryPlus(a) => format!("(+{})", expr_text(a)),
        Expression::UnaryMinus(a) => format!("(-{})", expr_text(a)),
        Expression::Not(a) => format!("(!{})", expr_text(a)),
        Expression::In(a, l) => format!("({} IN ({}))", expr_text(a), list(l)),
        Expression::If(a, b, c) => {
            format!("IF({}, {}, {})", expr_text(a), expr_text(b), expr_text(c))
        }
        Expression::Coalesce(l) => format!("COALESCE({})", list(l)),
        Expression::Bound(v) => format!("BOUND({v})"),
        Expression::FunctionCall(f, l) => format!("{f}({})", list(l)),
        Expression::Exists(_) => String::new(),
    }
}

fn parse_expression(
    text: &str,
    prefixes: &[(String, String)],
    args: &HashSet<Variable>,
) -> Result<(Expression, String), String> {
    if text.len() > MAX_TEXT {
        return Err(format!(
            "the expression is longer than {MAX_TEXT} characters"
        ));
    }
    let result = Variable::new_unchecked("__result");
    let q = parser(prefixes)?
        .parse_query(&format!("SELECT ({text}\n AS ?__result) WHERE {{}}"))
        .map_err(|e| format!("the expression `{text}` does not parse: {e}"))?;
    let Query::Select {
        dataset: None,
        pattern: GraphPattern::Project { inner, variables },
        ..
    } = q
    else {
        return Err(format!("`{text}` is not a single expression"));
    };
    let GraphPattern::Extend {
        inner,
        variable,
        expression,
    } = *inner
    else {
        return Err(format!("`{text}` is not a single expression"));
    };
    if variables != [result.clone()]
        || variable != result
        || *inner != (GraphPattern::Bgp { patterns: vec![] })
    {
        return Err(format!("`{text}` is not a single expression"));
    }
    check_expr(&expression, args, 0)?;
    let canonical = expr_text(&expression);
    Ok((expression, canonical))
}

/// A parsed, allow-listed calculation.
#[derive(Clone, Debug)]
pub(crate) struct Calc {
    pub iri: String,
    pub label: Option<String>,
    pub inferred: NamedNode,
    pub args: Vec<ArgPath>,
    pub expression: Expression,
    pub expression_text: String,
    pub foi_restriction: Option<NamedNode>,
    path_restriction: Option<(Vec<Piece>, String)>,
    pub data_graph: Option<String>,
}

/// The definition as given (by a caller, an import or the store).
#[derive(Clone, Debug, Default, Deserialize)]
pub struct CalcBody {
    pub label: Option<String>,
    pub inferred_property: String,
    pub argument_paths: Vec<String>,
    pub expression: String,
    /// Prefixes the paths and the expression may use (`{"ex": "https://…"}`).
    #[serde(default)]
    pub prefixes: BTreeMap<String, String>,
    pub foi_restriction: Option<String>,
    pub path_restriction: Option<String>,
    /// The data graph derived values go to (default: where the property's
    /// value already is, else the dataset's instances graph).
    pub graph: Option<String>,
}

pub(crate) fn build_calc(iri: &str, body: &CalcBody) -> Result<Calc, String> {
    let prefixes: Vec<(String, String)> = body
        .prefixes
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let expand = |s: &str, what: &str| -> Result<NamedNode, String> {
        if let Ok(n) = NamedNode::new(s) {
            if s.contains(':')
                && !prefixes
                    .iter()
                    .any(|(p, _)| s.starts_with(&format!("{p}:")))
            {
                return Ok(n);
            }
        }
        if let Some((p, local)) = s.split_once(':') {
            if let Some((_, ns)) = prefixes.iter().find(|(name, _)| name == p) {
                return NamedNode::new(format!("{ns}{local}")).map_err(|e| format!("{what}: {e}"));
            }
        }
        NamedNode::new(s).map_err(|e| format!("{what} `{s}` is not an IRI: {e}"))
    };
    let inferred = expand(&body.inferred_property, "inferred_property")?;
    if body.argument_paths.is_empty() || body.argument_paths.len() > MAX_ARGS {
        return Err(format!("a calculation has 1 to {MAX_ARGS} argument paths"));
    }
    let mut args = Vec::new();
    for text in &body.argument_paths {
        let a = parse_arg_path(text, &prefixes)?;
        if args.iter().any(|b: &ArgPath| b.var == a.var) {
            return Err(format!("two argument paths bind ?{}", a.var.as_str()));
        }
        args.push(a);
    }
    let vars: HashSet<Variable> = args.iter().map(|a| a.var.clone()).collect();
    let (expression, expression_text) = parse_expression(&body.expression, &prefixes, &vars)?;
    let foi_restriction = body
        .foi_restriction
        .as_deref()
        .map(|s| expand(s, "foi_restriction"))
        .transpose()?;
    let path_restriction = body
        .path_restriction
        .as_deref()
        .map(|s| parse_path_restriction(s, &prefixes))
        .transpose()?;
    Ok(Calc {
        iri: iri.to_string(),
        label: body.label.clone(),
        inferred,
        args,
        expression,
        expression_text,
        foi_restriction,
        path_restriction,
        data_graph: body.graph.clone(),
    })
}

impl Calc {
    /// The definition in its canonical form: full IRIs, no prefixes.
    fn canonical_body(&self) -> CalcBody {
        CalcBody {
            label: self.label.clone(),
            inferred_property: self.inferred.as_str().to_string(),
            argument_paths: self.args.iter().map(|a| a.text.clone()).collect(),
            expression: self.expression_text.clone(),
            prefixes: BTreeMap::new(),
            foi_restriction: self
                .foi_restriction
                .as_ref()
                .map(|n| n.as_str().to_string()),
            path_restriction: self.path_restriction.as_ref().map(|(_, t)| t.clone()),
            graph: self.data_graph.clone(),
        }
    }

    fn view(&self) -> serde_json::Value {
        let b = self.canonical_body();
        serde_json::json!({
            "calculation": self.iri,
            "id": calc_id(&self.iri),
            "label": b.label,
            "inferred_property": b.inferred_property,
            "argument_paths": b.argument_paths,
            "arguments": self.args.iter().map(|a| format!("?{}", a.var.as_str())).collect::<Vec<_>>(),
            "expression": b.expression,
            "foi_restriction": b.foi_restriction,
            "path_restriction": b.path_restriction,
            "graph": b.graph,
            "valid": true,
        })
    }

    /// The SELECT that finds every feature of interest with its argument
    /// owners and values, built from the parsed form: projected `?foi`, then
    /// per argument `i` the owner `?__o{i}` (when it is a variable) and the
    /// value `?__v{i}`.
    fn matching_query(&self, only: Option<&[NamedNode]>) -> Query {
        let foi = foi();
        let mut triples: Vec<TriplePattern> = Vec::new();
        let mut paths: Vec<GraphPattern> = Vec::new();
        let mut project = vec![foi.clone()];
        let mut counter = 0usize;
        let mut add = |pieces: &[Piece], rename: &mut dyn FnMut(&TermPattern) -> TermPattern| {
            for p in pieces {
                match p {
                    Piece::Triple(t) => triples.push(TriplePattern {
                        subject: rename(&t.subject),
                        predicate: t.predicate.clone(),
                        object: rename(&t.object),
                    }),
                    Piece::Path {
                        subject,
                        path,
                        object,
                    } => paths.push(GraphPattern::Path {
                        subject: rename(subject),
                        path: path.clone(),
                        object: rename(object),
                    }),
                }
            }
        };
        for (i, a) in self.args.iter().enumerate() {
            let mut blanks: HashMap<String, Variable> = HashMap::new();
            let value = Variable::new_unchecked(format!("__v{i}"));
            let owner = Variable::new_unchecked(format!("__o{i}"));
            let arg_var = a.var.clone();
            let owner_blank = match &a.owner {
                TermPattern::BlankNode(b) => Some(b.as_str().to_string()),
                _ => None,
            };
            let mut rename = |t: &TermPattern| -> TermPattern {
                match t {
                    TermPattern::BlankNode(b) if owner_blank.as_deref() == Some(b.as_str()) => {
                        TermPattern::Variable(owner.clone())
                    }
                    TermPattern::BlankNode(b) => TermPattern::Variable(
                        blanks
                            .entry(b.as_str().to_string())
                            .or_insert_with(|| {
                                counter += 1;
                                Variable::new_unchecked(format!("__b{counter}"))
                            })
                            .clone(),
                    ),
                    TermPattern::Variable(v) if *v == arg_var => {
                        TermPattern::Variable(value.clone())
                    }
                    other => other.clone(),
                }
            };
            add(&a.pieces, &mut rename);
            if matches!(a.owner, TermPattern::BlankNode(_)) {
                project.push(owner);
            }
            project.push(value);
        }
        if let Some((pieces, _)) = &self.path_restriction {
            let mut blanks: HashMap<String, Variable> = HashMap::new();
            let mut rename = |t: &TermPattern| -> TermPattern {
                match t {
                    TermPattern::BlankNode(b) => TermPattern::Variable(
                        blanks
                            .entry(b.as_str().to_string())
                            .or_insert_with(|| {
                                counter += 1;
                                Variable::new_unchecked(format!("__r{counter}"))
                            })
                            .clone(),
                    ),
                    other => other.clone(),
                }
            };
            add(pieces, &mut rename);
        }
        let mut pattern = GraphPattern::Bgp { patterns: triples };
        for p in paths {
            pattern = GraphPattern::Join {
                left: Box::new(pattern),
                right: Box::new(p),
            };
        }
        let mut fois: Vec<NamedNode> = Vec::new();
        if let Some(f) = &self.foi_restriction {
            fois.push(f.clone());
        }
        if let Some(only) = only {
            if self.foi_restriction.is_some() {
                fois.retain(|f| only.contains(f));
            } else {
                fois.extend(only.iter().cloned());
            }
        }
        if self.foi_restriction.is_some() || only.is_some() {
            pattern = GraphPattern::Join {
                left: Box::new(GraphPattern::Values {
                    variables: vec![foi.clone()],
                    bindings: fois
                        .into_iter()
                        .map(|f| vec![Some(GroundTerm::NamedNode(f))])
                        .collect(),
                }),
                right: Box::new(pattern),
            };
        }
        let pattern = GraphPattern::Filter {
            expr: Expression::FunctionCall(Function::IsIri, vec![Expression::Variable(foi)]),
            inner: Box::new(pattern),
        };
        Query::Select {
            dataset: None,
            pattern: GraphPattern::Distinct {
                inner: Box::new(GraphPattern::Project {
                    inner: Box::new(pattern),
                    variables: project,
                }),
            },
            base_iri: None,
        }
    }
}

/// Evaluate the expression over each row's argument values, in an empty
/// scratch store: `SELECT ?__row (expr AS ?__result) { VALUES … }`.
fn evaluate(
    calc: &Calc,
    rows: &[Vec<Term>],
    timeout: std::time::Duration,
) -> Result<Vec<Option<Term>>, String> {
    let row_var = Variable::new_unchecked("__row");
    let result = Variable::new_unchecked("__result");
    let mut out = vec![None; rows.len()];
    let scratch = oxigraph::store::Store::new().map_err(|e| e.to_string())?;
    for (chunk_index, chunk) in rows.chunks(1000).enumerate() {
        let mut variables = vec![row_var.clone()];
        variables.extend(calc.args.iter().map(|a| a.var.clone()));
        let bindings = chunk
            .iter()
            .enumerate()
            .map(|(i, values)| {
                let mut row = vec![Some(GroundTerm::Literal(Literal::from(
                    (chunk_index * 1000 + i) as i64,
                )))];
                row.extend(values.iter().map(|v| match v {
                    Term::Literal(l) => Some(GroundTerm::Literal(l.clone())),
                    Term::NamedNode(n) => Some(GroundTerm::NamedNode(n.clone())),
                    _ => None,
                }));
                row
            })
            .collect();
        let query = Query::Select {
            dataset: None,
            pattern: GraphPattern::Project {
                inner: Box::new(GraphPattern::Extend {
                    inner: Box::new(GraphPattern::Values {
                        variables,
                        bindings,
                    }),
                    variable: result.clone(),
                    expression: calc.expression.clone(),
                }),
                variables: vec![row_var.clone(), result.clone()],
            },
            base_iri: None,
        };
        let token = oxigraph::sparql::CancellationToken::new();
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let (done, token) = (done.clone(), token.clone());
            let deadline = std::time::Instant::now() + timeout;
            std::thread::spawn(move || {
                while !done.load(std::sync::atomic::Ordering::Relaxed) {
                    if std::time::Instant::now() >= deadline {
                        token.cancel();
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            });
        }
        let run = (|| -> Result<(), String> {
            let results = oxigraph::sparql::SparqlEvaluator::new()
                .with_cancellation_token(token)
                .for_query(query)
                .on_store(&scratch)
                .execute()
                .map_err(|e| e.to_string())?;
            if let oxigraph::sparql::QueryResults::Solutions(solutions) = results {
                for s in solutions {
                    let s = s.map_err(|e| e.to_string())?;
                    let Some(Term::Literal(i)) = s.get("__row") else {
                        continue;
                    };
                    let Ok(i) = i.value().parse::<usize>() else {
                        continue;
                    };
                    if i < out.len() {
                        out[i] = s.get("__result").cloned();
                    }
                }
            }
            Ok(())
        })();
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        run?;
    }
    Ok(out)
}

// ─── Storage ─────────────────────────────────────────────────────────────────

const CALC_PREFIX: &str = "urn:ots:calculation:";

fn calc_id(iri: &str) -> String {
    iri.strip_prefix(CALC_PREFIX).unwrap_or(iri).to_string()
}

fn calc_iri(id: &str) -> Result<String, ApiErr> {
    let iri = if id.contains(':') {
        id.to_string()
    } else {
        format!("{CALC_PREFIX}{id}")
    };
    NamedNode::new(&iri).map_err(|e| bad(format!("calculation: {e}")))?;
    Ok(iri)
}

/// The INSERT DATA statement storing `calc` in the states graph.
pub(crate) fn calc_insert(states: &str, calc: &Calc, agent: &str, now: &str) -> String {
    let lit = |s: &str| format!("\"{}\"", escape_sparql_literal(s));
    let c = escape_sparql_iri(&calc.iri);
    let mut lines = vec![
        format!("<{c}> a opm:Calculation"),
        format!(
            "<{c}> opm:inferredProperty <{}>",
            escape_sparql_iri(calc.inferred.as_str())
        ),
        format!("<{c}> opm:expression {}", lit(&calc.expression_text)),
        format!("<{c}> prov:generatedAtTime \"{now}\"^^xsd:dateTime"),
        format!("<{c}> prov:wasAttributedTo <{}>", escape_sparql_iri(agent)),
    ];
    if let Some(l) = &calc.label {
        lines.push(format!("<{c}> rdfs:label {}", lit(l)));
    }
    if let Some(f) = &calc.foi_restriction {
        lines.push(format!(
            "<{c}> opm:foiRestriction <{}>",
            escape_sparql_iri(f.as_str())
        ));
    }
    if let Some((_, t)) = &calc.path_restriction {
        lines.push(format!("<{c}> opm:pathRestriction {}", lit(t)));
    }
    if let Some(g) = &calc.data_graph {
        lines.push(format!("<{c}> ots:dataGraph <{}>", escape_sparql_iri(g)));
    }
    let list = format!("urn:ots:calculation-arguments:{}", uuid::Uuid::new_v4());
    let cell = |i: usize| format!("{list}:{i}");
    lines.push(format!("<{c}> opm:argumentPaths <{}>", cell(1)));
    for (i, a) in calc.args.iter().enumerate() {
        let n = i + 1;
        let rest = if n == calc.args.len() {
            "rdf:nil".to_string()
        } else {
            format!("<{}>", cell(n + 1))
        };
        lines.push(format!(
            "<{}> rdf:first {} ; rdf:rest {rest}",
            cell(n),
            lit(&a.text)
        ));
    }
    format!(
        "{}\nINSERT DATA {{ GRAPH <{}> {{\n  {} .\n}} }}",
        prefixes_block(),
        escape_sparql_iri(states),
        lines.join(" .\n  ")
    )
}

pub(crate) fn prefixes_block() -> String {
    format!(
        "PREFIX opm: <{OPM}>\nPREFIX schema: <{SCHEMA}>\nPREFIX prov: <{PROV}>\nPREFIX ots: <{OTS}>\nPREFIX xsd: <{XSD}>\nPREFIX rdfs: <{RDFS}>\nPREFIX rdf: <{RDF}>"
    )
}

/// The statement removing `iri` and its argument list from the states graph.
fn calc_delete(states: &str, iri: &str) -> String {
    let sg = escape_sparql_iri(states);
    let c = escape_sparql_iri(iri);
    format!(
        "{p}\nDELETE {{ GRAPH <{sg}> {{ ?cell ?cp ?co }} }} WHERE {{ GRAPH <{sg}> {{ <{c}> opm:argumentPaths ?head . ?head rdf:rest* ?cell . ?cell ?cp ?co }} }} ;\nDELETE WHERE {{ GRAPH <{sg}> {{ <{c}> ?p ?o }} }}",
        p = prefixes_block()
    )
}

/// A stored calculation, re-validated: `Err` carries why it is unusable.
pub(crate) struct Stored {
    pub iri: String,
    pub calc: Result<Calc, String>,
    pub body: CalcBody,
}

fn calcs_select() -> String {
    format!(
        r#"PREFIX opm: <{OPM}> PREFIX ots: <{OTS}> PREFIX rdfs: <{RDFS}>
SELECT ?c ?label ?inf ?expr ?foi ?pr ?dg ?head WHERE {{
  ?c a opm:Calculation .
  OPTIONAL {{ ?c opm:inferredProperty ?inf }}
  OPTIONAL {{ ?c opm:expression ?expr }}
  OPTIONAL {{ ?c opm:argumentPaths ?head }}
  OPTIONAL {{ ?c rdfs:label ?label }}
  OPTIONAL {{ ?c opm:foiRestriction ?foi }}
  OPTIONAL {{ ?c opm:pathRestriction ?pr }}
  OPTIONAL {{ ?c ots:dataGraph ?dg }}
}}"#
    )
}

fn cells_select() -> String {
    format!(
        "PREFIX opm: <{OPM}> PREFIX rdf: <{RDF}> SELECT ?c ?cell ?first ?rest WHERE {{ ?c opm:argumentPaths ?head . ?head rdf:rest* ?cell . ?cell rdf:first ?first ; rdf:rest ?rest }}"
    )
}

/// Calculations from the rows of [`calcs_select`] and [`cells_select`],
/// parsed with `prefixes` (none for stored ones: those are canonical).
fn calcs_from(
    rows: &[QuerySolution],
    cells: &[QuerySolution],
    prefixes: &[(String, String)],
) -> Vec<Stored> {
    let mut list: HashMap<(String, String), (String, String)> = HashMap::new();
    for r in cells {
        if let (Some(c), Some(cell), Some(first), Some(rest)) =
            (r.get("c"), r.get("cell"), r.get("first"), r.get("rest"))
        {
            list.insert(
                (term_string(c), term_string(cell)),
                (term_string(first), term_string(rest)),
            );
        }
    }
    let nil = format!("{RDF}nil");
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for r in rows {
        let Some(c) = r.get("c").map(term_string) else {
            continue;
        };
        if !seen.insert(c.clone()) {
            continue;
        }
        let get = |k: &str| r.get(k).map(term_string);
        let mut paths = Vec::new();
        let mut cursor = get("head");
        while let Some(cell) = cursor.filter(|c| *c != nil) {
            let Some((first, rest)) = list.get(&(c.clone(), cell)) else {
                break;
            };
            paths.push(first.clone());
            if paths.len() > MAX_ARGS {
                break;
            }
            cursor = Some(rest.clone());
        }
        let body = CalcBody {
            label: get("label"),
            inferred_property: get("inf").unwrap_or_default(),
            argument_paths: paths,
            expression: get("expr").unwrap_or_default(),
            prefixes: prefixes.iter().cloned().collect(),
            foi_restriction: get("foi"),
            path_restriction: get("pr"),
            graph: get("dg"),
        };
        out.push(Stored {
            calc: build_calc(&c, &body),
            iri: c,
            body,
        });
        if out.len() >= MAX_CALCULATIONS {
            break;
        }
    }
    out.sort_by(|a, b| a.iri.cmp(&b.iri));
    out
}

/// The calculations in the states graph (`only`: just that one).
pub(crate) fn load_calcs(
    state: &AppState,
    states: &str,
    only: Option<&str>,
) -> Result<Vec<Stored>, ApiErr> {
    let timeout = std::time::Duration::from_secs(state.query_timeout_secs.max(1));
    let scope = [states.to_string()];
    let mut bindings: Vec<(&str, Term)> = Vec::new();
    if let Some(c) = only {
        bindings.push((
            "c",
            NamedNode::new(c).map_err(|e| bad(e.to_string()))?.into(),
        ));
    }
    let run = |q: String, cap: usize| -> Result<Vec<QuerySolution>, ApiErr> {
        let parsed = crate::sparql::parser().parse_query(&q).map_err(e500)?;
        state
            .store
            .select_confined(&parsed, &scope, &bindings, cap, timeout)
            .map_err(e500)
    };
    let rows = run(calcs_select(), MAX_CALCULATIONS * 8)?;
    let cells = run(cells_select(), MAX_CALCULATIONS * MAX_ARGS * 4)?;
    Ok(calcs_from(&rows, &cells, &[]))
}

/// The calculations in an import's scratch store, parsed with the
/// document's own prefixes.
pub(crate) fn calcs_in(
    scratch: &oxigraph::store::Store,
    prefixes: &[(String, String)],
) -> Result<Vec<Stored>, String> {
    let run = |q: String| -> Result<Vec<QuerySolution>, String> {
        match oxigraph::sparql::SparqlEvaluator::new()
            .parse_query(&q)
            .map_err(|e| e.to_string())?
            .on_store(scratch)
            .execute()
            .map_err(|e| e.to_string())?
        {
            oxigraph::sparql::QueryResults::Solutions(s) => {
                s.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
            }
            _ => Ok(Vec::new()),
        }
    };
    Ok(calcs_from(
        &run(calcs_select())?,
        &run(cells_select())?,
        prefixes,
    ))
}

fn stored_view(s: &Stored) -> serde_json::Value {
    match &s.calc {
        Ok(c) => c.view(),
        Err(e) => serde_json::json!({
            "calculation": s.iri,
            "id": calc_id(&s.iri),
            "label": s.body.label,
            "inferred_property": s.body.inferred_property,
            "argument_paths": s.body.argument_paths,
            "expression": s.body.expression,
            "foi_restriction": s.body.foi_restriction,
            "path_restriction": s.body.path_restriction,
            "graph": s.body.graph,
            "valid": false,
            "error": e,
        }),
    }
}

fn one_calc(state: &AppState, states: &str, iri: &str) -> Result<Stored, ApiErr> {
    load_calcs(state, states, Some(iri))?
        .into_iter()
        .next()
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("no calculation <{iri}>")))
}

// ─── Running ─────────────────────────────────────────────────────────────────

/// The current state of every (item, kind) for some kinds, and whether
/// each state seen is current.
struct Currents {
    by_item: HashMap<(String, String), StoredState>,
    is_current: HashMap<String, bool>,
}

/// The current state of every (item, kind) for `kinds`: the server's own
/// state where it keeps one, else a canonical one.
fn current_states(
    state: &AppState,
    scope: &ReadScope,
    kinds: &[&NamedNode],
) -> Result<Currents, ApiErr> {
    let mut current: HashMap<(String, String), StoredState> = HashMap::new();
    let mut is_current: HashMap<String, bool> = HashMap::new();
    let mut done = HashSet::new();
    for k in kinds {
        if !done.insert(k.as_str().to_string()) {
            continue;
        }
        let groups = group_states(read_states(state, scope, None, Some(k.as_str()))?);
        let mut by_item: HashMap<(String, String), Vec<&PropertyStates>> = HashMap::new();
        for g in &groups {
            for s in &g.states {
                is_current.insert(s.iri.clone(), s.current);
            }
            by_item
                .entry((g.entity.clone(), g.property.clone()))
                .or_default()
                .push(g);
        }
        for (key, gs) in by_item {
            let managed = gs.iter().find(|g| g.states.iter().any(|s| !s.canonical));
            let chosen = managed.or(gs.first()).and_then(|g| g.current());
            if let Some(s) = chosen {
                current.insert(key, s.clone());
            }
        }
    }
    Ok(Currents {
        by_item: current,
        is_current,
    })
}

/// One feature of interest a run will derive, or why it will not.
struct Derivation {
    foi: NamedNode,
    arguments: Vec<StoredState>,
}

struct Matched {
    ready: Vec<Derivation>,
    skipped: Vec<(String, String)>,
}

/// Match the calculation's paths and resolve each argument to its current
/// state.
fn match_arguments(
    state: &AppState,
    scope: &ReadScope,
    states_g: &str,
    calc: &Calc,
    only: Option<&[NamedNode]>,
    current: &HashMap<(String, String), StoredState>,
) -> Result<Matched, ApiErr> {
    let data_graphs: Vec<String> = scope
        .graphs
        .iter()
        .filter(|g| g.as_str() != states_g)
        .cloned()
        .collect();
    if data_graphs.is_empty() || only.is_some_and(|o| o.is_empty()) {
        return Ok(Matched {
            ready: Vec::new(),
            skipped: Vec::new(),
        });
    }
    let cap = max_rows();
    let rows = state
        .store
        .select_confined(
            &calc.matching_query(only),
            &data_graphs,
            &[],
            cap,
            std::time::Duration::from_secs(state.query_timeout_secs.max(1)),
        )
        .map_err(|e| {
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("the calculation's paths could not be matched: {e}; narrow it with opm:foiRestriction or opm:pathRestriction"),
            )
        })?;
    // Per feature of interest, the distinct argument-state combinations.
    struct FoiMatches {
        foi: NamedNode,
        combos: Vec<Vec<String>>,
        reason: Option<String>,
    }
    let mut per_foi: BTreeMap<String, FoiMatches> = BTreeMap::new();
    for row in &rows {
        let Some(Term::NamedNode(f)) = row.get("foi") else {
            continue;
        };
        let entry = per_foi
            .entry(f.as_str().to_string())
            .or_insert_with(|| FoiMatches {
                foi: f.clone(),
                combos: Vec::new(),
                reason: None,
            });
        let mut combo = Vec::new();
        for (i, a) in calc.args.iter().enumerate() {
            let owner = match &a.owner {
                TermPattern::Variable(v) if *v == foi() => Some(f.as_str().to_string()),
                TermPattern::NamedNode(n) => Some(n.as_str().to_string()),
                _ => row.get(format!("__o{i}").as_str()).map(term_string),
            };
            let Some(owner) = owner else { continue };
            match current.get(&(owner.clone(), a.predicate.as_str().to_string())) {
                Some(s) => combo.push(s.iri.clone()),
                None => {
                    entry.reason.get_or_insert(format!(
                        "argument ?{} (<{owner}> <{}>) has no property state",
                        a.var.as_str(),
                        a.predicate.as_str()
                    ));
                    combo.clear();
                    break;
                }
            }
        }
        if combo.len() == calc.args.len() && !entry.combos.contains(&combo) {
            entry.combos.push(combo);
        }
    }
    let by_iri: HashMap<&str, &StoredState> =
        current.values().map(|s| (s.iri.as_str(), s)).collect();
    let mut ready = Vec::new();
    let mut skipped = Vec::new();
    for (foi_s, m) in per_foi {
        let (foi_n, combos, reason) = (m.foi, m.combos, m.reason);
        let combo = match combos.as_slice() {
            [one] => one,
            [] => {
                skipped.push((foi_s, reason.unwrap_or_else(|| "no arguments".to_string())));
                continue;
            }
            many => {
                skipped.push((
                    foi_s,
                    format!(
                        "the argument paths match {} different argument sets",
                        many.len()
                    ),
                ));
                continue;
            }
        };
        let arguments: Vec<StoredState> = combo
            .iter()
            .filter_map(|iri| by_iri.get(iri.as_str()).map(|s| (*s).clone()))
            .collect();
        let problem = calc.args.iter().zip(&arguments).find_map(|(a, s)| {
            let name = a.var.as_str();
            if s.deleted {
                Some(format!("argument ?{name} is deleted"))
            } else if s.iri.starts_with("_:") {
                Some(format!("argument ?{name} is a blank-node state; import the data to make it addressable"))
            } else if !s.value.as_ref().is_some_and(is_numeric) {
                Some(format!("argument ?{name} is not a numeric XSD value"))
            } else {
                None
            }
        });
        match problem {
            Some(p) => skipped.push((foi_s, p)),
            None => ready.push(Derivation {
                foi: foi_n,
                arguments,
            }),
        }
    }
    Ok(Matched { ready, skipped })
}

/// A derived state of `calc` whose arguments have since been outdated.
struct Outdated {
    foi: String,
    state: String,
    outdated_arguments: Vec<String>,
}

fn find_outdated(
    calc: &Calc,
    current: &HashMap<(String, String), StoredState>,
    is_current: &HashMap<String, bool>,
) -> Vec<Outdated> {
    let mut out: Vec<Outdated> = current
        .iter()
        .filter(|((_, p), s)| {
            p == calc.inferred.as_str()
                && s.calculation.as_deref() == Some(calc.iri.as_str())
                && !s.deleted
        })
        .filter_map(|((foi, _), s)| {
            let stale: Vec<String> = s
                .arguments
                .iter()
                .filter(|m| !is_current.get(m.as_str()).copied().unwrap_or(false))
                .cloned()
                .collect();
            (!stale.is_empty() || s.arguments.len() != calc.args.len()).then(|| Outdated {
                foi: foi.clone(),
                state: s.iri.clone(),
                outdated_arguments: stale,
            })
        })
        .collect();
    out.sort_by(|a, b| a.foi.cmp(&b.foi));
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// OPM POST: every feature of interest that lacks the property.
    Post,
    /// OPM PUT: every derived state whose arguments were outdated.
    Put,
}

/// Run `calc` (POST or PUT) and write the derived states as one commit.
async fn run(
    state: &AppState,
    user: &AuthenticatedUser,
    ds: &Dataset,
    calc: &Calc,
    mode: Mode,
) -> Result<serde_json::Value, ApiErr> {
    let scope = read_scope(state, Some(&user.user_id), ds)?;
    let states_g = ensure_states_graph(state, &ds.id)?;
    let mut kinds: Vec<&NamedNode> = calc.args.iter().map(|a| &a.predicate).collect();
    kinds.push(&calc.inferred);
    let Currents {
        by_item: current,
        is_current,
    } = current_states(state, &scope, &kinds)?;
    let outdated = find_outdated(calc, &current, &is_current);
    let only: Option<Vec<NamedNode>> = match mode {
        Mode::Post => None,
        Mode::Put => Some(
            outdated
                .iter()
                .filter_map(|o| NamedNode::new(&o.foi).ok())
                .collect(),
        ),
    };
    let matched = match_arguments(state, &scope, &states_g, calc, only.as_deref(), &current)?;
    let mut skipped = matched.skipped;
    let mut todo = Vec::new();
    for d in matched.ready {
        let existing = current.get(&(
            d.foi.as_str().to_string(),
            calc.inferred.as_str().to_string(),
        ));
        if mode == Mode::Post {
            if existing.is_some() {
                skipped.push((
                    d.foi.as_str().to_string(),
                    "already has the property (PUT recomputes derived values)".to_string(),
                ));
                continue;
            }
            let has_plain = scope.graphs.iter().any(|g| {
                NamedNode::new(g).is_ok_and(|g| {
                    state
                        .store
                        .store()
                        .quads_for_pattern(
                            Some(d.foi.as_ref().into()),
                            Some(calc.inferred.as_ref()),
                            None,
                            Some(g.as_ref().into()),
                        )
                        .next()
                        .is_some()
                })
            });
            if has_plain {
                skipped.push((
                    d.foi.as_str().to_string(),
                    "already has the property as a plain value".to_string(),
                ));
                continue;
            }
        }
        todo.push((d, existing.cloned()));
    }
    let timeout = std::time::Duration::from_secs(state.query_timeout_secs.max(1));
    let values: Vec<Vec<Term>> = todo
        .iter()
        .map(|(d, _)| d.arguments.iter().filter_map(|s| s.value.clone()).collect())
        .collect();
    let results = evaluate(calc, &values, timeout).map_err(|e| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("the expression could not be evaluated: {e}"),
        )
    })?;
    let now = chrono::Utc::now().to_rfc3339();
    let agent = agent_iri(state, &user.user_id);
    let mut statements = Vec::new();
    let mut graphs = Vec::new();
    let mut derived = Vec::new();
    for ((d, existing), result) in todo.into_iter().zip(results) {
        let Some(Term::Literal(result)) = result else {
            skipped.push((
                d.foi.as_str().to_string(),
                "the expression has no value for these arguments".to_string(),
            ));
            continue;
        };
        let data_graph = match calc.data_graph.as_deref() {
            Some(g) => registered_data_graph(state, &ds.id, g)?,
            None => match existing.as_ref().and_then(|e| e.data_graph.clone()) {
                Some(g) if registered_data_graph(state, &ds.id, &g).is_ok() => g,
                _ => default_data_graph(state, &ds.id).ok_or_else(|| {
                    bad("the dataset has no graph to hold derived values; register one or set `graph`")
                })?,
            },
        };
        let st = format!("urn:ots:property-state:{}", uuid::Uuid::new_v4());
        let seq = format!("urn:ots:derivation:{}", uuid::Uuid::new_v4());
        let st_e = escape_sparql_iri(&st);
        let seq_e = escape_sparql_iri(&seq);
        let mut extra = vec![
            format!(
                "<{st_e}> opm:expression \"{}\"",
                escape_sparql_literal(&calc.expression_text)
            ),
            format!("<{st_e}> prov:wasDerivedFrom <{seq_e}>"),
            format!(
                "<{st_e}> ots:calculation <{}>",
                escape_sparql_iri(&calc.iri)
            ),
            format!("<{seq_e}> a rdf:Seq"),
        ];
        for (i, a) in d.arguments.iter().enumerate() {
            extra.push(format!(
                "<{seq_e}> rdf:_{} <{}>",
                i + 1,
                escape_sparql_iri(&a.iri)
            ));
        }
        let prop = managed_property_node(state, &states_g, d.foi.as_str(), calc.inferred.as_str())?;
        let write = StateWrite {
            entity: d.foi.as_str().to_string(),
            property: calc.inferred.as_str().to_string(),
            value: Some(Term::Literal(result.clone()).to_string()),
            valid_from: now.clone(),
            recorded_at: now.clone(),
            agent: agent.clone(),
            reliability: Some(Reliability::Derived),
            note: None,
            documentation: Vec::new(),
            data_graph: data_graph.clone(),
            revision_of: None,
            extra,
        };
        statements.push(state_update(&states_g, &prop, &st, &write));
        graphs.push(data_graph);
        derived.push(serde_json::json!({
            "foi": d.foi.as_str(),
            "state": st,
            "value": result.value(),
            "datatype": result.datatype().as_str(),
            "derived_from": d.arguments.iter().map(|a| a.iri.clone()).collect::<Vec<_>>(),
        }));
    }
    graphs.sort();
    graphs.dedup();
    let n = derived.len();
    apply_writes(
        state,
        &ds.id,
        &user.user_id,
        statements,
        graphs,
        format!(
            "Calculation <{}> ({}): {n} derived states",
            calc.iri,
            if mode == Mode::Post { "POST" } else { "PUT" }
        ),
        n,
    )
    .await?;
    let skipped_count = skipped.len();
    skipped.truncate(100);
    Ok(serde_json::json!({
        "calculation": calc.iri,
        "mode": if mode == Mode::Post { "post" } else { "put" },
        "derived_count": n,
        "derived": derived,
        "skipped_count": skipped_count,
        "skipped": skipped.into_iter().map(|(foi, reason)| serde_json::json!({"foi": foi, "reason": reason})).collect::<Vec<_>>(),
    }))
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// POST /api/datasets/:id/properties/calculations — define a calculation.
pub async fn create_calculation(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(dataset_id): Path<String>,
    Json(body): Json<CalcBody>,
) -> Result<impl IntoResponse, ApiErr> {
    writable_dataset(&state, &user, &dataset_id)?;
    if let Some(g) = &body.graph {
        registered_data_graph(&state, &dataset_id, g)?;
    }
    let iri = format!("{CALC_PREFIX}{}", uuid::Uuid::new_v4());
    let calc = build_calc(&iri, &body).map_err(bad)?;
    let states = ensure_states_graph(&state, &dataset_id)?;
    let now = chrono::Utc::now().to_rfc3339();
    apply_writes(
        &state,
        &dataset_id,
        &user.user_id,
        vec![calc_insert(
            &states,
            &calc,
            &agent_iri(&state, &user.user_id),
            &now,
        )],
        Vec::new(),
        format!(
            "Calculation defined: <{iri}> infers <{}>",
            calc.inferred.as_str()
        ),
        1,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(calc.view())))
}

/// GET /api/datasets/:id/properties/calculations
pub async fn list_calculations(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(dataset_id): Path<String>,
) -> Result<impl IntoResponse, ApiErr> {
    let uid = user.as_ref().map(|Extension(u)| u.user_id.as_str());
    let ds = visible_dataset(&state, uid, &dataset_id)?;
    let states = states_graph(&dataset_id);
    let scope = read_scope(&state, uid, &ds)?;
    let calcs = if scope.graphs.contains(&states) {
        load_calcs(&state, &states, None)?
    } else {
        Vec::new()
    };
    Ok(Json(serde_json::json!({
        "dataset_id": dataset_id,
        "calculations": calcs.iter().map(stored_view).collect::<Vec<_>>(),
    })))
}

/// GET /api/datasets/:id/properties/calculations/:calc
pub async fn get_calculation(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path((dataset_id, id)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiErr> {
    let uid = user.as_ref().map(|Extension(u)| u.user_id.as_str());
    let ds = visible_dataset(&state, uid, &dataset_id)?;
    let states = states_graph(&dataset_id);
    let scope = read_scope(&state, uid, &ds)?;
    if !scope.graphs.contains(&states) {
        return Err((StatusCode::NOT_FOUND, "no such calculation".to_string()));
    }
    Ok(Json(stored_view(&one_calc(
        &state,
        &states,
        &calc_iri(&id)?,
    )?)))
}

/// DELETE /api/datasets/:id/properties/calculations/:calc — remove the
/// definition; the states it derived stay.
pub async fn delete_calculation(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((dataset_id, id)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiErr> {
    writable_dataset(&state, &user, &dataset_id)?;
    let states = states_graph(&dataset_id);
    let iri = calc_iri(&id)?;
    one_calc(&state, &states, &iri)?;
    apply_writes(
        &state,
        &dataset_id,
        &user.user_id,
        vec![calc_delete(&states, &iri)],
        Vec::new(),
        format!("Calculation removed: <{iri}>"),
        0,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn run_handler(
    state: AppState,
    user: AuthenticatedUser,
    dataset_id: String,
    id: String,
    mode: Mode,
) -> Result<Json<serde_json::Value>, ApiErr> {
    let ds = writable_dataset(&state, &user, &dataset_id)?;
    let states = states_graph(&dataset_id);
    let stored = one_calc(&state, &states, &calc_iri(&id)?)?;
    let calc = stored.calc.map_err(|e| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("the stored calculation is not valid: {e}"),
        )
    })?;
    Ok(Json(run(&state, &user, &ds, &calc, mode).await?))
}

/// POST /api/datasets/:id/properties/calculations/:calc — OPM's POST: derive
/// the property for every feature of interest that has the arguments and
/// lacks the property.
pub async fn post_calculation(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((dataset_id, id)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiErr> {
    run_handler(state, user, dataset_id, id, Mode::Post).await
}

/// PUT /api/datasets/:id/properties/calculations/:calc — OPM's PUT:
/// recompute every derived state one of whose arguments was outdated.
pub async fn put_calculation(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((dataset_id, id)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiErr> {
    run_handler(state, user, dataset_id, id, Mode::Put).await
}

/// GET /api/datasets/:id/properties/calculations/:calc/outdated
pub async fn outdated_calculation(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path((dataset_id, id)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiErr> {
    let uid = user.as_ref().map(|Extension(u)| u.user_id.as_str());
    let ds = visible_dataset(&state, uid, &dataset_id)?;
    let states = states_graph(&dataset_id);
    let scope = read_scope(&state, uid, &ds)?;
    if !scope.graphs.contains(&states) {
        return Err((StatusCode::NOT_FOUND, "no such calculation".to_string()));
    }
    let stored = one_calc(&state, &states, &calc_iri(&id)?)?;
    let calc = stored.calc.map_err(|e| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("the stored calculation is not valid: {e}"),
        )
    })?;
    let mut kinds: Vec<&NamedNode> = calc.args.iter().map(|a| &a.predicate).collect();
    kinds.push(&calc.inferred);
    let Currents {
        by_item: current,
        is_current,
    } = current_states(&state, &scope, &kinds)?;
    let outdated = find_outdated(&calc, &current, &is_current);
    Ok(Json(serde_json::json!({
        "calculation": calc.iri,
        "outdated_count": outdated.len(),
        "outdated": outdated.iter().map(|o| serde_json::json!({
            "foi": o.foi,
            "state": o.state,
            "outdated_arguments": o.outdated_arguments,
        })).collect::<Vec<_>>(),
    })))
}
