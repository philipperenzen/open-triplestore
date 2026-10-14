//! What a repair rule is, how its text becomes something the chase can run,
//! and what it reads and writes (`docs/notes/repair-layer-design.md` §3.1).
//!
//! A rule is a SPARQL `CONSTRUCT { head } WHERE { body }` plus the `ots:`
//! terms around it: labelled nulls (an existential TGD), an `ots:equate` pair
//! (an EGD, with an empty head), an `ots:retract` template (a replacement
//! rule), graph scope, placement, priority, policy and confidence. The body
//! is parsed with spargebra and checked by a walk over its algebra: basic
//! graph patterns, `FILTER`, `BIND`, `VALUES`, `FILTER [NOT] EXISTS`, `MINUS`
//! and `GRAPH`; no `OPTIONAL`, no top-level `UNION`, no `SERVICE`, no
//! aggregates, no property paths outside a negation. Every top-level atom is
//! rewritten into `GRAPH ?__gN { atom }` so a solution says which graph each
//! premise came from (§4.6), and the evaluation query is a `SELECT` of every
//! in-scope variable with the rule's satisfaction guard appended as
//! `FILTER NOT EXISTS` (§4.2).

use std::collections::{BTreeSet, HashMap};

use serde::Serialize;
use spargebra::algebra::{Expression, GraphPattern, PropertyPathExpression};
use spargebra::term::{GroundTerm, NamedNodePattern, TermPattern, TriplePattern, Variable};
use spargebra::Query;

use crate::shacl::shapes::Shape;

pub const OTS: &str = "https://opentriplestore.org/ns#";
pub const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
pub const OWL_SAME_AS: &str = "http://www.w3.org/2002/07/owl#sameAs";

/// `ots:graphScope`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GraphScope {
    /// The body's top-level atoms outside an explicit `GRAPH` all match in
    /// one graph.
    PerGraph,
    /// Each atom matches in any premise graph.
    Union,
}

/// `ots:mergeMode`: how an EGD merges two live IRIs (§4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MergeMode {
    /// One `loser owl:sameAs winner` line.
    SameAsOnly,
    /// Every triple of the loser deleted and re-added for the winner.
    Rewrite,
}

/// `ots:policy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Policy {
    Repair,
    /// Fires and is counted, emits no line.
    Report,
}

/// `ots:confidence`, copied into every action of the rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Confidence {
    /// Forced by the semantics of the constraint or axiom.
    Certain,
    /// A declared choice (an opt-in policy).
    Policy,
    /// Proposed by a language model; reserved for that channel.
    Heuristic,
}

impl Confidence {
    pub fn parse(s: &str) -> Option<Self> {
        match s
            .trim()
            .trim_start_matches(OTS)
            .to_ascii_lowercase()
            .as_str()
        {
            "certain" => Some(Self::Certain),
            "policy" => Some(Self::Policy),
            "heuristic" => Some(Self::Heuristic),
            _ => None,
        }
    }
}

/// Where a rule came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Origin {
    /// An `ots:Rule` in a rules or shapes graph.
    Authored,
    /// Compiled from a SHACL Core shape or an OWL axiom.
    Compiled,
    /// Imported from a SHACL-AF `sh:rule`.
    ShaclAf,
    /// Proposed by the assistant; held to the heuristic guard (§9).
    Heuristic,
}

/// A rule whose head the chase computes in Rust because SPARQL cannot state
/// it exactly (both are compiled from opt-in policies, §3.6).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Native {
    /// `datatype-relabel`: a literal `"lex"^^other` on `predicate` becomes
    /// `"lex"^^datatype` when `lex` is in that datatype's lexical space (the
    /// validator's own check, `xsd_lexical_valid`). The body binds `?this`
    /// and `?v`.
    DatatypeRelabel { datatype: String, predicate: String },
    /// `maxCount-keep-lexmin`: of the focus node's values on `predicate`
    /// (`inverse`: as object), keep the `max` smallest in N-Triples order and
    /// delete the rest. The body binds `?this`.
    MaxCountKeepLexmin {
        max: usize,
        predicate: String,
        inverse: bool,
    },
}

/// The validation result a compiled rule answers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ViolationRef {
    pub source_shape: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub source_constraint: String,
    pub source_constraint_component: String,
}

/// A rule as written — in a rules graph, by the compiler, or imported from a
/// SHACL-AF `sh:rule` — before it is parsed.
#[derive(Debug, Clone, Serialize)]
pub struct RuleSpec {
    pub iri: String,
    /// `CONSTRUCT { head } WHERE { body }` (the `INSERT` keyword is accepted
    /// for `CONSTRUCT`, as for `sh:construct`).
    pub construct: String,
    /// Head variables the body does not bind: labelled nulls.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub nulls: Vec<String>,
    /// What nulls are minted from instead of the rule's IRI: rules compiled
    /// from one constraint for several targets share it, so the same focus
    /// gets the same witness whichever target found it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub null_key: Option<String>,
    /// Two body variables an EGD equates; the head must then be empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equate: Option<(String, String)>,
    /// Triples deleted when the rule fires (a construct template).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retract: Option<String>,
    pub merge_mode: MergeMode,
    pub graph_scope: GraphScope,
    /// `None` is `ots:FocusGraph`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_graph: Option<String>,
    pub priority: f64,
    pub policy: Policy,
    pub confidence: Confidence,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub derived_from: Option<String>,
    pub destructive: bool,
    /// Explanation template: `{?var}` is replaced by the binding.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub origin: Origin,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_graph: Option<String>,
    /// The satisfaction check, a group graph pattern, when the compiler
    /// states the constraint's own check instead of "the head exists".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guard: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native: Option<Native>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub violation: Option<ViolationRef>,
    /// A policy deletion (`closed-delete`, `maxCount-keep-lexmin`, a
    /// `Rewrite` merge): confined to the last stratum (§6).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub policy_deletion: bool,
    /// `sh:condition` shapes of an imported SHACL-AF rule.
    #[serde(skip)]
    pub conditions: Vec<Shape>,
}

impl RuleSpec {
    pub fn new(iri: impl Into<String>, construct: impl Into<String>, origin: Origin) -> Self {
        Self {
            iri: iri.into(),
            construct: construct.into(),
            nulls: Vec::new(),
            null_key: None,
            equate: None,
            retract: None,
            merge_mode: MergeMode::SameAsOnly,
            graph_scope: match origin {
                Origin::Compiled => GraphScope::PerGraph,
                _ => GraphScope::Union,
            },
            target_graph: None,
            priority: 0.0,
            policy: Policy::Repair,
            confidence: match origin {
                Origin::Heuristic => Confidence::Heuristic,
                _ => Confidence::Certain,
            },
            derived_from: None,
            destructive: false,
            message: None,
            origin,
            source_graph: None,
            guard: None,
            native: None,
            violation: None,
            policy_deletion: false,
            conditions: Vec::new(),
        }
    }
}

/// A set of predicates a rule reads or writes; `all` when a variable stands
/// in predicate position, which mentions every predicate (§3.4).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PredSet {
    pub all: bool,
    pub iris: BTreeSet<String>,
}

impl PredSet {
    pub fn add(&mut self, p: &NamedNodePattern) {
        match p {
            NamedNodePattern::NamedNode(n) => {
                self.iris.insert(n.as_str().to_string());
            }
            NamedNodePattern::Variable(_) => self.all = true,
        }
    }

    pub fn add_iri(&mut self, iri: &str) {
        self.iris.insert(iri.to_string());
    }

    pub fn extend(&mut self, other: &PredSet) {
        self.all |= other.all;
        self.iris.extend(other.iris.iter().cloned());
    }

    pub fn is_empty(&self) -> bool {
        !self.all && self.iris.is_empty()
    }

    pub fn intersects(&self, other: &PredSet) -> bool {
        if self.is_empty() || other.is_empty() {
            return false;
        }
        self.all || other.all || self.iris.iter().any(|p| other.iris.contains(p))
    }
}

/// What a rule reads and writes, for stratification (§3.4).
#[derive(Debug, Clone, Default)]
pub struct Deps {
    /// Predicates its body reads positively (atoms, positive `EXISTS`).
    pub reads: PredSet,
    /// Predicates inside its own `NOT EXISTS` / `MINUS` (not its guard).
    pub negated: PredSet,
    pub produces: PredSet,
    pub retracts: PredSet,
}

/// The rule's kind.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Tgd,
    Egd { a: Variable, b: Variable },
}

/// A top-level body atom and the graph it matched in.
#[derive(Debug, Clone)]
pub struct Atom {
    pub pattern: TriplePattern,
    pub graph: NamedNodePattern,
}

/// Where a head (or retract) triple lands (§4.6).
#[derive(Debug, Clone, PartialEq)]
pub enum Placement {
    /// `ots:targetGraph`.
    Target(String),
    /// The graph body atom `k` matched in.
    Atom(usize),
    /// The graph head triple `j` landed in (a witness's own triples go
    /// where the triple introducing it goes).
    Head(usize),
    /// Undetermined: the dataset's only writable graph, else unplaceable.
    Unknown,
}

/// A rule ready to run.
#[derive(Debug, Clone)]
pub struct PreparedRule {
    pub spec: RuleSpec,
    pub kind: Kind,
    pub head: Vec<TriplePattern>,
    pub retract: Vec<TriplePattern>,
    pub nulls: Vec<Variable>,
    /// Head variables the body binds, by name: what a null depends on.
    pub frontier: Vec<Variable>,
    pub atoms: Vec<Atom>,
    /// The rewritten body, without the guard.
    pub body: GraphPattern,
    /// The pattern of the satisfaction guard (`FILTER NOT EXISTS { … }`).
    pub guard: Option<GraphPattern>,
    pub deps: Deps,
    pub head_placement: Vec<Placement>,
    pub retract_placement: Vec<Placement>,
    /// For an EGD: where `a owl:sameAs b` lands when `a` (resp. `b`) loses.
    pub egd_placement: Option<(Placement, Placement)>,
    /// Whether the body binds `?this` (scope.focus restricts it).
    pub binds_this: bool,
}

/// A rule set that could not be loaded: the 400 the endpoint answers.
#[derive(Debug, Clone)]
pub struct RuleError {
    pub rule: String,
    pub message: String,
}

impl std::fmt::Display for RuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "rule <{}>: {}", self.rule, self.message)
    }
}

fn var(name: &str) -> Result<Variable, String> {
    let name = name.trim().trim_start_matches(['?', '$']);
    Variable::new(name).map_err(|e| format!("`{name}` is not a variable name: {e}"))
}

/// Parse a group graph pattern `{ … }` (the inside, or with its braces),
/// with `prologue` prepended so prefixed names resolve.
fn parse_group(prologue: &str, text: &str) -> Result<GraphPattern, String> {
    let t = text.trim();
    let inner = t
        .strip_prefix('{')
        .and_then(|r| r.strip_suffix('}'))
        .unwrap_or(t);
    let q = spargebra::SparqlParser::new()
        .parse_query(&format!("{prologue}ASK WHERE {{ {inner} }}"))
        .map_err(|e| e.to_string())?;
    match q {
        Query::Ask { pattern, .. } => Ok(pattern),
        _ => Err("not a group graph pattern".into()),
    }
}

/// Parse a construct template `{ … }` (or its inside).
fn parse_template(prologue: &str, text: &str) -> Result<Vec<TriplePattern>, String> {
    let t = text.trim();
    let inner = t
        .strip_prefix('{')
        .and_then(|r| r.strip_suffix('}'))
        .unwrap_or(t);
    let q = spargebra::SparqlParser::new()
        .parse_query(&format!("{prologue}CONSTRUCT {{ {inner} }} WHERE {{}}"))
        .map_err(|e| e.to_string())?;
    match q {
        Query::Construct { template, .. } => Ok(template),
        _ => Err("not a construct template".into()),
    }
}

/// A head or retract term may not be a blank node (a null must be named, so
/// a `D` line or a later EGD can refer to it) nor a triple term (no patch
/// line can carry one).
fn check_template(tps: &[TriplePattern], what: &str) -> Result<(), String> {
    for tp in tps {
        for t in [&tp.subject, &tp.object] {
            match t {
                TermPattern::BlankNode(b) => {
                    return Err(format!(
                        "the {what} holds the blank node _:{} — declare it in ots:nulls as a variable instead",
                        b.as_str()
                    ))
                }
                TermPattern::Variable(_) | TermPattern::NamedNode(_) | TermPattern::Literal(_) => {}
                #[allow(unreachable_patterns)]
                _ => {
                    return Err(format!(
                        "the {what} holds a triple term, which no RDF Patch line can carry"
                    ))
                }
            }
        }
    }
    Ok(())
}

fn template_vars(tps: &[TriplePattern]) -> BTreeSet<Variable> {
    let mut out = BTreeSet::new();
    for tp in tps {
        for t in [&tp.subject, &tp.object] {
            if let TermPattern::Variable(v) = t {
                out.insert(v.clone());
            }
        }
        if let NamedNodePattern::Variable(v) = &tp.predicate {
            out.insert(v.clone());
        }
    }
    out
}

/// Predicates mentioned anywhere in a pattern (for the negated or
/// positively-`EXISTS`ed patterns, which are not rewritten).
fn pattern_predicates(p: &GraphPattern, out: &mut PredSet) -> Result<(), String> {
    match p {
        GraphPattern::Bgp { patterns } => {
            for tp in patterns {
                out.add(&tp.predicate);
            }
        }
        GraphPattern::Path { path, .. } => path_predicates(path, out),
        GraphPattern::Join { left, right }
        | GraphPattern::Union { left, right }
        | GraphPattern::Minus { left, right } => {
            pattern_predicates(left, out)?;
            pattern_predicates(right, out)?;
        }
        GraphPattern::LeftJoin {
            left,
            right,
            expression,
        } => {
            pattern_predicates(left, out)?;
            pattern_predicates(right, out)?;
            if let Some(e) = expression {
                expression_predicates(e, out)?;
            }
        }
        GraphPattern::Filter { expr, inner } => {
            expression_predicates(expr, out)?;
            pattern_predicates(inner, out)?;
        }
        GraphPattern::Graph { inner, .. }
        | GraphPattern::OrderBy { inner, .. }
        | GraphPattern::Project { inner, .. }
        | GraphPattern::Distinct { inner }
        | GraphPattern::Reduced { inner }
        | GraphPattern::Slice { inner, .. }
        | GraphPattern::Group { inner, .. } => pattern_predicates(inner, out)?,
        GraphPattern::Extend {
            inner, expression, ..
        } => {
            expression_predicates(expression, out)?;
            pattern_predicates(inner, out)?;
        }
        GraphPattern::Values { .. } => {}
        GraphPattern::Service { .. } => {
            return Err("SERVICE is not allowed in a rule".to_string());
        }
        #[allow(unreachable_patterns)]
        _ => return Err("this graph pattern form is not allowed in a rule".to_string()),
    }
    Ok(())
}

fn path_predicates(path: &PropertyPathExpression, out: &mut PredSet) {
    match path {
        PropertyPathExpression::NamedNode(n) => out.add_iri(n.as_str()),
        PropertyPathExpression::Reverse(p)
        | PropertyPathExpression::ZeroOrMore(p)
        | PropertyPathExpression::OneOrMore(p)
        | PropertyPathExpression::ZeroOrOne(p) => path_predicates(p, out),
        PropertyPathExpression::Sequence(a, b) | PropertyPathExpression::Alternative(a, b) => {
            path_predicates(a, out);
            path_predicates(b, out);
        }
        // A negated property set matches every predicate but a few.
        PropertyPathExpression::NegatedPropertySet(_) => out.all = true,
    }
}

fn expression_predicates(e: &Expression, out: &mut PredSet) -> Result<(), String> {
    let mut err = None;
    walk_expression(e, true, &mut |p, _| {
        if let Err(e) = pattern_predicates(p, out) {
            err = Some(e);
        }
    });
    err.map_or(Ok(()), Err)
}

/// Call `f(pattern, positive)` for every `EXISTS` pattern in `e`, with its
/// polarity: under an odd number of `!` it is negative. Inside any other
/// operator than `!`, `&&`, `||` the polarity is undetermined and the
/// pattern counts as negative (the conservative reading for stratification).
fn walk_expression(e: &Expression, positive: bool, f: &mut impl FnMut(&GraphPattern, bool)) {
    match e {
        Expression::Exists(p) => f(p, positive),
        Expression::Not(x) => walk_expression(x, !positive, f),
        Expression::And(a, b) | Expression::Or(a, b) => {
            walk_expression(a, positive, f);
            walk_expression(b, positive, f);
        }
        Expression::NamedNode(_)
        | Expression::Literal(_)
        | Expression::Variable(_)
        | Expression::Bound(_) => {}
        Expression::Equal(a, b)
        | Expression::SameTerm(a, b)
        | Expression::Greater(a, b)
        | Expression::GreaterOrEqual(a, b)
        | Expression::Less(a, b)
        | Expression::LessOrEqual(a, b)
        | Expression::Add(a, b)
        | Expression::Subtract(a, b)
        | Expression::Multiply(a, b)
        | Expression::Divide(a, b) => {
            walk_expression(a, false, f);
            walk_expression(b, false, f);
        }
        Expression::UnaryPlus(a) | Expression::UnaryMinus(a) => walk_expression(a, false, f),
        Expression::In(a, list) => {
            walk_expression(a, false, f);
            for x in list {
                walk_expression(x, false, f);
            }
        }
        Expression::If(a, b, c) => {
            walk_expression(a, false, f);
            walk_expression(b, false, f);
            walk_expression(c, false, f);
        }
        Expression::Coalesce(list) | Expression::FunctionCall(_, list) => {
            for x in list {
                walk_expression(x, false, f);
            }
        }
    }
}

/// Rewrites a body: top-level atoms into `GRAPH ?__gN { atom }`, blank nodes
/// into variables, and records atoms and dependencies on the way.
struct Rewriter {
    scope: GraphScope,
    next_graph: usize,
    bnodes: HashMap<String, Variable>,
    atoms: Vec<Atom>,
    reads: PredSet,
    negated: PredSet,
}

/// The graph variable every top-level atom of a per-graph rule shares.
pub const SHARED_GRAPH_VAR: &str = "__g";

impl Rewriter {
    fn debnode(&mut self, t: &TermPattern) -> TermPattern {
        match t {
            TermPattern::BlankNode(b) => {
                let n = self.bnodes.len();
                TermPattern::Variable(
                    self.bnodes
                        .entry(b.as_str().to_string())
                        .or_insert_with(|| Variable::new_unchecked(format!("__b{n}")))
                        .clone(),
                )
            }
            other => other.clone(),
        }
    }

    fn graph_var(&mut self) -> Variable {
        match self.scope {
            GraphScope::PerGraph => Variable::new_unchecked(SHARED_GRAPH_VAR),
            GraphScope::Union => {
                let v = Variable::new_unchecked(format!("__g{}", self.next_graph));
                self.next_graph += 1;
                v
            }
        }
    }

    fn expression(&mut self, e: &Expression) -> Result<(), String> {
        let mut err = None;
        let (mut reads, mut negated) = (PredSet::default(), PredSet::default());
        walk_expression(e, true, &mut |p, positive| {
            let target = if positive { &mut reads } else { &mut negated };
            if let Err(e) = pattern_predicates(p, target) {
                err = Some(e);
            }
        });
        if let Some(e) = err {
            return Err(e);
        }
        self.reads.extend(&reads);
        self.negated.extend(&negated);
        Ok(())
    }

    fn rewrite(
        &mut self,
        p: &GraphPattern,
        graph: Option<&NamedNodePattern>,
    ) -> Result<GraphPattern, String> {
        Ok(match p {
            GraphPattern::Bgp { patterns } => {
                let tps: Vec<TriplePattern> = patterns
                    .iter()
                    .map(|tp| TriplePattern {
                        subject: self.debnode(&tp.subject),
                        predicate: tp.predicate.clone(),
                        object: self.debnode(&tp.object),
                    })
                    .collect();
                for tp in &tps {
                    self.reads.add(&tp.predicate);
                }
                match graph {
                    Some(g) => {
                        for tp in &tps {
                            self.atoms.push(Atom {
                                pattern: tp.clone(),
                                graph: g.clone(),
                            });
                        }
                        GraphPattern::Bgp { patterns: tps }
                    }
                    None => {
                        let mut out: Option<GraphPattern> = None;
                        for tp in tps {
                            let gv = self.graph_var();
                            self.atoms.push(Atom {
                                pattern: tp.clone(),
                                graph: NamedNodePattern::Variable(gv.clone()),
                            });
                            let atom = GraphPattern::Graph {
                                name: NamedNodePattern::Variable(gv),
                                inner: Box::new(GraphPattern::Bgp { patterns: vec![tp] }),
                            };
                            out = Some(match out {
                                None => atom,
                                Some(left) => GraphPattern::Join {
                                    left: Box::new(left),
                                    right: Box::new(atom),
                                },
                            });
                        }
                        out.unwrap_or(GraphPattern::Bgp {
                            patterns: Vec::new(),
                        })
                    }
                }
            }
            GraphPattern::Join { left, right } => GraphPattern::Join {
                left: Box::new(self.rewrite(left, graph)?),
                right: Box::new(self.rewrite(right, graph)?),
            },
            GraphPattern::Filter { expr, inner } => {
                self.expression(expr)?;
                GraphPattern::Filter {
                    expr: expr.clone(),
                    inner: Box::new(self.rewrite(inner, graph)?),
                }
            }
            GraphPattern::Graph { name, inner } => GraphPattern::Graph {
                name: name.clone(),
                inner: Box::new(self.rewrite(inner, Some(name))?),
            },
            GraphPattern::Extend {
                inner,
                variable,
                expression,
            } => {
                self.expression(expression)?;
                GraphPattern::Extend {
                    inner: Box::new(self.rewrite(inner, graph)?),
                    variable: variable.clone(),
                    expression: expression.clone(),
                }
            }
            GraphPattern::Minus { left, right } => {
                let mut negated = PredSet::default();
                pattern_predicates(right, &mut negated)?;
                self.negated.extend(&negated);
                GraphPattern::Minus {
                    left: Box::new(self.rewrite(left, graph)?),
                    right: right.clone(),
                }
            }
            GraphPattern::Values { .. } => p.clone(),
            GraphPattern::Path { .. } => {
                return Err(
                    "a property path in a rule body is not allowed: every premise must be one \
                     quad (a path may be used inside FILTER NOT EXISTS)"
                        .into(),
                )
            }
            GraphPattern::LeftJoin { .. } => {
                return Err("OPTIONAL is not allowed in a rule body".into())
            }
            GraphPattern::Union { .. } => {
                return Err("UNION is not allowed in a rule body (write two rules)".into())
            }
            GraphPattern::Service { .. } => {
                return Err("SERVICE is not allowed in a rule body".into())
            }
            GraphPattern::Group { .. } => {
                return Err("aggregates are not allowed in a rule body".into())
            }
            _ => {
                return Err(
                    "sub-queries and solution modifiers (ORDER BY, LIMIT, DISTINCT) are not allowed in a rule body"
                        .into(),
                )
            }
        })
    }
}

/// The WHERE clause of a parsed CONSTRUCT: spargebra wraps it in the
/// projection of `SELECT *`.
pub fn unwrap_star(p: GraphPattern) -> GraphPattern {
    match p {
        GraphPattern::Project { inner, .. } => *inner,
        other => other,
    }
}

/// The variables in scope of `p`, by name.
pub fn in_scope(p: &GraphPattern) -> Vec<Variable> {
    let mut vars: BTreeSet<Variable> = BTreeSet::new();
    p.on_in_scope_variable(|v| {
        vars.insert(v.clone());
    });
    vars.into_iter().collect()
}

fn is_rdf_type(p: &NamedNodePattern) -> bool {
    matches!(p, NamedNodePattern::NamedNode(n) if n.as_str() == RDF_TYPE)
}

/// Where a triple whose subject (else object) is `t` lands: the typing atom
/// of a body-bound variable, else the first atom binding it as subject, else
/// any atom mentioning it.
fn atom_for(t: &TermPattern, atoms: &[Atom]) -> Option<usize> {
    let same = |x: &TermPattern| x == t;
    if matches!(t, TermPattern::Literal(_)) {
        return None;
    }
    atoms
        .iter()
        .position(|a| is_rdf_type(&a.pattern.predicate) && same(&a.pattern.subject))
        .or_else(|| atoms.iter().position(|a| same(&a.pattern.subject)))
        .or_else(|| atoms.iter().position(|a| same(&a.pattern.object)))
}

fn place_template(
    tps: &[TriplePattern],
    atoms: &[Atom],
    nulls: &[Variable],
    target: Option<&str>,
    match_atoms_exactly: bool,
) -> Vec<Placement> {
    let is_null = |t: &TermPattern| matches!(t, TermPattern::Variable(v) if nulls.contains(v));
    tps.iter()
        .enumerate()
        .map(|(j, tp)| {
            if let Some(g) = target {
                return Placement::Target(g.to_string());
            }
            if match_atoms_exactly {
                if let Some(k) = atoms.iter().position(|a| &a.pattern == tp) {
                    return Placement::Atom(k);
                }
            }
            if is_null(&tp.subject) {
                // A witness's own triples go where the triple that introduces
                // it goes.
                if let Some(parent) = tps.iter().enumerate().position(|(i, other)| {
                    i != j && other.object == tp.subject && !is_null(&other.subject)
                }) {
                    return Placement::Head(parent);
                }
            } else if let Some(k) = atom_for(&tp.subject, atoms) {
                return Placement::Atom(k);
            }
            if !is_null(&tp.object) {
                if let Some(k) = atom_for(&tp.object, atoms) {
                    return Placement::Atom(k);
                }
            }
            Placement::Unknown
        })
        .collect()
}

/// The graph term a placement resolves to statically (for the guard).
fn placement_graph(
    p: &Placement,
    placements: &[Placement],
    atoms: &[Atom],
    depth: usize,
) -> Option<NamedNodePattern> {
    match p {
        Placement::Target(g) => oxigraph::model::NamedNode::new(g)
            .ok()
            .map(NamedNodePattern::NamedNode),
        Placement::Atom(k) => Some(atoms[*k].graph.clone()),
        Placement::Head(j) if depth < placements.len() => {
            placement_graph(&placements[*j], placements, atoms, depth + 1)
        }
        _ => None,
    }
}

/// The head guard (§4.2): the complete head with every null replaced by a
/// fresh variable. Per-graph rules check value atoms in the graph the triple
/// would land in and typing atoms anywhere (the validator's `sh:class` reads
/// every graph); union rules check everything anywhere.
fn head_guard(
    head: &[TriplePattern],
    nulls: &[Variable],
    placements: &[Placement],
    atoms: &[Atom],
    scope: GraphScope,
) -> GraphPattern {
    let fresh = |t: &TermPattern| match t {
        TermPattern::Variable(v) if nulls.contains(v) => {
            TermPattern::Variable(Variable::new_unchecked(format!("__n_{}", v.as_str())))
        }
        other => other.clone(),
    };
    let mut plain: Vec<TriplePattern> = Vec::new();
    let mut graphed: Vec<GraphPattern> = Vec::new();
    for (j, tp) in head.iter().enumerate() {
        let t = TriplePattern {
            subject: fresh(&tp.subject),
            predicate: tp.predicate.clone(),
            object: fresh(&tp.object),
        };
        let graph = match scope {
            GraphScope::PerGraph if !is_rdf_type(&tp.predicate) => {
                placement_graph(&placements[j], placements, atoms, 0)
            }
            _ => None,
        };
        match graph {
            Some(g) => graphed.push(GraphPattern::Graph {
                name: g,
                inner: Box::new(GraphPattern::Bgp { patterns: vec![t] }),
            }),
            None => plain.push(t),
        }
    }
    let mut out = GraphPattern::Bgp { patterns: plain };
    for g in graphed {
        out = GraphPattern::Join {
            left: Box::new(out),
            right: Box::new(g),
        };
    }
    out
}

/// Parse a rule's text as a `CONSTRUCT` query. The `INSERT { … } WHERE { … }`
/// spelling `sh:construct` accepts is accepted too, and read the same way:
/// the leading keyword after the prologue becomes `CONSTRUCT`. Anything that
/// is not then a `CONSTRUCT` query is refused — a rule cannot write, drop or
/// name a destination graph.
pub fn parse_construct(body: &str) -> Result<Query, String> {
    let head_len = prologue_len(body);
    let rest = &body[head_len..];
    let token = leading_token(rest);
    let text = if token.eq_ignore_ascii_case("insert") {
        format!("{}CONSTRUCT{}", &body[..head_len], &rest[token.len()..])
    } else {
        body.to_string()
    };
    let query = spargebra::SparqlParser::new()
        .parse_query(&text)
        .map_err(|e| format!("the rule must be a CONSTRUCT query ({e})"))?;
    if !matches!(query, Query::Construct { .. }) {
        return Err(
            "the rule must be a CONSTRUCT query (`CONSTRUCT { … } WHERE { … }`), not another \
             query or an update"
                .to_string(),
        );
    }
    Ok(query)
}

/// Byte length of the leading `PREFIX`/`BASE` prologue, trailing whitespace
/// included: `body[..len]` is the prologue, `body[len..]` starts at the first
/// operation keyword. Zero when a declaration is unterminated (the parser
/// reports that).
fn prologue_len(body: &str) -> usize {
    let mut rest = body.trim_start();
    loop {
        let token = leading_token(rest);
        if !(token.eq_ignore_ascii_case("prefix") || token.eq_ignore_ascii_case("base")) {
            return body.len() - rest.len();
        }
        match rest.find('>') {
            Some(gt) => rest = rest[gt + 1..].trim_start(),
            None => return 0,
        }
    }
}

/// The first keyword-like token of `s` (up to whitespace, `<` or `{`).
fn leading_token(s: &str) -> &str {
    s.split(|c: char| c.is_whitespace() || c == '<' || c == '{')
        .next()
        .unwrap_or("")
}

/// Parse and check a rule (§3.1). `Err` is the message the 400 carries.
pub fn prepare(spec: RuleSpec) -> Result<PreparedRule, RuleError> {
    let iri = spec.iri.clone();
    prepare_inner(spec).map_err(|message| RuleError { rule: iri, message })
}

fn prepare_inner(spec: RuleSpec) -> Result<PreparedRule, String> {
    let query = parse_construct(&spec.construct)?;
    let Query::Construct {
        template,
        dataset,
        pattern,
        ..
    } = query
    else {
        return Err("the rule must be a CONSTRUCT query".into());
    };
    if dataset.is_some() {
        return Err(
            "FROM / FROM NAMED are not allowed: a rule reads the premises the run seeds".into(),
        );
    }
    // A CONSTRUCT is parsed as `SELECT *` over its WHERE clause: a top-level
    // projection of every in-scope variable, which is no part of the body.
    let pattern = unwrap_star(pattern);
    let prologue_end = prologue_len(&spec.construct);
    let prologue = spec.construct[..prologue_end].to_string();

    let nulls: Vec<Variable> = spec
        .nulls
        .iter()
        .map(|n| var(n))
        .collect::<Result<_, _>>()?;
    let kind = match &spec.equate {
        Some((a, b)) => {
            if !template.is_empty() {
                return Err("an EGD (ots:equate) must have an empty head".into());
            }
            Kind::Egd {
                a: var(a)?,
                b: var(b)?,
            }
        }
        None => Kind::Tgd,
    };
    check_template(&template, "head")?;
    let retract = match &spec.retract {
        Some(text) => {
            let tps = parse_template(&prologue, text).map_err(|e| format!("ots:retract: {e}"))?;
            check_template(&tps, "retract")?;
            tps
        }
        None => Vec::new(),
    };
    if !retract.is_empty() && matches!(kind, Kind::Egd { .. }) {
        return Err("an EGD cannot retract".into());
    }

    // The body.
    let bound: BTreeSet<Variable> = in_scope(&pattern).into_iter().collect();
    let mut rw = Rewriter {
        scope: spec.graph_scope,
        next_graph: 0,
        bnodes: HashMap::new(),
        atoms: Vec::new(),
        reads: PredSet::default(),
        negated: PredSet::default(),
    };
    let body = rw.rewrite(&pattern, None)?;
    if rw.atoms.is_empty() && spec.native.is_none() {
        // A rule with no atom fires without premises; nothing can say where
        // its head lands or why it fired.
        if !template.is_empty() || !retract.is_empty() {
            return Err("the body matches no triple pattern".into());
        }
    }

    // Head variables: body-bound or declared nulls; nulls never body-bound.
    for n in &nulls {
        if bound.contains(n) {
            return Err(format!(
                "?{} is declared in ots:nulls but the body binds it",
                n.as_str()
            ));
        }
    }
    for v in template_vars(&template) {
        if !bound.contains(&v) && !nulls.contains(&v) {
            return Err(format!(
                "head variable ?{} is neither bound by the body nor declared in ots:nulls",
                v.as_str()
            ));
        }
    }
    for v in template_vars(&retract) {
        if !bound.contains(&v) {
            return Err(format!(
                "retract variable ?{} is not bound by the body",
                v.as_str()
            ));
        }
    }
    if let Kind::Egd { a, b } = &kind {
        for v in [a, b] {
            if !bound.contains(v) {
                return Err(format!(
                    "ots:equate names ?{}, which the body does not bind",
                    v.as_str()
                ));
            }
        }
    }
    let head_vars = template_vars(&template);
    let frontier: Vec<Variable> = head_vars
        .iter()
        .filter(|v| bound.contains(*v))
        .cloned()
        .collect();

    let target = spec.target_graph.as_deref();
    let head_placement = place_template(&template, &rw.atoms, &nulls, target, false);
    let retract_placement = place_template(&retract, &rw.atoms, &nulls, target, true);
    let egd_placement = match &kind {
        Kind::Egd { a, b } => {
            let at = |v: &Variable| {
                target
                    .map(|g| Placement::Target(g.to_string()))
                    .unwrap_or_else(|| {
                        atom_for(&TermPattern::Variable(v.clone()), &rw.atoms)
                            .map(Placement::Atom)
                            .unwrap_or(Placement::Unknown)
                    })
            };
            Some((at(a), at(b)))
        }
        Kind::Tgd => None,
    };

    // The guard.
    let guard = match (&spec.guard, &kind) {
        (Some(text), _) => Some(parse_group(&prologue, text).map_err(|e| format!("guard: {e}"))?),
        (None, Kind::Tgd) if !template.is_empty() => Some(head_guard(
            &template,
            &nulls,
            &head_placement,
            &rw.atoms,
            spec.graph_scope,
        )),
        _ => None,
    };

    // Dependencies.
    let mut deps = Deps {
        reads: rw.reads.clone(),
        negated: rw.negated.clone(),
        ..Deps::default()
    };
    for tp in &template {
        deps.produces.add(&tp.predicate);
    }
    for tp in &retract {
        deps.retracts.add(&tp.predicate);
    }
    if let Kind::Egd { .. } = &kind {
        match spec.merge_mode {
            MergeMode::SameAsOnly => deps.produces.add_iri(OWL_SAME_AS),
            MergeMode::Rewrite => {
                deps.produces.all = true;
                deps.retracts.all = true;
            }
        }
    }
    match &spec.native {
        Some(Native::DatatypeRelabel { predicate, .. }) => {
            deps.produces.add_iri(predicate);
            deps.retracts.add_iri(predicate);
        }
        Some(Native::MaxCountKeepLexmin { predicate, .. }) => deps.retracts.add_iri(predicate),
        None => {}
    }

    let binds_this = bound.iter().any(|v| v.as_str() == "this");
    Ok(PreparedRule {
        kind,
        head: template,
        retract,
        nulls,
        frontier,
        atoms: rw.atoms,
        body,
        guard,
        deps,
        head_placement,
        retract_placement,
        egd_placement,
        binds_this,
        spec,
    })
}

impl PreparedRule {
    /// The evaluation query: the body, `extra` joined in (a focus or delta
    /// `VALUES`), the guard, every in-scope variable projected.
    pub fn select(&self, extra: Option<GraphPattern>) -> (Query, Vec<Variable>) {
        let mut pattern = self.body.clone();
        if let Some(v) = extra {
            pattern = GraphPattern::Join {
                left: Box::new(v),
                right: Box::new(pattern),
            };
        }
        if let Some(g) = &self.guard {
            pattern = GraphPattern::Filter {
                expr: Expression::Not(Box::new(Expression::Exists(Box::new(g.clone())))),
                inner: Box::new(pattern),
            };
        }
        let variables = in_scope(&pattern);
        (
            Query::Select {
                dataset: None,
                pattern: GraphPattern::Project {
                    inner: Box::new(pattern),
                    variables: variables.clone(),
                },
                base_iri: None,
            },
            variables,
        )
    }

    /// Whether this rule runs in the last stratum.
    pub fn is_policy_deletion(&self) -> bool {
        self.spec.policy_deletion
            || (matches!(self.kind, Kind::Egd { .. }) && self.spec.merge_mode == MergeMode::Rewrite)
            || matches!(self.spec.native, Some(Native::MaxCountKeepLexmin { .. }))
    }
}

/// An RDF term as a SPARQL `VALUES` cell; `None` for a blank node (no
/// SPARQL syntax addresses a stored one) or a triple term.
pub fn ground(t: &oxigraph::model::Term) -> Option<GroundTerm> {
    match t {
        oxigraph::model::Term::NamedNode(n) => Some(GroundTerm::NamedNode(n.clone())),
        oxigraph::model::Term::Literal(l) => Some(GroundTerm::Literal(l.clone())),
        _ => None,
    }
}

/// `VALUES ?var { terms }`.
pub fn values(var: &Variable, terms: &[oxigraph::model::Term]) -> GraphPattern {
    GraphPattern::Values {
        variables: vec![var.clone()],
        bindings: terms
            .iter()
            .filter_map(ground)
            .map(|g| vec![Some(g)])
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(construct: &str) -> RuleSpec {
        RuleSpec::new("urn:r", construct, Origin::Authored)
    }

    #[test]
    fn a_ground_rule_is_rewritten_into_graph_scoped_atoms() {
        let r = prepare(spec(
            "PREFIX ex: <http://example.org/> CONSTRUCT { ?x ex:hasPart ?z } WHERE { ?x ex:hasPart ?y . ?y ex:hasPart ?z . FILTER(?x != ?z) }",
        ))
        .unwrap();
        assert_eq!(r.kind, Kind::Tgd);
        assert_eq!(r.atoms.len(), 2);
        // Union scope: one graph variable per atom.
        assert_ne!(
            format!("{:?}", r.atoms[0].graph),
            format!("{:?}", r.atoms[1].graph)
        );
        assert!(r.deps.reads.iris.contains("http://example.org/hasPart"));
        assert!(r.deps.produces.iris.contains("http://example.org/hasPart"));
        assert!(r.guard.is_some(), "a TGD gets the head guard");
        assert_eq!(r.head_placement, vec![Placement::Atom(0)]);
        let (q, vars) = r.select(None);
        assert!(vars.iter().any(|v| v.as_str() == "__g0"));
        // The query is a SELECT that parses back.
        spargebra::SparqlParser::new()
            .parse_query(&q.to_string())
            .unwrap();
    }

    #[test]
    fn per_graph_scope_shares_one_graph_variable() {
        let mut s = spec("PREFIX ex: <http://example.org/> CONSTRUCT { ?x ex:p ?y } WHERE { ?x ex:q ?y . ?y ex:r ?x }");
        s.graph_scope = GraphScope::PerGraph;
        let r = prepare(s).unwrap();
        assert_eq!(r.atoms[0].graph, r.atoms[1].graph);
    }

    #[test]
    fn nulls_and_the_head_guard() {
        let mut s = spec(
            "PREFIX ex: <http://example.org/> CONSTRUCT { ?this ex:hasDeck ?w . ?w a ex:Deck } WHERE { GRAPH ?g { ?this a ex:Bridge } }",
        );
        s.nulls = vec!["w".into()];
        s.graph_scope = GraphScope::PerGraph;
        let r = prepare(s).unwrap();
        assert_eq!(r.nulls.len(), 1);
        assert_eq!(
            r.frontier.iter().map(|v| v.as_str()).collect::<Vec<_>>(),
            ["this"]
        );
        assert_eq!(
            r.head_placement,
            vec![Placement::Atom(0), Placement::Head(0)]
        );
        // Value atom in the focus graph, typing atom anywhere.
        let g = r.guard.as_ref().unwrap().to_string();
        assert!(g.contains("GRAPH ?g"), "{g}");
        assert!(g.contains("__n_w"), "{g}");
        assert!(r.binds_this);
    }

    #[test]
    fn the_checker_refuses_what_the_note_forbids() {
        let err = |c: &str| prepare(spec(c)).unwrap_err().message;
        assert!(err("CONSTRUCT { ?x <urn:p> _:w } WHERE { ?x <urn:q> ?y }").contains("ots:nulls"));
        assert!(
            err("CONSTRUCT { ?x <urn:p> ?w } WHERE { ?x <urn:q> ?y }").contains("neither bound")
        );
        assert!(err(
            "CONSTRUCT { ?x <urn:p> ?y } WHERE { ?x <urn:q> ?y OPTIONAL { ?y <urn:r> ?z } }"
        )
        .contains("OPTIONAL"));
        assert!(err(
            "CONSTRUCT { ?x <urn:p> ?y } WHERE { { ?x <urn:q> ?y } UNION { ?x <urn:r> ?y } }"
        )
        .contains("UNION"));
        assert!(
            err("CONSTRUCT { ?x <urn:p> ?y } WHERE { ?x <urn:q>+ ?y }").contains("property path")
        );
        assert!(
            err("CONSTRUCT { ?x <urn:p> ?y } FROM <urn:g> WHERE { ?x <urn:q> ?y }")
                .contains("FROM")
        );
        assert!(err("SELECT * WHERE { ?x <urn:q> ?y }").contains("CONSTRUCT"));
        let mut s = spec("CONSTRUCT { ?x <urn:p> ?y } WHERE { ?x <urn:q> ?y }");
        s.equate = Some(("x".into(), "y".into()));
        assert!(prepare(s).unwrap_err().message.contains("empty head"));
        let mut s = spec("CONSTRUCT { ?x <urn:p> ?w } WHERE { ?x <urn:q> ?w }");
        s.nulls = vec!["w".into()];
        assert!(prepare(s).unwrap_err().message.contains("binds it"));
    }

    #[test]
    fn negation_and_retracts_are_recorded_for_stratification() {
        let mut s = spec(
            "PREFIX ex: <http://example.org/> CONSTRUCT { ?x ex:length ?m ; ex:unit \"m\" } WHERE { ?x ex:length ?v ; ex:unit \"mm\" . BIND(?v / 1000 AS ?m) FILTER NOT EXISTS { ?x ex:locked true } }",
        );
        s.retract = Some("?x ex:length ?v ; ex:unit \"mm\"".into());
        let r = prepare(s).unwrap();
        assert!(r.deps.negated.iris.contains("http://example.org/locked"));
        assert!(r.deps.retracts.iris.contains("http://example.org/unit"));
        assert_eq!(r.retract.len(), 2);
        // Each retract triple is placed where its own atom matched.
        assert_eq!(
            r.retract_placement,
            vec![Placement::Atom(0), Placement::Atom(1)]
        );
        // A variable predicate in a negation mentions every predicate.
        let r = prepare(spec(
            "CONSTRUCT { ?x <urn:p> ?y } WHERE { ?x <urn:q> ?y FILTER NOT EXISTS { ?x ?any ?y } }",
        ))
        .unwrap();
        assert!(r.deps.negated.all);
    }

    #[test]
    fn an_egd_produces_same_as_or_everything() {
        let mut s = spec(
            "PREFIX ex: <http://example.org/> CONSTRUCT { } WHERE { ?x a ex:A ; ex:code ?k . ?y a ex:A ; ex:code ?k . FILTER(?x != ?y) }",
        );
        s.equate = Some(("?x".into(), "?y".into()));
        let r = prepare(s.clone()).unwrap();
        assert!(matches!(r.kind, Kind::Egd { .. }));
        assert!(r.deps.produces.iris.contains(OWL_SAME_AS));
        assert!(r.guard.is_none());
        assert_eq!(
            r.egd_placement,
            Some((Placement::Atom(0), Placement::Atom(2)))
        );
        s.merge_mode = MergeMode::Rewrite;
        let r = prepare(s).unwrap();
        assert!(r.deps.produces.all && r.deps.retracts.all);
        assert!(r.is_policy_deletion());
    }
}
