//! SWRL rule evaluation engine.
//!
//! Translates SWRL rules to SPARQL INSERT WHERE queries and executes them
//! in a fixed-point loop until no new triples are inferred.
//!
//! Translation ([`compile_rules`]) refuses, with a message, every rule it
//! cannot run as written:
//! - an unsafe rule: a variable in the head, or in a built-in, that no body
//!   atom binds (SWRL §2–3 safety; built-ins cannot bind variables yet);
//! - a built-in in the head;
//! - a built-in it cannot translate;
//! - a constant in the wrong kind of position (a literal where an individual
//!   belongs, or the other way round), and a variable used as both.
//!
//! Variables may be named by any IRI (`urn:swrl:var#x`, the OWL API's
//! `urn:swrl#x`, an ontology-namespace IRI) or by `?x` in the text form; each
//! is mapped to a generated SPARQL variable, so the name never reaches the
//! query text.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use oxigraph::model::{Literal, NamedNode};
use serde::Serialize;
use tracing::{debug, info, warn};

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
    /// Built-in predicate (math, string, comparison)
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

/// The SWRL built-in namespace. Only built-ins in it are translated: a
/// same-named function in another namespace means something else.
const SWRLB: &str = "http://www.w3.org/2003/11/swrlb#";

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
}

/// Render a constant argument (individual or literal) as a SPARQL term.
fn constant_to_sparql(arg: &SwrlArg) -> Result<String, String> {
    match arg {
        SwrlArg::Variable(v) => Err(format!("'{v}' is a variable, not a constant")),
        SwrlArg::Individual(iri) => {
            let trimmed = iri.trim_start_matches('<').trim_end_matches('>');
            let node = NamedNode::new(trimmed)
                .map_err(|e| format!("Invalid SWRL individual IRI '{trimmed}': {e}"))?;
            Ok(node.to_string())
        }
        SwrlArg::Literal {
            value,
            language: Some(lang),
            ..
        } => Literal::new_language_tagged_literal(value.as_str(), lang.as_str())
            .map(|l| l.to_string())
            .map_err(|e| format!("Invalid SWRL literal language tag '{lang}': {e}")),
        SwrlArg::Literal {
            value,
            datatype: Some(dt),
            ..
        } => {
            let dt_node = NamedNode::new(dt.as_str())
                .map_err(|e| format!("Invalid SWRL literal datatype '{dt}': {e}"))?;
            Ok(Literal::new_typed_literal(value.as_str(), dt_node).to_string())
        }
        SwrlArg::Literal { value, .. } => {
            Ok(Literal::new_simple_literal(value.as_str()).to_string())
        }
    }
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
    pub sparql: String,
    pub success: bool,
    pub error: Option<String>,
}

/// Rules translated to SPARQL, ready to run.
#[derive(Debug, Clone)]
pub struct CompiledRules {
    rules: Vec<(String, String)>,
    target: Option<NamedNode>,
}

/// Translate every rule, refusing the whole set if any rule cannot run as
/// written. Running the rest would hand back a closure the caller did not
/// ask for, so the error names each refused rule and nothing runs.
pub fn compile_rules(
    rules: &[SwrlRule],
    target_graph: Option<&str>,
) -> Result<CompiledRules, String> {
    let target = target_graph.map(validate_target_graph).transpose()?;
    let mut compiled = Vec::with_capacity(rules.len());
    let mut errors = Vec::new();
    for (i, rule) in rules.iter().enumerate() {
        let name = rule
            .name
            .clone()
            .unwrap_or_else(|| format!("rule_{}", i + 1));
        match rule_to_sparql(rule, target.as_ref()) {
            Ok(sparql) => compiled.push((name, sparql)),
            Err(e) => errors.push(format!("rule '{name}': {e}")),
        }
    }
    if !errors.is_empty() {
        return Err(format!("SWRL rules refused: {}", errors.join("; ")));
    }
    Ok(CompiledRules {
        rules: compiled,
        target,
    })
}

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

    let start_count = count()?;
    let mut rule_results: Vec<RuleResult> = compiled
        .rules
        .iter()
        .map(|(name, sparql)| RuleResult {
            rule_name: name.clone(),
            sparql: sparql.clone(),
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
        for ((name, sparql), result) in compiled.rules.iter().zip(rule_results.iter_mut()) {
            if expired() {
                stop_reason = StopReason::Timeout;
                break 'fixpoint;
            }
            // Through `TripleStore::update`, not a raw `parse_update(..).execute()`
            // on the inner store: the store's write guard (mirror, query cache)
            // and per-graph count maintenance must see these writes like any
            // other update.
            if let Err(e) = store.update(sparql) {
                warn!("Rule {} failed: {}", name, e);
                if result.success {
                    result.success = false;
                    result.error = Some(e.to_string());
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
        Atom::ClassAtom { arg, .. } => vec![arg],
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
        Atom::ClassAtom { .. } => ("ClassAtom", &[Some(Individual)]),
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
        // Built-in arguments are not given a sort here: built-ins in use
        // compare individuals as well as data values.
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

/// Refuse an unsafe rule: every variable in the head or in a body built-in
/// must occur in a body atom that binds it (any atom but a built-in, since no
/// built-in binds a variable yet). Without the check, a head-only variable made
/// the INSERT skip silently, and a built-in-only one made its FILTER fail on
/// every binding.
fn check_safety(rule: &SwrlRule) -> Result<(), String> {
    let bound: HashSet<&str> = rule
        .body
        .iter()
        .filter(|a| !matches!(a, Atom::BuiltinAtom { .. }))
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
    for atom in &rule.body {
        if let Atom::BuiltinAtom { builtin, args } = atom {
            if let Some(v) = variables(args).find(|v| !bound.contains(v)) {
                return Err(format!(
                    "unsafe rule: variable '{v}' of built-in '{builtin}' is bound by no \
                     class, property or individual atom in the body (built-ins cannot \
                     bind variables yet)"
                ));
            }
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
        Atom::BuiltinAtom { .. } => unreachable!("built-ins are not triple patterns"),
    };
    Ok(format!("  {s} {p} {o} ."))
}

/// Translate a SWRL rule to a SPARQL INSERT WHERE query.
fn rule_to_sparql(rule: &SwrlRule, target_graph: Option<&NamedNode>) -> Result<String, String> {
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

    for atom in &rule.body {
        match atom {
            Atom::BuiltinAtom { builtin, args } => {
                // A builtin we cannot translate is a hard error, not a skip.
                // Dropping the FILTER left the rest of the rule intact and
                // firing — so `Person(?x) ^ stringLength(?n, ?len) ^
                // greaterThan(?len, 5) -> LongName(?x)` lost its guard entirely
                // and asserted the head for every binding. Silently unsound
                // inference is worse than a refused rule.
                let rendered = args
                    .iter()
                    .map(|a| scope.term(a))
                    .collect::<Result<Vec<_>, _>>()?;
                match builtin_to_filter(builtin, &rendered) {
                    Some(filter) => filters.push(filter),
                    None => {
                        return Err(format!(
                            "Unsupported SWRL builtin '{builtin}' with {} argument(s): refusing \
                             to run the rule, because dropping its condition would assert the \
                             head unconditionally",
                            args.len()
                        ))
                    }
                }
            }
            other => {
                where_patterns.push(atom_pattern(&mut scope, other)?);
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

    let mut where_clause = where_patterns.join("\n");
    if !filters.is_empty() {
        where_clause.push('\n');
        for f in &filters {
            where_clause.push_str(&format!("  FILTER({}) .\n", f));
        }
    }

    Ok(format!(
        "INSERT {{\n{}\n}} WHERE {{\n{}\n}}",
        graph_clause, where_clause
    ))
}

/// Translate a SWRL built-in predicate, its arguments already rendered as
/// SPARQL terms, to a SPARQL FILTER expression. `None` means the built-in is
/// not one of the supported ones (or not with this many arguments); the
/// caller must treat that as a rule-level failure — see the `BuiltinAtom` arm
/// of [`rule_to_sparql`].
fn builtin_to_filter(builtin: &str, a: &[String]) -> Option<String> {
    let Some(local) = builtin.strip_prefix(SWRLB) else {
        debug!("SWRL builtin outside the swrlb namespace: {}", builtin);
        return None;
    };
    let binary = |op: &str| format!("{} {op} {}", a[0], a[1]);
    let arithmetic = |op: &str| format!("{} = {} {op} {}", a[0], a[1], a[2]);
    Some(match (local, a.len()) {
        ("equal", 2) => binary("="),
        ("notEqual", 2) => binary("!="),
        ("lessThan", 2) => binary("<"),
        ("lessThanOrEqual", 2) => binary("<="),
        ("greaterThan", 2) => binary(">"),
        ("greaterThanOrEqual", 2) => binary(">="),
        ("add", 3) => arithmetic("+"),
        ("subtract", 3) => arithmetic("-"),
        ("multiply", 3) => arithmetic("*"),
        ("divide", 3) => arithmetic("/"),
        // stringConcat(?r, ?a, ?b, …) holds when ?r is the concatenation of
        // the rest. A bare `CONCAT(…)` as the FILTER was true for every
        // non-empty result, so the condition never excluded anything.
        ("stringConcat", n) if n >= 2 => format!("{} = CONCAT({})", a[0], a[1..].join(", ")),
        ("contains", 2) => format!("CONTAINS({}, {})", a[0], a[1]),
        // matches(?s, pattern) or matches(?s, pattern, flags); any other count
        // is refused rather than truncated.
        ("matches", 2 | 3) => format!("REGEX({})", a.join(", ")),
        _ => {
            debug!("Unsupported SWRL builtin: {}", builtin);
            return None;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn test_builtin_to_filter() {
        let args = vec!["?x".to_string(), "?y".to_string()];
        assert_eq!(
            builtin_to_filter("http://www.w3.org/2003/11/swrlb#greaterThan", &args),
            Some("?x > ?y".to_string())
        );
        // A same-named function outside swrlb: is not swrlb:greaterThan.
        assert_eq!(builtin_to_filter("http://ex/fn#greaterThan", &args), None);
        // stringConcat is an equation on its first argument, not a bare CONCAT.
        let args = vec!["?r".to_string(), "?a".to_string(), "?b".to_string()];
        assert_eq!(
            builtin_to_filter("http://www.w3.org/2003/11/swrlb#stringConcat", &args),
            Some("?r = CONCAT(?a, ?b)".to_string())
        );
        // matches takes two or three arguments; a fourth is refused, not cut off.
        let four: Vec<String> = ["?s", "\"a\"", "\"i\"", "\"x\""]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            builtin_to_filter("http://www.w3.org/2003/11/swrlb#matches", &four),
            None
        );
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
                    builtin: "http://www.w3.org/2003/11/swrlb#stringLength".to_string(),
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
                    args: vec![var("?z"), var("?x"), var("?x")],
                },
            ],
            head: vec![class("http://ex/B", var("?x"))],
        };
        let err = rule_to_sparql(&builtin_only, None).unwrap_err();
        assert!(err.contains("unsafe rule") && err.contains("'?z'"), "{err}");
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
