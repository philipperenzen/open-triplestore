//! SWRL rule evaluation engine.
//!
//! A rule without built-ins or data ranges is translated to one SPARQL
//! `INSERT … WHERE …` update. A rule with them runs natively: its class,
//! property and individual atoms become a SPARQL `SELECT`, the built-in and
//! data-range atoms are evaluated in Rust over each solution
//! ([`super::builtins`], [`super::datarange`]) — so a built-in can bind a
//! variable, split a value into its components or enumerate solutions — and
//! the head triples are written in one batch. Either way the rules run in a
//! fixed-point loop until an iteration derives nothing new.
//!
//! A class-expression atom (`ObjectSomeValuesFrom(p B)(?x)`) is replaced by a
//! class atom over an auxiliary named class `urn:ots:swrl:aux:<hash>`, and
//! `aux owl:equivalentClass <expression>` joins the target graph, which the
//! regime the rules run with reads: the regime materialises who belongs to the
//! expression ([`super::expr::AuxClass`]). Such rules need a regime.
//!
//! Translation ([`compile_rules_for_regime`]) refuses, with a message, every rule it
//! cannot run as written:
//! - an unsafe rule: a variable in the head that occurs nowhere in the body
//!   (SWRL §2–3 safety);
//! - a built-in in the head, or a data range in the head;
//! - a built-in outside `swrlb:` §8, or with the wrong number of arguments;
//! - a built-in or data range no order of the body can evaluate: one whose
//!   unbound arguments have infinitely many solutions (`add(?z, ?x, ?y)` with
//!   only `?z`… bound by nothing);
//! - a constant in the wrong kind of position (a literal where an individual
//!   belongs, or the other way round), and a variable used as both.
//!
//! Variables may be named by any IRI (`urn:swrl:var#x`, the OWL API's
//! `urn:swrl#x`, an ontology-namespace IRI) or by `?x` in the text form; each
//! is mapped to a generated SPARQL variable, so the name never reaches the
//! query text.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use oxigraph::model::{
    GraphName, GraphNameRef, Literal, NamedNode, NamedNodeRef, NamedOrBlankNode, Quad, Term,
};
use serde::Serialize;
use tracing::{debug, info, warn};

use super::builtins::{self, EvalCtx, ListSource, Step, StepArg, StepKind};
use super::datarange;
use super::expr::{AuxClass, ClassExpr, DataRange};
use crate::store::TripleStore;

/// A SWRL rule with antecedent (body) and consequent (head).
#[derive(Debug, Clone)]
pub struct SwrlRule {
    pub name: Option<String>,
    pub body: Vec<Atom>,
    pub head: Vec<Atom>,
}

/// An atom in a SWRL rule.
#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)] // variant names mirror the W3C SWRL atom types
pub enum Atom {
    /// Class membership: Class(?x) → ?x rdf:type Class
    ClassAtom { class_iri: String, arg: SwrlArg },
    /// Membership in a class expression: `ObjectSomeValuesFrom(p B)(?x)`.
    ClassExpressionAtom { expr: ClassExpr, arg: SwrlArg },
    /// Object property: prop(?x, ?y) → ?x prop ?y, where ?y is an individual
    ObjectPropertyAtom {
        property: String,
        arg1: SwrlArg,
        arg2: SwrlArg,
    },
    /// Data property: prop(?x, ?y) → ?x prop ?y, where ?y is a literal
    DataPropertyAtom {
        property: String,
        arg1: SwrlArg,
        arg2: SwrlArg,
    },
    /// A property atom of unknown kind: prop(?x, ?y) → ?x prop ?y, where ?y may
    /// be an individual or a literal. Only the text form produces it, as it has
    /// no way to say which kind of property it means.
    PropertyAtom {
        property: String,
        arg1: SwrlArg,
        arg2: SwrlArg,
    },
    /// owl:sameAs assertion
    SameIndividualAtom { arg1: SwrlArg, arg2: SwrlArg },
    /// owl:differentFrom assertion
    DifferentIndividualsAtom { arg1: SwrlArg, arg2: SwrlArg },
    /// Membership of a data value in a data range: `xsd:integer(?v)`.
    DataRangeAtom { range: DataRange, arg: SwrlArg },
    /// Built-in predicate (SWRL §8)
    BuiltinAtom { builtin: String, args: Vec<SwrlArg> },
}

/// An argument in a SWRL atom.
#[derive(Debug, Clone, Serialize)]
pub enum SwrlArg {
    /// A SWRL variable, named by any IRI (`urn:swrl:var#x`) or `?x`
    Variable(String),
    /// A named individual IRI
    Individual(String),
    /// A literal value with an optional datatype or language tag
    Literal {
        value: String,
        datatype: Option<String>,
        language: Option<String>,
    },
}

/// What a variable is bound to, by the positions it occurs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sort {
    Individual,
    Data,
}

impl Sort {
    fn describe(self) -> &'static str {
        match self {
            Sort::Individual => "an individual",
            Sort::Data => "a data value",
        }
    }
}

/// Per-rule translation state: the generated variable names and what each
/// variable is bound to.
#[derive(Default)]
struct RuleScope {
    /// Source variable name → generated SPARQL variable name.
    vars: HashMap<String, String>,
    /// Source variable name → the sort its typed positions give it.
    sorts: HashMap<String, Sort>,
}

impl RuleScope {
    /// The generated SPARQL variable for a source variable name: `?v<n>`, plus
    /// the ASCII letters and digits of the name's last segment for legibility
    /// (`urn:swrl:var#x` → `?v0_x`). Distinct names never share a variable.
    fn var(&mut self, name: &str) -> Result<String, String> {
        if name.trim().trim_start_matches('?').is_empty() {
            return Err("SWRL variable with an empty name".to_string());
        }
        let next = self.vars.len();
        Ok(self
            .vars
            .entry(name.to_string())
            .or_insert_with(|| {
                let tail = name
                    .rsplit(['#', '/', ':', '?'])
                    .find(|seg| !seg.is_empty())
                    .unwrap_or("");
                let hint: String = tail
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .take(32)
                    .collect();
                if hint.is_empty() {
                    format!("?v{next}")
                } else {
                    format!("?v{next}_{hint}")
                }
            })
            .clone())
    }

    /// Record that `arg` stands in a position of `sort`. A constant of the
    /// wrong kind, or a variable already used as the other kind, is refused:
    /// such an atom can never hold, so the rule would silently never fire.
    fn require(&mut self, arg: &SwrlArg, sort: Sort, position: &str) -> Result<(), String> {
        match (arg, sort) {
            (SwrlArg::Variable(v), _) => match self.sorts.insert(v.clone(), sort) {
                Some(prev) if prev != sort => Err(format!(
                    "SWRL variable '{v}' is used as {} and as {} ({position})",
                    prev.describe(),
                    sort.describe()
                )),
                _ => Ok(()),
            },
            (SwrlArg::Literal { value, .. }, Sort::Individual) => Err(format!(
                "{position} takes an individual, found the literal \"{value}\""
            )),
            (SwrlArg::Individual(iri), Sort::Data) => Err(format!(
                "{position} takes a data value, found the individual <{iri}>"
            )),
            _ => Ok(()),
        }
    }

    /// Render an argument as a SPARQL term.
    ///
    /// Every constant goes through the oxrdf constructors and their `Display`
    /// impls, which quote and escape to N-Triples syntax. Formatting these by
    /// hand — `format!("\"{value}\"")` / `format!("<{iri}>")` — let a literal
    /// containing a quote or an IRI containing `>` close the generated
    /// `INSERT { … } WHERE { … }` early and append attacker-chosen SPARQL. A
    /// term that cannot be represented (an invalid IRI, a bad datatype) is
    /// rejected here rather than being spliced in as text. Variables are
    /// replaced by generated names ([`RuleScope::var`]).
    fn term(&mut self, arg: &SwrlArg) -> Result<String, String> {
        match arg {
            SwrlArg::Variable(v) => self.var(v),
            other => constant_to_sparql(other),
        }
    }

    /// An argument as a step argument: a generated variable name (without
    /// `?`) or a constant term.
    fn step_arg(&mut self, arg: &SwrlArg) -> Result<StepArg, String> {
        Ok(match arg {
            SwrlArg::Variable(v) => StepArg::Var(self.var(v)?.trim_start_matches('?').to_string()),
            other => StepArg::Const(constant_term(other)?),
        })
    }
}

/// A constant argument (individual or literal) as an RDF term.
pub(crate) fn constant_term(arg: &SwrlArg) -> Result<Term, String> {
    match arg {
        SwrlArg::Variable(v) => Err(format!("'{v}' is a variable, not a constant")),
        SwrlArg::Individual(iri) => {
            let trimmed = iri.trim_start_matches('<').trim_end_matches('>');
            NamedNode::new(trimmed)
                .map(Term::NamedNode)
                .map_err(|e| format!("Invalid SWRL individual IRI '{trimmed}': {e}"))
        }
        SwrlArg::Literal {
            value,
            language: Some(lang),
            ..
        } => Literal::new_language_tagged_literal(value.as_str(), lang.as_str())
            .map(Term::Literal)
            .map_err(|e| format!("Invalid SWRL literal language tag '{lang}': {e}")),
        SwrlArg::Literal {
            value,
            datatype: Some(dt),
            ..
        } => {
            let dt_node = NamedNode::new(dt.as_str())
                .map_err(|e| format!("Invalid SWRL literal datatype '{dt}': {e}"))?;
            Ok(Literal::new_typed_literal(value.as_str(), dt_node).into())
        }
        SwrlArg::Literal { value, .. } => Ok(Literal::new_simple_literal(value.as_str()).into()),
    }
}

/// Render a constant argument (individual or literal) as a SPARQL term.
fn constant_to_sparql(arg: &SwrlArg) -> Result<String, String> {
    constant_term(arg).map(|t| t.to_string())
}

/// Check that a class or property predicate is an absolute IRI.
///
/// Both rule syntaxes hand predicates over verbatim (`Person(?x)` in the text
/// form, the `IRI` attribute in OWL/XML), so this is where a predicate that is
/// not an IRI is refused with a message naming it. Surrounding `<…>` is
/// tolerated, as the text form allows it.
pub(crate) fn validate_predicate_iri(iri: &str, what: &str) -> Result<NamedNode, String> {
    let trimmed = iri.trim_start_matches('<').trim_end_matches('>');
    NamedNode::new(trimmed).map_err(|e| {
        format!("Invalid SWRL {what} IRI '{trimmed}': {e} (predicates must be absolute IRIs)")
    })
}

/// Render a class or property predicate as a SPARQL IRI term.
///
/// Same rule as [`constant_to_sparql`]: the text is validated by `NamedNode`
/// and printed by its `Display`, never pasted between angle brackets. The
/// parser already refuses bad predicates; this is the second line, for rules
/// built in code or by a future syntax.
fn predicate_to_sparql(iri: &str, what: &str) -> Result<String, String> {
    Ok(validate_predicate_iri(iri, what)?.to_string())
}

/// Check that a target graph is an absolute IRI. It is written into the
/// generated update as `GRAPH <…>`, so anything else is refused, not spliced.
pub fn validate_target_graph(iri: &str) -> Result<NamedNode, String> {
    NamedNode::new(iri)
        .map_err(|e| format!("Invalid target_graph '{iri}': {e} (it must be an absolute IRI)"))
}

/// Why the fixed-point loop stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// An iteration derived nothing new: the closure is complete.
    Fixpoint,
    /// `max_iterations` ran out first.
    MaxIterations,
    /// The server's time limit ran out first.
    Timeout,
}

/// Result of SWRL rule execution.
#[derive(Debug, Clone, Serialize)]
pub struct SwrlExecutionResult {
    /// Number of rules processed.
    pub rules_count: usize,
    /// Iterations of the fixed-point loop that ran (fully or in part).
    pub iterations: usize,
    /// New triples written to the target graph by this run.
    pub triples_inferred: usize,
    /// Whether the run reached its fixed point. `false` means more could be
    /// derived: `max_iterations` or the time limit stopped it first.
    pub converged: bool,
    /// Why the loop stopped.
    pub stop_reason: StopReason,
    /// Per-rule execution details.
    pub rule_results: Vec<RuleResult>,
}

/// Result for a single rule execution.
#[derive(Debug, Clone, Serialize)]
pub struct RuleResult {
    pub rule_name: String,
    /// The SPARQL the rule runs: its `INSERT … WHERE …`, or for a rule with
    /// built-ins the `SELECT` over its body atoms followed by the built-ins
    /// evaluated natively (as comments).
    pub sparql: String,
    pub success: bool,
    pub error: Option<String>,
}

/// What a head triple's object may be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObjectKind {
    Individual,
    Data,
    Either,
}

/// One head atom as a triple template.
#[derive(Debug, Clone)]
struct HeadTriple {
    subject: StepArg,
    predicate: NamedNode,
    object: StepArg,
    object_kind: ObjectKind,
}

/// A rule evaluated natively: a SELECT over its body atoms, built-in steps,
/// head templates.
#[derive(Debug, Clone)]
struct NativeRule {
    select: String,
    steps: Vec<Step>,
    head: Vec<HeadTriple>,
}

#[derive(Debug, Clone)]
enum Plan {
    Insert(String),
    Native(NativeRule),
}

#[derive(Debug, Clone)]
struct CompiledRule {
    name: String,
    /// The rule's SPARQL, as reported.
    sparql: String,
    plan: Plan,
}

/// Rules translated to SPARQL, ready to run.
#[derive(Debug, Clone)]
pub struct CompiledRules {
    rules: Vec<CompiledRule>,
    target: Option<NamedNode>,
    /// The graphs rule bodies read, the target included; `None` reads the
    /// default graph.
    scope: Option<Vec<String>>,
    /// Class expressions the rules use, named by auxiliary classes.
    aux: Vec<AuxClass>,
}

impl CompiledRules {
    /// Write `aux owl:equivalentClass <expression>` for every auxiliary class
    /// the target graph does not define yet, so the regime reasons over it.
    /// Returns the number of triples written.
    pub fn install_aux(&self, store: &TripleStore) -> Result<usize, String> {
        if self.aux.is_empty() {
            return Ok(0);
        }
        let Some(target) = self.target.as_ref() else {
            return Err("class-expression atoms need a target graph".to_string());
        };
        let eqc = NamedNodeRef::new_unchecked("http://www.w3.org/2002/07/owl#equivalentClass");
        let mut quads = Vec::new();
        for aux in &self.aux {
            let node = NamedNode::new_unchecked(aux.iri.as_str());
            let present = store
                .store()
                .quads_for_pattern(
                    Some(node.as_ref().into()),
                    Some(eqc),
                    None,
                    Some(GraphNameRef::NamedNode(target.as_ref())),
                )
                .next()
                .is_some();
            if present {
                continue;
            }
            for t in aux.axioms() {
                quads.push(Quad::new(
                    t.subject,
                    t.predicate,
                    t.object,
                    GraphName::NamedNode(target.clone()),
                ));
            }
        }
        let n = quads.len();
        if n > 0 {
            store
                .insert_quads(quads)
                .map_err(|e| format!("writing the auxiliary class axioms: {e}"))?;
        }
        Ok(n)
    }
}

/// Translate every rule, refusing the whole set if any rule cannot run as
/// written. Running the rest would hand back a closure the caller did not
/// ask for, so the error names each refused rule and nothing runs.
///
/// Rule bodies read the default graph, or with `sources` the merge of those
/// graphs plus the target graph, so a rule sees what earlier iterations
/// derived. A scoped run must name its target: the default graph cannot be
/// part of a scope.
///
/// With `regime` (the rules run jointly with it) class-expression atoms are
/// accepted, as the regime materialises the auxiliary classes standing for
/// them ([`CompiledRules::install_aux`]); without one they are refused, as
/// nothing would compute who belongs to the expression.
pub fn compile_rules_for_regime(
    rules: &[SwrlRule],
    target_graph: Option<&str>,
    sources: Option<&[String]>,
    regime: Option<&str>,
) -> Result<CompiledRules, String> {
    let target = target_graph.map(validate_target_graph).transpose()?;
    let scope = match sources {
        None => None,
        Some(sources) => {
            let Some(t) = target.as_ref() else {
                return Err(
                    "a run scoped to named graphs needs a target_graph: what it derives \
                     must be readable by its next iteration"
                        .to_string(),
                );
            };
            let mut scope = Vec::with_capacity(sources.len() + 1);
            for g in sources {
                NamedNode::new(g.as_str())
                    .map_err(|e| format!("Invalid source graph '{g}': {e}"))?;
                if !scope.contains(g) {
                    scope.push(g.clone());
                }
            }
            if !scope.iter().any(|g| g == t.as_str()) {
                scope.push(t.as_str().to_string());
            }
            Some(scope)
        }
    };
    let mut compiled = Vec::with_capacity(rules.len());
    let mut errors = Vec::new();
    let mut aux: Vec<AuxClass> = Vec::new();
    for (i, rule) in rules.iter().enumerate() {
        let name = rule
            .name
            .clone()
            .unwrap_or_else(|| format!("rule_{}", i + 1));
        let outcome = lower_class_expressions(rule, &mut aux, regime)
            .and_then(|lowered| compile_rule(&lowered, target.as_ref()));
        match outcome {
            Ok((sparql, plan)) => compiled.push(CompiledRule { name, sparql, plan }),
            Err(e) => errors.push(format!("rule '{name}': {e}")),
        }
    }
    if !errors.is_empty() {
        return Err(format!("SWRL rules refused: {}", errors.join("; ")));
    }
    if !aux.is_empty() && target.is_none() {
        return Err(
            "SWRL rules refused: class-expression atoms need a target_graph, where the \
             auxiliary class axioms are written for the regime"
                .to_string(),
        );
    }
    Ok(CompiledRules {
        rules: compiled,
        target,
        scope,
        aux,
    })
}

/// Replace each class-expression atom by a class atom over its auxiliary
/// class (a named class stays a plain class atom).
fn lower_class_expressions(
    rule: &SwrlRule,
    aux: &mut Vec<AuxClass>,
    regime: Option<&str>,
) -> Result<SwrlRule, String> {
    let mut lower = |atoms: &[Atom]| -> Result<Vec<Atom>, String> {
        atoms
            .iter()
            .map(|a| match a {
                Atom::ClassExpressionAtom {
                    expr: ClassExpr::Named(c),
                    arg,
                } => Ok(Atom::ClassAtom {
                    class_iri: c.clone(),
                    arg: arg.clone(),
                }),
                Atom::ClassExpressionAtom { expr, arg } => {
                    expr.validate()?;
                    if regime.is_none() {
                        return Err(format!(
                            "the class-expression atom {expr}: who belongs to a class \
                             expression is computed by an entailment regime. Run the rules \
                             with \"regime\" (owl2-rl, or owl2-dl with a DL backend) or store \
                             them with a dataset whose regime is in materialize mode"
                        ));
                    }
                    let a = AuxClass::new(expr.clone());
                    let class_iri = a.iri.clone();
                    if !aux.iter().any(|x| x.iri == a.iri) {
                        aux.push(a);
                    }
                    Ok(Atom::ClassAtom {
                        class_iri,
                        arg: arg.clone(),
                    })
                }
                other => Ok(other.clone()),
            })
            .collect()
    };
    Ok(SwrlRule {
        name: rule.name.clone(),
        body: lower(&rule.body)?,
        head: lower(&rule.head)?,
    })
}

/// Reads list cells from the graphs a rule reads.
struct StoreLists<'a> {
    store: &'a oxigraph::store::Store,
    /// `None`: the default graph.
    graphs: Option<Vec<NamedNode>>,
}

impl ListSource for StoreLists<'_> {
    fn cell(&self, node: &Term) -> Option<(Term, Term)> {
        let subject: NamedOrBlankNode = match node {
            Term::NamedNode(n) => n.clone().into(),
            Term::BlankNode(b) => b.clone().into(),
            _ => return None,
        };
        let first = NamedNodeRef::new_unchecked("http://www.w3.org/1999/02/22-rdf-syntax-ns#first");
        let rest = NamedNodeRef::new_unchecked("http://www.w3.org/1999/02/22-rdf-syntax-ns#rest");
        let graphs: Vec<GraphNameRef<'_>> = match &self.graphs {
            Some(gs) => gs
                .iter()
                .map(|g| GraphNameRef::NamedNode(g.as_ref()))
                .collect(),
            None => vec![GraphNameRef::DefaultGraph],
        };
        let object = |p: NamedNodeRef<'_>| -> Option<Term> {
            graphs.iter().find_map(|g| {
                self.store
                    .quads_for_pattern(Some(subject.as_ref()), Some(p), None, Some(*g))
                    .filter_map(Result::ok)
                    .map(|q| q.object)
                    .next()
            })
        };
        Some((object(first)?, object(rest)?))
    }
}

/// The most bindings one native rule may produce in one iteration.
const MAX_BINDINGS: usize = 1_000_000;

/// Run compiled rules until an iteration derives nothing new, `max_iterations`
/// iterations have run, or `deadline` passes (checked before each rule).
///
/// Only what this run writes is counted: the size of the target graph (the
/// default graph when there is none) before and after, not the whole store.
pub fn execute_compiled(
    store: &TripleStore,
    compiled: &CompiledRules,
    max_iterations: usize,
    deadline: Option<Instant>,
) -> Result<SwrlExecutionResult, String> {
    info!(
        "Executing {} SWRL rules (max {} iterations)",
        compiled.rules.len(),
        max_iterations
    );
    let target = compiled.target.as_ref().map(|t| t.as_str());
    let count = || {
        store
            .count_graph(target)
            .map_err(|e| format!("counting the target graph: {e}"))
    };
    let expired = || deadline.is_some_and(|d| Instant::now() >= d);

    let lists = StoreLists {
        store: store.store(),
        graphs: compiled.scope.as_ref().map(|s| {
            s.iter()
                .map(|g| NamedNode::new_unchecked(g.as_str()))
                .collect()
        }),
    };
    let mut ctx = EvalCtx::new(&lists);

    let start_count = count()?;
    let mut rule_results: Vec<RuleResult> = compiled
        .rules
        .iter()
        .map(|r| RuleResult {
            rule_name: r.name.clone(),
            sparql: r.sparql.clone(),
            success: true,
            error: None,
        })
        .collect();
    let mut iterations = 0;
    let mut stop_reason = StopReason::MaxIterations;

    'fixpoint: while iterations < max_iterations {
        if expired() {
            stop_reason = StopReason::Timeout;
            break;
        }
        iterations += 1;
        let count_before = count()?;
        for (rule, result) in compiled.rules.iter().zip(rule_results.iter_mut()) {
            if expired() {
                stop_reason = StopReason::Timeout;
                break 'fixpoint;
            }
            let outcome = match &rule.plan {
                // Through `TripleStore::update`, not a raw `parse_update(..).execute()`
                // on the inner store: the store's write guard (mirror, query cache)
                // and per-graph count maintenance must see these writes like any
                // other update.
                Plan::Insert(sparql) => match &compiled.scope {
                    Some(scope) => store.update_scoped(sparql, scope),
                    None => store.update(sparql),
                }
                .map_err(|e| e.to_string()),
                Plan::Native(native) => run_native(store, compiled, native, &mut ctx),
            };
            if let Err(e) = outcome {
                warn!("Rule {} failed: {}", rule.name, e);
                if result.success {
                    result.success = false;
                    result.error = Some(e);
                }
            }
        }
        let new_triples = count()?.saturating_sub(count_before);
        debug!("Iteration {}: {} new triples", iterations, new_triples);
        if new_triples == 0 {
            stop_reason = StopReason::Fixpoint;
            break;
        }
    }

    let triples_inferred = count()?.saturating_sub(start_count);
    match stop_reason {
        StopReason::Fixpoint => info!(
            "Fixed point reached after {} iterations ({} triples inferred)",
            iterations, triples_inferred
        ),
        other => warn!(
            "SWRL run stopped before its fixed point ({:?}) after {} iterations",
            other, iterations
        ),
    }

    Ok(SwrlExecutionResult {
        rules_count: compiled.rules.len(),
        iterations,
        triples_inferred,
        converged: stop_reason == StopReason::Fixpoint,
        stop_reason,
        rule_results,
    })
}

/// One pass of a native rule: SELECT the body's solutions, run the steps over
/// each, write the head triples (and the cells of lists minted for them).
fn run_native(
    store: &TripleStore,
    compiled: &CompiledRules,
    rule: &NativeRule,
    ctx: &mut EvalCtx<'_>,
) -> Result<(), String> {
    let results = match &compiled.scope {
        Some(scope) => store.query_scoped(&rule.select, scope),
        None => store.query(&rule.select),
    }
    .map_err(|e| e.to_string())?;
    let oxigraph::sparql::QueryResults::Solutions(solutions) = results else {
        return Err("the rule body query returned no solutions table".to_string());
    };
    let mut bindings: Vec<HashMap<String, Term>> = Vec::new();
    for solution in solutions {
        let solution = solution.map_err(|e| e.to_string())?;
        let mut b = HashMap::new();
        for (var, term) in solution.iter() {
            b.insert(var.as_str().to_string(), term.clone());
        }
        bindings.push(b);
        if bindings.len() > MAX_BINDINGS {
            return Err(format!(
                "the rule body has more than {MAX_BINDINGS} solutions in one iteration"
            ));
        }
    }
    for step in &rule.steps {
        let mut next = Vec::new();
        for b in &bindings {
            next.extend(step.eval(b, ctx));
            if next.len() > MAX_BINDINGS {
                return Err(format!(
                    "{} produced more than {MAX_BINDINGS} solutions in one iteration",
                    step.label
                ));
            }
        }
        bindings = next;
        if bindings.is_empty() {
            return Ok(());
        }
    }

    let graph = match &compiled.target {
        Some(t) => GraphName::NamedNode(t.clone()),
        None => GraphName::DefaultGraph,
    };
    let resolve = |a: &StepArg, b: &HashMap<String, Term>| -> Option<Term> {
        match a {
            StepArg::Const(t) => Some(t.clone()),
            StepArg::Var(v) => b.get(v).cloned(),
        }
    };
    let mut quads: HashSet<Quad> = HashSet::new();
    let mut minted_used: HashSet<String> = HashSet::new();
    for b in &bindings {
        for h in &rule.head {
            let (Some(s), Some(o)) = (resolve(&h.subject, b), resolve(&h.object, b)) else {
                continue;
            };
            let subject: NamedOrBlankNode = match &s {
                Term::NamedNode(n) => n.clone().into(),
                Term::BlankNode(n) => n.clone().into(),
                // A literal cannot be a subject: no triple to assert.
                _ => continue,
            };
            let fits = match h.object_kind {
                ObjectKind::Individual => !matches!(o, Term::Literal(_)),
                ObjectKind::Data => matches!(o, Term::Literal(_)),
                ObjectKind::Either => true,
            };
            if !fits {
                continue;
            }
            for t in [&s, &o] {
                if let Term::NamedNode(n) = t {
                    if ctx.minted.contains_key(n.as_str()) {
                        minted_used.insert(n.as_str().to_string());
                    }
                }
            }
            quads.insert(Quad::new(subject, h.predicate.clone(), o, graph.clone()));
        }
    }
    let mut cells = Vec::new();
    for node in &minted_used {
        builtins::minted_cells(ctx, node, &mut cells);
    }
    for (s, p, o) in cells {
        if let Term::NamedNode(s) = s {
            quads.insert(Quad::new(s, NamedNode::new_unchecked(p), o, graph.clone()));
        }
    }
    let fresh: Vec<Quad> = quads
        .into_iter()
        .filter(|q| !store.store().contains(q).unwrap_or(false))
        .collect();
    if fresh.is_empty() {
        return Ok(());
    }
    store.insert_quads(fresh).map_err(|e| e.to_string())
}

/// The variables an argument list mentions.
fn variables<'a>(args: impl IntoIterator<Item = &'a SwrlArg>) -> impl Iterator<Item = &'a str> {
    args.into_iter().filter_map(|a| match a {
        SwrlArg::Variable(v) => Some(v.as_str()),
        _ => None,
    })
}

/// The arguments of an atom, in order.
fn atom_args(atom: &Atom) -> Vec<&SwrlArg> {
    match atom {
        Atom::ClassAtom { arg, .. }
        | Atom::ClassExpressionAtom { arg, .. }
        | Atom::DataRangeAtom { arg, .. } => vec![arg],
        Atom::ObjectPropertyAtom { arg1, arg2, .. }
        | Atom::DataPropertyAtom { arg1, arg2, .. }
        | Atom::PropertyAtom { arg1, arg2, .. }
        | Atom::SameIndividualAtom { arg1, arg2 }
        | Atom::DifferentIndividualsAtom { arg1, arg2 } => vec![arg1, arg2],
        Atom::BuiltinAtom { args, .. } => args.iter().collect(),
    }
}

/// Give each typed argument position its sort; see [`RuleScope::require`].
fn check_positions(scope: &mut RuleScope, atom: &Atom, place: &str) -> Result<(), String> {
    use Sort::*;
    let (name, sorts): (&str, &[Option<Sort>]) = match atom {
        Atom::ClassAtom { .. } | Atom::ClassExpressionAtom { .. } => {
            ("ClassAtom", &[Some(Individual)])
        }
        Atom::DataRangeAtom { .. } => ("DataRangeAtom", &[Some(Data)]),
        Atom::ObjectPropertyAtom { .. } => {
            ("ObjectPropertyAtom", &[Some(Individual), Some(Individual)])
        }
        Atom::DataPropertyAtom { .. } => ("DataPropertyAtom", &[Some(Individual), Some(Data)]),
        // Untyped: the subject is an individual, the value may be either.
        Atom::PropertyAtom { .. } => ("property atom", &[Some(Individual), None]),
        Atom::SameIndividualAtom { .. } => {
            ("SameIndividualAtom", &[Some(Individual), Some(Individual)])
        }
        Atom::DifferentIndividualsAtom { .. } => (
            "DifferentIndividualsAtom",
            &[Some(Individual), Some(Individual)],
        ),
        // Built-in arguments are not given a sort here: built-ins compare
        // individuals as well as data values, and list built-ins take lists.
        Atom::BuiltinAtom { .. } => return Ok(()),
    };
    for (i, (arg, sort)) in atom_args(atom).into_iter().zip(sorts).enumerate() {
        if let Some(sort) = sort {
            scope.require(
                arg,
                *sort,
                &format!("argument {} of {name} in the {place}", i + 1),
            )?;
        }
    }
    Ok(())
}

/// Refuse an unsafe rule (SWRL §2–3): every variable in the head must occur
/// in the body. Without the check a head-only variable made the INSERT skip
/// silently. Whether the body can bind every variable its built-ins use is
/// decided when the steps are ordered ([`builtins::plan`]).
fn check_safety(rule: &SwrlRule) -> Result<(), String> {
    let bound: HashSet<&str> = rule
        .body
        .iter()
        .flat_map(|a| variables(atom_args(a)))
        .collect();
    for atom in &rule.head {
        if let Some(v) = variables(atom_args(atom)).find(|v| !bound.contains(v)) {
            return Err(format!(
                "unsafe rule: head variable '{v}' does not occur in the body \
                 (every head variable must be bound by a body atom)"
            ));
        }
    }
    Ok(())
}

/// Render one class, property or individual atom as a triple pattern.
fn atom_pattern(scope: &mut RuleScope, atom: &Atom) -> Result<String, String> {
    let owl = |local: &str| format!("<http://www.w3.org/2002/07/owl#{local}>");
    let (s, p, o) = match atom {
        Atom::ClassAtom { class_iri, arg } => (
            scope.term(arg)?,
            "a".to_string(),
            predicate_to_sparql(class_iri, "class")?,
        ),
        Atom::ObjectPropertyAtom {
            property,
            arg1,
            arg2,
        }
        | Atom::DataPropertyAtom {
            property,
            arg1,
            arg2,
        }
        | Atom::PropertyAtom {
            property,
            arg1,
            arg2,
        } => (
            scope.term(arg1)?,
            predicate_to_sparql(property, "property")?,
            scope.term(arg2)?,
        ),
        Atom::SameIndividualAtom { arg1, arg2 } => {
            (scope.term(arg1)?, owl("sameAs"), scope.term(arg2)?)
        }
        Atom::DifferentIndividualsAtom { arg1, arg2 } => {
            (scope.term(arg1)?, owl("differentFrom"), scope.term(arg2)?)
        }
        Atom::ClassExpressionAtom { .. } => {
            unreachable!("class expressions are lowered before translation")
        }
        Atom::BuiltinAtom { .. } | Atom::DataRangeAtom { .. } => {
            unreachable!("built-ins and data ranges are not triple patterns")
        }
    };
    Ok(format!("  {s} {p} {o} ."))
}

/// One head atom as a triple template.
fn head_triple(scope: &mut RuleScope, atom: &Atom) -> Result<HeadTriple, String> {
    let owl =
        |local: &str| NamedNode::new_unchecked(format!("http://www.w3.org/2002/07/owl#{local}"));
    Ok(match atom {
        Atom::ClassAtom { class_iri, arg } => HeadTriple {
            subject: scope.step_arg(arg)?,
            predicate: NamedNode::new_unchecked("http://www.w3.org/1999/02/22-rdf-syntax-ns#type"),
            object: StepArg::Const(validate_predicate_iri(class_iri, "class")?.into()),
            object_kind: ObjectKind::Individual,
        },
        Atom::ObjectPropertyAtom {
            property,
            arg1,
            arg2,
        }
        | Atom::DataPropertyAtom {
            property,
            arg1,
            arg2,
        }
        | Atom::PropertyAtom {
            property,
            arg1,
            arg2,
        } => HeadTriple {
            subject: scope.step_arg(arg1)?,
            predicate: validate_predicate_iri(property, "property")?,
            object: scope.step_arg(arg2)?,
            object_kind: match atom {
                Atom::ObjectPropertyAtom { .. } => ObjectKind::Individual,
                Atom::DataPropertyAtom { .. } => ObjectKind::Data,
                _ => ObjectKind::Either,
            },
        },
        Atom::SameIndividualAtom { arg1, arg2 } | Atom::DifferentIndividualsAtom { arg1, arg2 } => {
            HeadTriple {
                subject: scope.step_arg(arg1)?,
                predicate: owl(if matches!(atom, Atom::SameIndividualAtom { .. }) {
                    "sameAs"
                } else {
                    "differentFrom"
                }),
                object: scope.step_arg(arg2)?,
                object_kind: ObjectKind::Individual,
            }
        }
        Atom::ClassExpressionAtom { .. } => {
            unreachable!("class expressions are lowered before translation")
        }
        Atom::BuiltinAtom { .. } | Atom::DataRangeAtom { .. } => {
            unreachable!("refused in the head before translation")
        }
    })
}

/// Translate a SWRL rule (class expressions already lowered) to the SPARQL
/// it runs and its plan.
fn compile_rule(
    rule: &SwrlRule,
    target_graph: Option<&NamedNode>,
) -> Result<(String, Plan), String> {
    if rule.head.is_empty() {
        return Err("Rule has no head atoms".to_string());
    }
    if let Some(Atom::BuiltinAtom { builtin, .. }) = rule
        .head
        .iter()
        .find(|a| matches!(a, Atom::BuiltinAtom { .. }))
    {
        return Err(format!(
            "built-in '{builtin}' in the rule head: SWRL allows built-ins only in the body"
        ));
    }
    if let Some(Atom::DataRangeAtom { range, .. }) = rule
        .head
        .iter()
        .find(|a| matches!(a, Atom::DataRangeAtom { .. }))
    {
        return Err(format!(
            "the data range {range} in the rule head: a data range can be tested in the body, \
             but there is no triple that asserts it"
        ));
    }
    check_safety(rule)?;

    let mut scope = RuleScope::default();
    for atom in &rule.body {
        check_positions(&mut scope, atom, "body")?;
    }
    for atom in &rule.head {
        check_positions(&mut scope, atom, "head")?;
    }

    let mut where_patterns = Vec::new();
    let mut filters = Vec::new();
    // Variables in a body object position could bind to either kind of term;
    // a typed variable there gets a filter so it binds only to its own kind.
    let mut object_vars: Vec<&str> = Vec::new();
    let mut pattern_vars: Vec<String> = Vec::new();
    let mut steps: Vec<Step> = Vec::new();

    for atom in &rule.body {
        match atom {
            Atom::BuiltinAtom { builtin, args } => {
                // A built-in the engine cannot evaluate is a hard error, not
                // a skip: dropping its condition would assert the head for
                // every binding the rest of the body allows.
                let local = builtins::resolve(builtin, args.len())?;
                let step_args = args
                    .iter()
                    .map(|a| scope.step_arg(a))
                    .collect::<Result<Vec<_>, _>>()?;
                let consts: Vec<Option<Term>> = step_args
                    .iter()
                    .map(|a| match a {
                        StepArg::Const(t) => Some(t.clone()),
                        StepArg::Var(_) => None,
                    })
                    .collect();
                builtins::check_constants(&local, &consts)?;
                let label = format!(
                    "swrlb:{local}({})",
                    step_args
                        .iter()
                        .map(step_arg_text)
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                steps.push(Step {
                    kind: StepKind::Builtin(local),
                    args: step_args,
                    label,
                });
            }
            Atom::DataRangeAtom { range, arg } => {
                range.validate()?;
                datarange::check_range(range)?;
                let a = scope.step_arg(arg)?;
                let label = format!("DataRangeAtom({range} {})", step_arg_text(&a));
                steps.push(Step {
                    kind: StepKind::Range(range.clone()),
                    args: vec![a],
                    label,
                });
            }
            other => {
                where_patterns.push(atom_pattern(&mut scope, other)?);
                for v in variables(atom_args(other)) {
                    let g = scope.var(v)?;
                    if !pattern_vars.contains(&g) {
                        pattern_vars.push(g);
                    }
                }
                if let [_, SwrlArg::Variable(v)] = atom_args(other).as_slice() {
                    object_vars.push(v.as_str());
                }
            }
        }
    }
    let mut typed: Vec<(String, Sort)> = Vec::new();
    for v in object_vars {
        if let Some(sort) = scope.sorts.get(v).copied() {
            let var = scope.var(v)?;
            if !typed.iter().any(|(t, _)| *t == var) {
                typed.push((var, sort));
            }
        }
    }
    for (var, sort) in typed {
        filters.push(match sort {
            Sort::Individual => format!("!isLiteral({var})"),
            Sort::Data => format!("isLiteral({var})"),
        });
    }

    let mut where_clause = where_patterns.join("\n");
    if !filters.is_empty() {
        where_clause.push('\n');
        for f in &filters {
            where_clause.push_str(&format!("  FILTER({}) .\n", f));
        }
    }

    if steps.is_empty() {
        let insert_patterns = rule
            .head
            .iter()
            .map(|atom| atom_pattern(&mut scope, atom))
            .collect::<Result<Vec<_>, _>>()?;
        let graph_clause = if let Some(g) = target_graph {
            format!("GRAPH {g} {{\n{}\n  }}", insert_patterns.join("\n"))
        } else {
            insert_patterns.join("\n")
        };
        let sparql = format!(
            "INSERT {{\n{}\n}} WHERE {{\n{}\n}}",
            graph_clause, where_clause
        );
        return Ok((sparql.clone(), Plan::Insert(sparql)));
    }

    let mut bound: HashSet<String> = pattern_vars
        .iter()
        .map(|v| v.trim_start_matches('?').to_string())
        .collect();
    let steps = builtins::plan(steps, &mut bound)?;
    let head = rule
        .head
        .iter()
        .map(|atom| head_triple(&mut scope, atom))
        .collect::<Result<Vec<_>, _>>()?;
    let projection = if pattern_vars.is_empty() {
        "*".to_string()
    } else {
        pattern_vars.join(" ")
    };
    let select = format!("SELECT DISTINCT {projection} WHERE {{\n{where_clause}\n}}");
    let mut shown = select.clone();
    for s in &steps {
        shown.push_str(&format!("\n# then {}", s.label));
    }
    Ok((
        shown,
        Plan::Native(NativeRule {
            select,
            steps,
            head,
        }),
    ))
}

fn step_arg_text(a: &StepArg) -> String {
    match a {
        StepArg::Var(v) => format!("?{v}"),
        StepArg::Const(t) => t.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SWRLB: &str = "http://www.w3.org/2003/11/swrlb#";

    fn rule_to_sparql(rule: &SwrlRule, target: Option<&NamedNode>) -> Result<String, String> {
        compile_rule(rule, target).map(|(sparql, _)| sparql)
    }

    #[test]
    fn test_rule_to_sparql() {
        let rule = SwrlRule {
            name: Some("test".to_string()),
            body: vec![
                Atom::ClassAtom {
                    class_iri: "http://example.org/Person".to_string(),
                    arg: SwrlArg::Variable("?x".to_string()),
                },
                Atom::ObjectPropertyAtom {
                    property: "http://example.org/knows".to_string(),
                    arg1: SwrlArg::Variable("?x".to_string()),
                    arg2: SwrlArg::Variable("?y".to_string()),
                },
            ],
            head: vec![Atom::ClassAtom {
                class_iri: "http://example.org/Person".to_string(),
                arg: SwrlArg::Variable("?y".to_string()),
            }],
        };

        let sparql = rule_to_sparql(&rule, None).unwrap();
        assert!(sparql.contains("INSERT"));
        assert!(sparql.contains("WHERE"));
        assert!(sparql.contains("http://example.org/Person"));
        assert!(sparql.contains("http://example.org/knows"));
    }

    /// A rule with a built-in runs natively: a SELECT over its other body
    /// atoms, then the built-in, which may bind a variable the head uses.
    #[test]
    fn builtins_compile_to_a_native_plan() {
        let rule = SwrlRule {
            name: None,
            body: vec![
                Atom::DataPropertyAtom {
                    property: "http://ex/age".to_string(),
                    arg1: SwrlArg::Variable("?x".to_string()),
                    arg2: SwrlArg::Variable("?a".to_string()),
                },
                Atom::BuiltinAtom {
                    builtin: format!("{SWRLB}add"),
                    args: vec![
                        SwrlArg::Variable("?n".to_string()),
                        SwrlArg::Variable("?a".to_string()),
                        SwrlArg::Literal {
                            value: "1".to_string(),
                            datatype: Some("http://www.w3.org/2001/XMLSchema#integer".to_string()),
                            language: None,
                        },
                    ],
                },
            ],
            head: vec![Atom::DataPropertyAtom {
                property: "http://ex/nextAge".to_string(),
                arg1: SwrlArg::Variable("?x".to_string()),
                arg2: SwrlArg::Variable("?n".to_string()),
            }],
        };
        let (sparql, plan) = compile_rule(&rule, None).unwrap();
        assert!(
            sparql.starts_with("SELECT DISTINCT ?v0_x ?v1_a"),
            "{sparql}"
        );
        assert!(sparql.contains("# then swrlb:add(?v2_n, ?v1_a"), "{sparql}");
        assert!(matches!(plan, Plan::Native(_)));
    }

    #[test]
    fn test_swrl_arg_to_sparql() {
        let mut scope = RuleScope::default();
        let x = scope.term(&SwrlArg::Variable("?x".to_string())).unwrap();
        assert_eq!(x, "?v0_x");
        assert_eq!(
            scope
                .term(&SwrlArg::Variable("urn:swrl:var#foo".to_string()))
                .unwrap(),
            "?v1_foo"
        );
        // Two IRIs with the same local name stay two variables.
        assert_eq!(
            scope
                .term(&SwrlArg::Variable("http://ex/onto#foo".to_string()))
                .unwrap(),
            "?v2_foo"
        );
        // Characters SPARQL variables cannot hold are dropped from the hint.
        assert_eq!(
            scope
                .term(&SwrlArg::Variable("urn:x#a b>}".to_string()))
                .unwrap(),
            "?v3_ab"
        );
        assert_eq!(
            scope.term(&SwrlArg::Variable("?x".to_string())).unwrap(),
            x,
            "the same name maps to the same variable"
        );
        assert!(scope.term(&SwrlArg::Variable("?".to_string())).is_err());
        assert_eq!(
            scope
                .term(&SwrlArg::Individual("http://example.org/Alice".to_string()))
                .unwrap(),
            "<http://example.org/Alice>"
        );
        assert_eq!(
            scope
                .term(&SwrlArg::Literal {
                    value: "hallo".to_string(),
                    datatype: None,
                    language: Some("nl".to_string()),
                })
                .unwrap(),
            "\"hallo\"@nl"
        );
    }

    /// A literal argument must be escaped, not pasted in. Rendering it as
    /// `format!("\"{value}\"")` let a quote close the string and the rest of the
    /// value become SPARQL syntax in the generated INSERT/WHERE.
    #[test]
    fn literal_arg_cannot_break_out_of_its_quotes() {
        let evil = SwrlArg::Literal {
            value: "\" } ; DROP ALL ; INSERT DATA { <urn:x> <urn:y> \"pwned".to_string(),
            datatype: None,
            language: None,
        };
        let rendered = constant_to_sparql(&evil).unwrap();
        // The payload text is still present — it is data. What matters is that
        // the quote inside it is escaped, so it cannot terminate the literal.
        assert!(
            rendered.contains("\\\""),
            "inner quote must be escaped: {rendered}"
        );

        // The property that actually matters: the generated update still parses
        // as exactly ONE operation, so the payload never becomes executable.
        let rule = SwrlRule {
            name: Some("injected".to_string()),
            body: vec![Atom::DataPropertyAtom {
                property: "http://example.org/name".to_string(),
                arg1: SwrlArg::Variable("?x".to_string()),
                arg2: evil,
            }],
            head: vec![Atom::ClassAtom {
                class_iri: "http://example.org/Tagged".to_string(),
                arg: SwrlArg::Variable("?x".to_string()),
            }],
        };
        let sparql = rule_to_sparql(&rule, None).unwrap();
        let parsed = spargebra::SparqlParser::new()
            .parse_update(&sparql)
            .expect("generated update must still parse");
        assert_eq!(
            parsed.operations.len(),
            1,
            "the literal must not split the update into several operations: {sparql}"
        );
        assert!(
            !matches!(
                parsed.operations[0],
                spargebra::GraphUpdateOperation::Drop { .. }
            ),
            "the injected DROP must not become an operation: {sparql}"
        );
    }

    /// An IRI argument that is not a valid IRI is rejected outright rather than
    /// interpolated between angle brackets.
    #[test]
    fn individual_arg_with_invalid_iri_is_rejected() {
        let evil = SwrlArg::Individual("http://ex/a> <urn:p> <urn:o> . <urn:s".to_string());
        assert!(
            constant_to_sparql(&evil).is_err(),
            "an unrepresentable IRI must be an error, not spliced text"
        );
    }

    /// Class and property predicates take the same road as arguments: a value
    /// that is not an IRI is refused, never interpolated between `<…>`. The
    /// text `<http://ex/A> } ; INSERT DATA …` used to land verbatim in the
    /// WHERE clause and turn one rule into several update operations.
    #[test]
    fn class_and_property_predicates_that_are_not_iris_are_rejected() {
        let payload = "http://ex/Person> } ; INSERT DATA { GRAPH <urn:probe> { <urn:s> <urn:p> <urn:o> } } ; INSERT { } WHERE { ?z a <http://ex/Q";
        let by_class = SwrlRule {
            name: None,
            body: vec![Atom::ClassAtom {
                class_iri: payload.to_string(),
                arg: SwrlArg::Variable("?x".to_string()),
            }],
            head: vec![Atom::ClassAtom {
                class_iri: "http://ex/T".to_string(),
                arg: SwrlArg::Variable("?x".to_string()),
            }],
        };
        let err = rule_to_sparql(&by_class, None).expect_err("class predicate");
        assert!(err.contains("class IRI"), "{err}");

        let by_property = SwrlRule {
            name: None,
            body: vec![Atom::ClassAtom {
                class_iri: "http://ex/Person".to_string(),
                arg: SwrlArg::Variable("?x".to_string()),
            }],
            head: vec![Atom::ObjectPropertyAtom {
                property: payload.to_string(),
                arg1: SwrlArg::Variable("?x".to_string()),
                arg2: SwrlArg::Variable("?x".to_string()),
            }],
        };
        let err = rule_to_sparql(&by_property, None).expect_err("property predicate");
        assert!(err.contains("property IRI"), "{err}");

        // A relative name is not an IRI either.
        assert!(validate_predicate_iri("Person", "class").is_err());
        // Angle brackets around a proper IRI are tolerated.
        assert_eq!(
            predicate_to_sparql("<http://ex/Person>", "class").unwrap(),
            "<http://ex/Person>"
        );
    }

    /// A rule whose body uses a builtin the engine cannot translate must be
    /// refused. It used to drop the FILTER and run the rest, so the head was
    /// asserted for every binding — silently unsound inference.
    #[test]
    fn unsupported_builtin_fails_the_rule_instead_of_dropping_its_guard() {
        let rule = SwrlRule {
            name: Some("guarded".to_string()),
            body: vec![
                Atom::ClassAtom {
                    class_iri: "http://example.org/Person".to_string(),
                    arg: SwrlArg::Variable("?x".to_string()),
                },
                Atom::DataPropertyAtom {
                    property: "http://example.org/name".to_string(),
                    arg1: SwrlArg::Variable("?x".to_string()),
                    arg2: SwrlArg::Variable("?n".to_string()),
                },
                Atom::DataPropertyAtom {
                    property: "http://example.org/nameLength".to_string(),
                    arg1: SwrlArg::Variable("?x".to_string()),
                    arg2: SwrlArg::Variable("?len".to_string()),
                },
                Atom::BuiltinAtom {
                    builtin: "http://example.org/fn#stringLength".to_string(),
                    args: vec![
                        SwrlArg::Variable("?n".to_string()),
                        SwrlArg::Variable("?len".to_string()),
                    ],
                },
            ],
            head: vec![Atom::ClassAtom {
                class_iri: "http://example.org/LongName".to_string(),
                arg: SwrlArg::Variable("?x".to_string()),
            }],
        };

        let err = rule_to_sparql(&rule, None)
            .expect_err("a rule with an untranslatable builtin must not produce SPARQL");
        assert!(
            err.contains("Unsupported SWRL builtin") && err.contains("stringLength"),
            "the error must name the offending builtin: {err}"
        );
    }

    fn var(name: &str) -> SwrlArg {
        SwrlArg::Variable(name.to_string())
    }

    fn class(iri: &str, arg: SwrlArg) -> Atom {
        Atom::ClassAtom {
            class_iri: iri.to_string(),
            arg,
        }
    }

    /// Safety (SWRL §2–3): a head variable the body does not bind, and a
    /// built-in variable only a built-in mentions, are refused by name.
    #[test]
    fn unsafe_rules_are_refused() {
        let head_only = SwrlRule {
            name: None,
            body: vec![class("http://ex/A", var("?x"))],
            head: vec![Atom::ObjectPropertyAtom {
                property: "http://ex/p".to_string(),
                arg1: var("?x"),
                arg2: var("?y"),
            }],
        };
        let err = rule_to_sparql(&head_only, None).unwrap_err();
        assert!(err.contains("unsafe rule") && err.contains("'?y'"), "{err}");

        let builtin_only = SwrlRule {
            name: None,
            body: vec![
                class("http://ex/A", var("?x")),
                Atom::BuiltinAtom {
                    builtin: format!("{SWRLB}add"),
                    args: vec![var("?z"), var("?w"), var("?x")],
                },
            ],
            head: vec![class("http://ex/B", var("?x"))],
        };
        let err = rule_to_sparql(&builtin_only, None).unwrap_err();
        assert!(
            err.contains("infinitely many") && err.contains("swrlb:add"),
            "{err}"
        );

        // add(?z, ?x, ?x) binds ?z from ?x: safe now that built-ins bind.
        let binds = SwrlRule {
            name: None,
            body: vec![
                class("http://ex/A", var("?x")),
                Atom::BuiltinAtom {
                    builtin: format!("{SWRLB}add"),
                    args: vec![var("?z"), var("?x"), var("?x")],
                },
            ],
            head: vec![class("http://ex/B", var("?x"))],
        };
        assert!(rule_to_sparql(&binds, None).is_ok());
    }

    /// Typed positions: constants of the wrong kind and variables used as both
    /// kinds are refused; a typed variable in an object position is filtered.
    #[test]
    fn argument_positions_are_typed() {
        let literal = SwrlArg::Literal {
            value: "v".to_string(),
            datatype: None,
            language: None,
        };
        let bad_object = SwrlRule {
            name: None,
            body: vec![Atom::ObjectPropertyAtom {
                property: "http://ex/p".to_string(),
                arg1: var("?x"),
                arg2: literal,
            }],
            head: vec![class("http://ex/B", var("?x"))],
        };
        let err = rule_to_sparql(&bad_object, None).unwrap_err();
        assert!(err.contains("takes an individual"), "{err}");

        let both = SwrlRule {
            name: None,
            body: vec![Atom::DataPropertyAtom {
                property: "http://ex/d".to_string(),
                arg1: var("?x"),
                arg2: var("?v"),
            }],
            head: vec![class("http://ex/B", var("?v"))],
        };
        let err = rule_to_sparql(&both, None).unwrap_err();
        assert!(
            err.contains("'?v' is used as a data value and as an individual"),
            "{err}"
        );

        let object = SwrlRule {
            name: None,
            body: vec![Atom::ObjectPropertyAtom {
                property: "http://ex/p".to_string(),
                arg1: var("?x"),
                arg2: var("?y"),
            }],
            head: vec![class("http://ex/B", var("?y"))],
        };
        let sparql = rule_to_sparql(&object, None).unwrap();
        assert!(sparql.contains("FILTER(!isLiteral(?v1_y))"), "{sparql}");
    }
}
