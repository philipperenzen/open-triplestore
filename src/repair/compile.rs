//! Rules compiled from a dataset's SHACL Core shapes and OWL axioms
//! (`docs/notes/repair-layer-design.md` §3.6, §3.7), and imported from its
//! SHACL-AF `sh:rule`s.
//!
//! The compiler follows the validator, not the specification's every
//! reading of it: the shapes come from the engine's own loader
//! (`shacl::engine::load_shapes`, implicit class targets and all), class
//! targets are instances typed in one graph with the `rdfs:subClassOf*`
//! chain read across every graph the validation reads (`DataView::prepare`),
//! a single-predicate value is looked up in every graph (the engine unions
//! single hops across graphs, so a value linked in another graph satisfies
//! it), and `sh:class` accepts a type asserted in any graph. Each rule's
//! guard is the constraint's own check, so a repaired focus node passes the
//! validator and an already-valid one never fires.
//!
//! Only what the shape determines becomes a `Repair` rule: a value
//! (`sh:hasValue`, a single-member `sh:in`), a type (`sh:class`), an
//! existential witness (`sh:minCount` whose siblings an IRI can satisfy).
//! Choices — which value to drop, which member of `sh:in` — are made only by
//! an opt-in policy with a declared total order (`closed-delete`,
//! `maxCount-keep-lexmin`, `datatype-relabel`); everything else is listed as
//! report-only with the reason, so the residue is visible rather than
//! guessed at.

use std::collections::{BTreeSet, HashSet};

use oxigraph::model::{NamedNode, Term};
use oxigraph::sparql::QueryResults;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::rules::{
    Confidence, GraphScope, MergeMode, Native, Origin, Policy, RuleSpec, ViolationRef, RDF_TYPE,
};
use crate::shacl::shapes::{Constraint, NodeKind, PropertyPath, PropertyShape, Shape, Target};
use crate::store::TripleStore;

const SH: &str = "http://www.w3.org/ns/shacl#";

/// The constraint component each `sh:` keyword the engine writes at the head
/// of `ValidationResult::source_constraint` belongs to (SHACL §4, SHACL-AF).
const COMPONENTS: &[(&str, &str)] = &[
    ("sh:class", "ClassConstraintComponent"),
    ("sh:datatype", "DatatypeConstraintComponent"),
    ("sh:nodeKind", "NodeKindConstraintComponent"),
    ("sh:minCount", "MinCountConstraintComponent"),
    ("sh:maxCount", "MaxCountConstraintComponent"),
    ("sh:minExclusive", "MinExclusiveConstraintComponent"),
    ("sh:minInclusive", "MinInclusiveConstraintComponent"),
    ("sh:maxExclusive", "MaxExclusiveConstraintComponent"),
    ("sh:maxInclusive", "MaxInclusiveConstraintComponent"),
    ("sh:minLength", "MinLengthConstraintComponent"),
    ("sh:maxLength", "MaxLengthConstraintComponent"),
    ("sh:pattern", "PatternConstraintComponent"),
    ("sh:languageIn", "LanguageInConstraintComponent"),
    ("sh:uniqueLang", "UniqueLangConstraintComponent"),
    ("sh:equals", "EqualsConstraintComponent"),
    ("sh:disjoint", "DisjointConstraintComponent"),
    ("sh:lessThan", "LessThanConstraintComponent"),
    ("sh:lessThanOrEquals", "LessThanOrEqualsConstraintComponent"),
    ("sh:not", "NotConstraintComponent"),
    ("sh:and", "AndConstraintComponent"),
    ("sh:or", "OrConstraintComponent"),
    ("sh:xone", "XoneConstraintComponent"),
    ("sh:node", "NodeConstraintComponent"),
    ("sh:property", "PropertyConstraintComponent"),
    (
        "sh:qualifiedMinCount",
        "QualifiedMinCountConstraintComponent",
    ),
    (
        "sh:qualifiedMaxCount",
        "QualifiedMaxCountConstraintComponent",
    ),
    ("sh:closed", "ClosedConstraintComponent"),
    ("sh:hasValue", "HasValueConstraintComponent"),
    ("sh:in", "InConstraintComponent"),
    ("sh:SPARQLConstraint", "SPARQLConstraintComponent"),
    ("sh:sparql", "SPARQLConstraintComponent"),
    ("sh:expression", "ExpressionConstraintComponent"),
];

/// The constraint component IRI a `source_constraint` text names: the engine
/// writes the constraint's keyword first (`sh:minCount 1`, `sh:class <…>`),
/// a SHACL-AF component's own IRI, and a stored report may carry the
/// component's name (`sh:MaxLengthConstraintComponent`). `None` for a result
/// no component produced (a gate that could not be evaluated).
pub fn constraint_component(source_constraint: &str) -> Option<String> {
    let head = source_constraint.split_whitespace().next()?;
    if let Some((_, local)) = COMPONENTS.iter().find(|(k, _)| *k == head) {
        return Some(format!("{SH}{local}"));
    }
    if let Some(local) = head.strip_prefix("sh:") {
        return local.ends_with("Component").then(|| format!("{SH}{local}"));
    }
    let iri = head.trim_start_matches('<').trim_end_matches('>');
    (iri.contains("://") || iri.starts_with("urn:")).then(|| iri.to_string())
}

const RDFS_SUBCLASS: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const OWL: &str = "http://www.w3.org/2002/07/owl#";
/// How deep `sh:node` / `sh:and` / nested `sh:property` are followed.
const MAX_DEPTH: usize = 3;

/// The opt-in policies of a run (§3.6).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Policies {
    pub closed_delete: bool,
    pub max_count_keep_lexmin: bool,
    pub datatype_relabel: bool,
}

impl Policies {
    pub const NAMES: [&'static str; 3] =
        ["closed-delete", "maxCount-keep-lexmin", "datatype-relabel"];

    /// Parse the request's list; `Err` names an unknown policy.
    pub fn parse(names: &[String]) -> Result<Self, String> {
        let mut p = Policies::default();
        for n in names {
            match n.trim() {
                "closed-delete" => p.closed_delete = true,
                "maxCount-keep-lexmin" => p.max_count_keep_lexmin = true,
                "datatype-relabel" => p.datatype_relabel = true,
                other => {
                    return Err(format!(
                        "unknown policy `{other}` (known: {})",
                        Self::NAMES.join(", ")
                    ))
                }
            }
        }
        Ok(p)
    }
}

/// A constraint the compiler does not turn into a repair, and why.
#[derive(Debug, Clone, Serialize)]
pub struct ReportOnly {
    pub rule: String,
    pub source_shape: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub source_constraint_component: String,
    pub reason: String,
    /// Validation results it accounts for, when the run validated (`null`
    /// otherwise: counting them needs the validator).
    pub triggers: Option<usize>,
}

/// What a compilation produced.
#[derive(Debug, Default)]
pub struct Compiled {
    pub rules: Vec<RuleSpec>,
    pub report_only: Vec<ReportOnly>,
    /// Targets and shapes that could not be compiled at all, with the reason.
    pub skipped: Vec<String>,
}

impl Compiled {
    pub fn extend(&mut self, other: Compiled) {
        self.rules.extend(other.rules);
        self.report_only.extend(other.report_only);
        self.skipped.extend(other.skipped);
    }
}

fn short_hash(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update(b"\x1f");
    }
    hex::encode(h.finalize())[..8].to_string()
}

/// The last segment of an IRI (or a blank node's label), reduced to IRI-safe
/// characters, for readable rule IRIs.
fn local(iri: &str) -> String {
    let tail = iri
        .rsplit(['#', '/', ':'])
        .find(|s| !s.is_empty())
        .unwrap_or(iri);
    let s: String = tail
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if s.is_empty() {
        "x".into()
    } else {
        s
    }
}

fn iri_term(iri: &str) -> String {
    format!("<{iri}>")
}

/// A term in SPARQL syntax; `None` for a blank node (no syntax names a stored
/// one).
fn sparql_term(t: &Term) -> Option<String> {
    match t {
        Term::NamedNode(_) | Term::Literal(_) => Some(t.to_string()),
        _ => None,
    }
}

/// `rdfs:subClassOf*` of `class` over `graphs` (the class itself included),
/// sorted: the closure the validator computes across its data graphs.
pub fn subclass_closure(store: &TripleStore, graphs: &[String], class: &str) -> Vec<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    out.insert(class.to_string());
    let Ok(c) = NamedNode::new(class) else {
        return out.into_iter().collect();
    };
    let mut queue = vec![c];
    let sub = NamedNode::new_unchecked(RDFS_SUBCLASS);
    while let Some(c) = queue.pop() {
        for g in graphs {
            let Ok(gn) = NamedNode::new(g) else { continue };
            for q in store
                .store()
                .quads_for_pattern(
                    None,
                    Some(sub.as_ref()),
                    Some(c.as_ref().into()),
                    Some(gn.as_ref().into()),
                )
                .flatten()
            {
                if let oxigraph::model::NamedOrBlankNode::NamedNode(s) = q.subject {
                    if out.insert(s.as_str().to_string()) {
                        queue.push(s);
                    }
                }
            }
        }
    }
    out.into_iter().collect()
}

/// A focus pattern: SPARQL that binds `?this` (and `?__fg`, the graph a
/// class-targeted focus node is typed in).
#[derive(Debug, Clone)]
struct Focus {
    pattern: String,
    /// Distinguishes the rules of one constraint compiled for several
    /// targets.
    key: String,
}

/// The value atom of a single-step path: `?this <p> ?v` or its inverse.
#[derive(Debug, Clone)]
struct Step {
    predicate: String,
    inverse: bool,
}

impl Step {
    fn of(path: &PropertyPath) -> Option<Step> {
        match path {
            PropertyPath::Predicate(p) => Some(Step {
                predicate: p.clone(),
                inverse: false,
            }),
            PropertyPath::Inverse(inner) => match inner.as_ref() {
                PropertyPath::Predicate(p) => Some(Step {
                    predicate: p.clone(),
                    inverse: true,
                }),
                _ => None,
            },
            _ => None,
        }
    }

    fn atom(&self, focus: &str, value: &str) -> String {
        if self.inverse {
            format!("{value} <{}> {focus}", self.predicate)
        } else {
            format!("{focus} <{}> {value}", self.predicate)
        }
    }

    fn sparql(&self) -> String {
        if self.inverse {
            format!("^<{}>", self.predicate)
        } else {
            format!("<{}>", self.predicate)
        }
    }
}

struct ShaclCompiler<'a> {
    store: &'a TripleStore,
    /// The graphs the validator reads (class closures are computed over them).
    closure_graphs: &'a [String],
    policies: Policies,
    shapes: &'a [Shape],
    out: Compiled,
    seen_report: HashSet<String>,
}

/// Compile one shapes graph of the sandbox.
pub fn compile_shapes(
    store: &TripleStore,
    shapes_graph: &str,
    closure_graphs: &[String],
    policies: Policies,
) -> Result<Compiled, String> {
    let shapes = crate::shacl::engine::load_shapes(store, shapes_graph)?;
    let mut c = ShaclCompiler {
        store,
        closure_graphs,
        policies,
        shapes: &shapes,
        out: Compiled::default(),
        seen_report: HashSet::new(),
    };
    for shape in &shapes {
        if shape.deactivated || shape.targets.is_empty() {
            continue;
        }
        for focus in c.foci(shape) {
            let mut visiting = vec![shape.iri.clone()];
            c.shape_constraints(shape, &focus, 0, &mut visiting);
        }
    }
    Ok(c.out)
}

impl<'a> ShaclCompiler<'a> {
    /// The focus patterns of a shape's targets: one for all class targets
    /// together (so a node typed with two targeted classes is one focus),
    /// one per other target.
    fn foci(&mut self, shape: &Shape) -> Vec<Focus> {
        let mut out = Vec::new();
        let mut classes: BTreeSet<String> = BTreeSet::new();
        for t in &shape.targets {
            match t {
                Target::TargetClass(c) => {
                    classes.extend(subclass_closure(self.store, self.closure_graphs, c));
                }
                Target::TargetNode(n) => match n {
                    Term::NamedNode(nn) => out.push(Focus {
                        pattern: format!("VALUES ?this {{ <{}> }}", nn.as_str()),
                        key: format!("node:{}", nn.as_str()),
                    }),
                    other => self.out.skipped.push(format!(
                        "shape <{}>: sh:targetNode {other} is not an IRI, so nothing can be added to it",
                        shape.iri
                    )),
                },
                Target::TargetSubjectsOf(p) => out.push(Focus {
                    pattern: format!("GRAPH ?__fg {{ ?this <{p}> ?__to }}"),
                    key: format!("subjectsOf:{p}"),
                }),
                Target::TargetObjectsOf(p) => out.push(Focus {
                    pattern: format!("GRAPH ?__fg {{ ?__ts <{p}> ?this }} FILTER(!isLiteral(?this))"),
                    key: format!("objectsOf:{p}"),
                }),
                Target::SparqlTarget(_) => self.out.skipped.push(format!(
                    "shape <{}>: a SPARQL target (sh:target) is not compiled",
                    shape.iri
                )),
            }
        }
        if !classes.is_empty() {
            let list: Vec<String> = classes.iter().map(|c| iri_term(c)).collect();
            out.insert(
                0,
                Focus {
                    pattern: format!(
                        "GRAPH ?__fg {{ ?this a ?__tc }} VALUES ?__tc {{ {} }}",
                        list.join(" ")
                    ),
                    key: "class".into(),
                },
            );
        }
        out
    }

    fn rule_iri(
        &self,
        kind: &str,
        shape: &str,
        path: Option<&Step>,
        focus: &Focus,
        suffix: &str,
    ) -> String {
        let p = path.map(|s| s.predicate.as_str()).unwrap_or("");
        let h = short_hash(&[shape, p, &focus.key, suffix, kind]);
        let mut iri = format!("urn:ots:rule:{kind}:{h}:{}", local(shape));
        if let Some(s) = path {
            iri.push(':');
            iri.push_str(&local(&s.predicate));
        }
        if !suffix.is_empty() {
            iri.push(':');
            iri.push_str(suffix);
        }
        iri
    }

    /// The key nulls of one constraint are minted from (target-independent).
    fn null_key(shape: &str, path: Option<&Step>, suffix: &str) -> String {
        let p = path.map(|s| s.predicate.as_str()).unwrap_or("");
        format!("urn:ots:null-key:{}", short_hash(&[shape, p, suffix]))
    }

    fn report(
        &mut self,
        source_shape: &str,
        path: Option<String>,
        constraint_text: &str,
        reason: impl Into<String>,
    ) {
        let component =
            constraint_component(constraint_text).unwrap_or_else(|| constraint_text.to_string());
        let key = format!(
            "{source_shape}|{}|{component}",
            path.as_deref().unwrap_or("")
        );
        if !self.seen_report.insert(key) {
            return;
        }
        let rule = format!(
            "urn:ots:rule:report:{}:{}:{}",
            short_hash(&[source_shape, path.as_deref().unwrap_or(""), &component]),
            local(source_shape),
            local(&component)
        );
        self.out.report_only.push(ReportOnly {
            rule,
            source_shape: source_shape.to_string(),
            path,
            source_constraint_component: component,
            reason: reason.into(),
            triggers: None,
        });
    }

    fn violation(source_shape: &str, path: Option<&Step>, constraint_text: String) -> ViolationRef {
        ViolationRef {
            source_shape: source_shape.to_string(),
            path: path.map(Step::sparql),
            source_constraint_component: constraint_component(&constraint_text).unwrap_or_default(),
            source_constraint: constraint_text,
        }
    }

    fn base_spec(&self, iri: String, construct: String, from: &str) -> RuleSpec {
        let mut spec = RuleSpec::new(iri, construct, Origin::Compiled);
        spec.derived_from = Some(from.to_string());
        spec.graph_scope = GraphScope::PerGraph;
        spec
    }

    /// Node-level constraints of `shape` and its property shapes, for the
    /// focus nodes `focus` binds.
    fn shape_constraints(
        &mut self,
        shape: &Shape,
        focus: &Focus,
        depth: usize,
        visiting: &mut Vec<String>,
    ) {
        let repairable = shape
            .severity
            .as_deref()
            .is_none_or(|s| s.ends_with("Violation"));
        for c in &shape.constraints {
            self.node_constraint(shape, c, focus, repairable, depth, visiting);
        }
        for ps in &shape.property_shapes {
            let source = ps.iri.clone().unwrap_or_else(|| shape.iri.clone());
            let repairable = ps
                .severity
                .as_deref()
                .or(shape.severity.as_deref())
                .is_none_or(|s| s.ends_with("Violation"));
            self.property_shape(&source, ps, focus, repairable, depth, visiting);
        }
    }

    fn node_constraint(
        &mut self,
        shape: &Shape,
        c: &Constraint,
        focus: &Focus,
        repairable: bool,
        depth: usize,
        visiting: &mut Vec<String>,
    ) {
        let text = constraint_text(c);
        if !repairable {
            self.report(
                &shape.iri,
                None,
                &text,
                "the shape's severity is not sh:Violation, so it is reported, not repaired",
            );
            return;
        }
        match c {
            Constraint::Class(class) => {
                let closure = subclass_closure(self.store, self.closure_graphs, class);
                let iri = self.rule_iri("class", &shape.iri, None, focus, "");
                let mut spec = self.base_spec(
                    iri,
                    format!(
                        "CONSTRUCT {{ ?this a <{class}> }} WHERE {{ {} FILTER(isIRI(?this)) }}",
                        focus.pattern
                    ),
                    &shape.iri,
                );
                spec.guard = Some(type_guard("?this", &closure));
                spec.message = Some(format!("{{?this}} must be an instance of <{class}>"));
                spec.violation = Some(Self::violation(&shape.iri, None, text));
                self.out.rules.push(spec);
            }
            Constraint::Closed {
                ignored_properties,
                allowed_properties,
            } => {
                if !self.policies.closed_delete {
                    self.report(
                        &shape.iri,
                        None,
                        &text,
                        "sh:closed: deleting the extra triples is the opt-in policy closed-delete",
                    );
                    return;
                }
                let mut allowed: BTreeSet<String> = ignored_properties.iter().cloned().collect();
                allowed.extend(allowed_properties.iter().cloned());
                // rdf:type is never deleted: dropping a targeted node's type
                // would only untarget it.
                allowed.insert(RDF_TYPE.to_string());
                let list: Vec<String> = allowed.iter().map(|p| iri_term(p)).collect();
                let iri = self.rule_iri("closed-delete", &shape.iri, None, focus, "");
                let mut spec = self.base_spec(
                    iri,
                    format!(
                        "CONSTRUCT {{ }} WHERE {{ {} ?this ?__cp ?__co . FILTER(?__cp NOT IN ({})) }}",
                        focus.pattern,
                        list.join(", ")
                    ),
                    &shape.iri,
                );
                spec.retract = Some("?this ?__cp ?__co".into());
                spec.policy_deletion = true;
                spec.destructive = true;
                spec.confidence = Confidence::Policy;
                spec.message = Some(format!(
                    "<{}> is sh:closed: {{?this}} may not carry {{?__cp}}",
                    shape.iri
                ));
                spec.violation = Some(Self::violation(&shape.iri, None, text));
                self.out.rules.push(spec);
            }
            Constraint::Node(inner) => {
                self.nested(inner, focus, depth, visiting);
            }
            Constraint::And(members) => {
                for m in members {
                    self.nested(m, focus, depth, visiting);
                }
            }
            Constraint::Property(ps) => {
                let source = ps.iri.clone().unwrap_or_else(|| shape.iri.clone());
                self.property_shape(&source, ps, focus, true, depth, visiting);
            }
            other => {
                let reason = report_reason(other);
                self.report(&shape.iri, None, &text, reason);
            }
        }
    }

    /// Follow `sh:node` / `sh:and` into an inline shape, for the same focus.
    fn nested(&mut self, inner: &Shape, focus: &Focus, depth: usize, visiting: &mut Vec<String>) {
        if depth >= MAX_DEPTH || visiting.contains(&inner.iri) {
            return;
        }
        visiting.push(inner.iri.clone());
        // An inline shape's own targets are not the outer focus nodes'.
        let mut inner = inner.clone();
        inner.targets.clear();
        self.shape_constraints(&inner, focus, depth + 1, visiting);
        visiting.pop();
    }

    /// The focus pattern for the values of `step` from `focus`: the outer
    /// focus renamed, its value as the new `?this`.
    fn value_focus(&self, focus: &Focus, step: &Step, depth: usize) -> Focus {
        let outer = format!("?__o{depth}");
        let renamed = rename_this(&focus.pattern, &outer);
        Focus {
            pattern: format!(
                "{renamed} {} . FILTER(isIRI(?this))",
                step.atom(&outer, "?this")
            ),
            key: format!(
                "{}/{}{}",
                focus.key,
                if step.inverse { "^" } else { "" },
                step.predicate
            ),
        }
    }

    fn property_shape(
        &mut self,
        source: &str,
        ps: &PropertyShape,
        focus: &Focus,
        repairable: bool,
        depth: usize,
        visiting: &mut Vec<String>,
    ) {
        let Some(step) = Step::of(&ps.path) else {
            for c in &ps.constraints {
                self.report(
                    source,
                    Some(ps.path.to_sparql()),
                    &constraint_text(c),
                    "only a single predicate (or its inverse) is repaired; this path is not",
                );
            }
            return;
        };
        let path_text = Some(step.sparql());
        for c in &ps.constraints {
            let text = constraint_text(c);
            if !repairable {
                self.report(
                    source,
                    path_text.clone(),
                    &text,
                    "the shape's severity is not sh:Violation, so it is reported, not repaired",
                );
                continue;
            }
            match c {
                Constraint::MinCount(n) => {
                    self.min_count(source, &step, *n, &ps.constraints, focus, text)
                }
                Constraint::HasValue(v) => self.has_value(source, &step, v, focus, text),
                Constraint::Class(class) => {
                    let closure = subclass_closure(self.store, self.closure_graphs, class);
                    let iri = self.rule_iri("class", source, Some(&step), focus, "");
                    let mut spec = self.base_spec(
                        iri,
                        format!(
                            "CONSTRUCT {{ ?v a <{class}> }} WHERE {{ {} {} . FILTER(isIRI(?v)) }}",
                            focus.pattern,
                            step.atom("?this", "?v")
                        ),
                        source,
                    );
                    spec.guard = Some(type_guard("?v", &closure));
                    spec.message = Some(format!(
                        "{{?v}}, a {} value of {{?this}}, must be an instance of <{class}>",
                        step.sparql()
                    ));
                    spec.violation = Some(Self::violation(source, Some(&step), text));
                    self.out.rules.push(spec);
                }
                Constraint::Datatype(dt) => {
                    if !self.policies.datatype_relabel {
                        self.report(source, path_text.clone(), &text, "sh:datatype: relabelling a literal is the opt-in policy datatype-relabel; ill-formed values stay residue");
                        continue;
                    }
                    if step.inverse {
                        self.report(
                            source,
                            path_text.clone(),
                            &text,
                            "an inverse path's values are subjects, never literals",
                        );
                        continue;
                    }
                    let iri = self.rule_iri("datatype-relabel", source, Some(&step), focus, "");
                    let mut spec = self.base_spec(
                        iri,
                        format!(
                            "CONSTRUCT {{ }} WHERE {{ {} {} . FILTER(isLiteral(?v) && datatype(?v) != <{dt}>) }}",
                            focus.pattern,
                            step.atom("?this", "?v")
                        ),
                        source,
                    );
                    spec.native = Some(Native::DatatypeRelabel {
                        datatype: dt.clone(),
                        predicate: step.predicate.clone(),
                    });
                    spec.confidence = Confidence::Policy;
                    spec.message = Some(format!("{{?v}} relabelled as <{dt}>: {{?relabelled}}"));
                    spec.violation = Some(Self::violation(source, Some(&step), text));
                    self.out.rules.push(spec);
                }
                Constraint::MaxCount(max) => {
                    if !self.policies.max_count_keep_lexmin {
                        self.report(source, path_text.clone(), &text, "sh:maxCount: which values to drop is undetermined; maxCount-keep-lexmin is the opt-in policy");
                        continue;
                    }
                    let iri = self.rule_iri("maxcount-keep-lexmin", source, Some(&step), focus, "");
                    let mut spec = self.base_spec(
                        iri,
                        format!("CONSTRUCT {{ }} WHERE {{ {} }}", focus.pattern),
                        source,
                    );
                    spec.native = Some(Native::MaxCountKeepLexmin {
                        max: *max,
                        predicate: step.predicate.clone(),
                        inverse: step.inverse,
                    });
                    spec.policy_deletion = true;
                    spec.destructive = true;
                    spec.confidence = Confidence::Policy;
                    spec.message = Some(format!(
                        "{{?this}} keeps its {max} smallest {} values; {{?surplus}} is deleted",
                        step.sparql()
                    ));
                    spec.violation = Some(Self::violation(source, Some(&step), text));
                    self.out.rules.push(spec);
                }
                Constraint::Node(inner) => {
                    if depth < MAX_DEPTH && !visiting.contains(&inner.iri) {
                        let vf = self.value_focus(focus, &step, depth);
                        self.nested(inner, &vf, depth, visiting);
                    }
                }
                Constraint::And(members) => {
                    if depth < MAX_DEPTH {
                        let vf = self.value_focus(focus, &step, depth);
                        for m in members {
                            self.nested(m, &vf, depth, visiting);
                        }
                    }
                }
                Constraint::Property(nested) => {
                    if depth < MAX_DEPTH {
                        let vf = self.value_focus(focus, &step, depth);
                        let src = nested.iri.clone().unwrap_or_else(|| source.to_string());
                        self.property_shape(&src, nested, &vf, true, depth + 1, visiting);
                    }
                }
                Constraint::In(members) if members.len() == 1 => {
                    // Determined only together with a minimum: handled there.
                    if !ps
                        .constraints
                        .iter()
                        .any(|c| matches!(c, Constraint::MinCount(n) if *n >= 1))
                    {
                        self.report(
                            source,
                            path_text.clone(),
                            &text,
                            "sh:in does not say a value is required (no sh:minCount)",
                        );
                    }
                }
                other => {
                    let reason = report_reason(other);
                    self.report(source, path_text.clone(), &text, reason);
                }
            }
        }
    }

    fn has_value(&mut self, source: &str, step: &Step, v: &Term, focus: &Focus, text: String) {
        let Some(value) = sparql_term(v) else {
            self.report(
                source,
                Some(step.sparql()),
                &text,
                "the required value is a blank node, which no patch line can name",
            );
            return;
        };
        if step.inverse && !matches!(v, Term::NamedNode(_)) {
            self.report(
                source,
                Some(step.sparql()),
                &text,
                "an inverse path's value is a subject and must be an IRI",
            );
            return;
        }
        let iri = self.rule_iri("hasvalue", source, Some(step), focus, "");
        let mut spec = self.base_spec(
            iri,
            format!(
                "CONSTRUCT {{ {} }} WHERE {{ {} }}",
                step.atom("?this", &value),
                focus.pattern
            ),
            source,
        );
        spec.guard = Some(format!("{{ {} }}", step.atom("?this", &value)));
        spec.message = Some(format!("{{?this}} must have {} {value}", step.sparql()));
        spec.violation = Some(Self::violation(source, Some(step), text));
        self.out.rules.push(spec);
    }

    fn min_count(
        &mut self,
        source: &str,
        step: &Step,
        n: usize,
        siblings: &[Constraint],
        focus: &Focus,
        text: String,
    ) {
        if n == 0 {
            return;
        }
        let path_text = Some(step.sparql());
        // A value the shape determines.
        if siblings
            .iter()
            .any(|c| matches!(c, Constraint::HasValue(_)))
        {
            if n > 1 {
                self.report(
                    source,
                    path_text,
                    &text,
                    "sh:hasValue determines one value; the others are undetermined",
                );
            }
            return; // the sh:hasValue rule adds it
        }
        if let Some(Constraint::In(members)) =
            siblings.iter().find(|c| matches!(c, Constraint::In(_)))
        {
            if members.len() == 1 && n == 1 {
                match sparql_term(&members[0])
                    .filter(|_| !step.inverse || matches!(members[0], Term::NamedNode(_)))
                {
                    Some(value) => {
                        let iri = self.rule_iri("in-single", source, Some(step), focus, "");
                        let mut spec = self.base_spec(
                            iri,
                            format!(
                                "CONSTRUCT {{ {} }} WHERE {{ {} }}",
                                step.atom("?this", &value),
                                focus.pattern
                            ),
                            source,
                        );
                        spec.guard = Some(format!("{{ {} }}", step.atom("?this", "?__v")));
                        spec.message = Some(format!(
                            "{{?this}} needs a {} value, and sh:in allows only {value}",
                            step.sparql()
                        ));
                        spec.violation = Some(Self::violation(source, Some(step), text));
                        self.out.rules.push(spec);
                    }
                    None => self.report(
                        source,
                        path_text,
                        &text,
                        "the only value sh:in allows cannot be written as this path's value",
                    ),
                }
                return;
            }
        }
        // An existential witness: only when an IRI can satisfy every sibling.
        let plan = match witness_plan(siblings, n) {
            Ok(p) => p,
            Err(reason) => {
                self.report(source, path_text, &text, reason);
                return;
            }
        };
        for class in &plan.classes {
            if let Some(closed) = self.closed_shape_for(class) {
                self.report(source, path_text, &text, format!(
                    "a witness typed <{class}> would violate <{closed}>, which is sh:closed without rdf:type ignored"
                ));
                return;
            }
        }
        let typing: String = plan
            .classes
            .iter()
            .map(|c| format!(" ?w a <{c}> ."))
            .collect();
        for k in 1..=n {
            let suffix = if n == 1 { String::new() } else { k.to_string() };
            let iri = self.rule_iri("mincount", source, Some(step), focus, &suffix);
            let mut spec = self.base_spec(
                iri,
                format!(
                    "CONSTRUCT {{ {} .{typing} }} WHERE {{ {} }}",
                    step.atom("?this", "?w"),
                    focus.pattern
                ),
                source,
            );
            spec.nulls = vec!["w".into()];
            spec.null_key = Some(Self::null_key(source, Some(step), &suffix));
            spec.guard = Some(if k == 1 {
                format!("{{ {} }}", step.atom("?this", "?__v"))
            } else {
                format!(
                    "{{ {{ SELECT ?this (COUNT(DISTINCT ?__v) AS ?__n) WHERE {{ {} }} GROUP BY ?this }} FILTER(?__n >= {k}) }}",
                    step.atom("?this", "?__v")
                )
            });
            spec.message = Some(match plan.classes.first() {
                Some(c) => format!(
                    "{{?this}} needs at least {n} {} value(s) of class <{c}>",
                    step.sparql()
                ),
                None => format!("{{?this}} needs at least {n} {} value(s)", step.sparql()),
            });
            spec.violation = Some(Self::violation(source, Some(step), text.clone()));
            self.out.rules.push(spec);
        }
    }

    /// A shape a witness typed `class` would be targeted by that is closed
    /// without `rdf:type` allowed (§3.6: such a minCount is report-only).
    fn closed_shape_for(&self, class: &str) -> Option<String> {
        for s in self.shapes {
            let targets_class = s.targets.iter().any(|t| match t {
                Target::TargetClass(c) => {
                    c == class
                        || subclass_closure(self.store, self.closure_graphs, c)
                            .iter()
                            .any(|x| x == class)
                }
                _ => false,
            });
            if !targets_class {
                continue;
            }
            for c in &s.constraints {
                if let Constraint::Closed {
                    ignored_properties,
                    allowed_properties,
                } = c
                {
                    if !ignored_properties
                        .iter()
                        .chain(allowed_properties)
                        .any(|p| p == RDF_TYPE)
                    {
                        return Some(s.iri.clone());
                    }
                }
            }
        }
        None
    }
}

/// The classes a witness must carry, when an IRI witness satisfies every
/// sibling constraint (§3.6 table, row 3); else why not.
struct WitnessPlan {
    classes: Vec<String>,
}

fn witness_plan(siblings: &[Constraint], n: usize) -> Result<WitnessPlan, String> {
    let mut classes = Vec::new();
    for c in siblings {
        match c {
            Constraint::MinCount(_) | Constraint::UniqueLang(_) | Constraint::Node(_) | Constraint::And(_) | Constraint::Property(_) => {}
            Constraint::Class(cl) => classes.push(cl.clone()),
            Constraint::NodeKind(NodeKind::IRI) | Constraint::NodeKind(NodeKind::BlankNodeOrIRI) => {}
            Constraint::NodeKind(k) => {
                return Err(format!("sh:nodeKind {k:?} needs a value a null (an IRI) cannot be"))
            }
            Constraint::MaxCount(m) if *m < n => {
                return Err(format!("sh:maxCount {m} is below sh:minCount {n}: no witness can satisfy both"))
            }
            Constraint::MaxCount(_) => {}
            Constraint::Datatype(_)
            | Constraint::Pattern { .. }
            | Constraint::MinLength(_)
            | Constraint::MaxLength(_)
            | Constraint::MinExclusive(_)
            | Constraint::MinInclusive(_)
            | Constraint::MaxExclusive(_)
            | Constraint::MaxInclusive(_)
            | Constraint::LanguageIn(_)
            | Constraint::In(_) => {
                return Err(format!(
                    "{} constrains the value to something a null cannot be (a null is an IRI, never a literal)",
                    constraint_text(c)
                ))
            }
            other => {
                return Err(format!(
                    "{} cannot be checked for a fresh witness, so none is minted",
                    constraint_text(other)
                ))
            }
        }
    }
    classes.sort();
    classes.dedup();
    Ok(WitnessPlan { classes })
}

/// `{ ?x a ?__c . VALUES ?__c { closure } }`: `?x` is an instance of the
/// class or one of its subclasses, typed in any graph.
fn type_guard(x: &str, closure: &[String]) -> String {
    let list: Vec<String> = closure.iter().map(|c| iri_term(c)).collect();
    format!("{{ {x} a ?__c . VALUES ?__c {{ {} }} }}", list.join(" "))
}

/// Replace the variable `?this` (a whole token) with `to`.
fn rename_this(pattern: &str, to: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut rest = pattern;
    while let Some(i) = rest.find("?this") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 5..];
        let continues = after
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        if continues {
            out.push_str("?this");
        } else {
            out.push_str(to);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// The engine's `source_constraint` text for a constraint (the form
/// `ValidationResult::source_constraint` carries).
pub fn constraint_text(c: &Constraint) -> String {
    use crate::shacl::constraints::display_term;
    match c {
        Constraint::Class(x) => format!("sh:class <{x}>"),
        Constraint::Datatype(x) => format!("sh:datatype <{x}>"),
        Constraint::NodeKind(k) => format!("sh:nodeKind {k:?}"),
        Constraint::MinCount(n) => format!("sh:minCount {n}"),
        Constraint::MaxCount(n) => format!("sh:maxCount {n}"),
        Constraint::MinExclusive(t) => format!("sh:minExclusive {}", display_term(t)),
        Constraint::MinInclusive(t) => format!("sh:minInclusive {}", display_term(t)),
        Constraint::MaxExclusive(t) => format!("sh:maxExclusive {}", display_term(t)),
        Constraint::MaxInclusive(t) => format!("sh:maxInclusive {}", display_term(t)),
        Constraint::MinLength(n) => format!("sh:minLength {n}"),
        Constraint::MaxLength(n) => format!("sh:maxLength {n}"),
        Constraint::Pattern { pattern, .. } => format!("sh:pattern \"{pattern}\""),
        Constraint::LanguageIn(_) => "sh:languageIn".into(),
        Constraint::UniqueLang(_) => "sh:uniqueLang true".into(),
        Constraint::Equals(p) => format!("sh:equals <{p}>"),
        Constraint::Disjoint(p) => format!("sh:disjoint <{p}>"),
        Constraint::LessThan(p) => format!("sh:lessThan <{p}>"),
        Constraint::LessThanOrEquals(p) => format!("sh:lessThanOrEquals <{p}>"),
        Constraint::Not(_) => "sh:not".into(),
        Constraint::And(_) => "sh:and".into(),
        Constraint::Or(_) => "sh:or".into(),
        Constraint::Xone(_) => "sh:xone".into(),
        Constraint::Node(s) => format!("sh:node <{}>", s.iri),
        Constraint::Property(_) => "sh:property".into(),
        Constraint::QualifiedValueShape { min_count, .. } => match min_count {
            Some(n) => format!("sh:qualifiedMinCount {n}"),
            None => "sh:qualifiedMaxCount".into(),
        },
        Constraint::Closed { .. } => "sh:closed true".into(),
        Constraint::HasValue(v) => format!("sh:hasValue {}", display_term(v)),
        Constraint::In(_) => "sh:in".into(),
        Constraint::SparqlConstraint { .. } => "sh:SPARQLConstraint".into(),
        Constraint::Custom(cc) => cc.component.clone(),
        Constraint::Expression { .. } => "sh:expression".into(),
    }
}

/// Why a constraint is report-only (§3.6 table, §6 "out of reach").
fn report_reason(c: &Constraint) -> String {
    match c {
        Constraint::In(_) => "sh:in with several members: which member is undetermined".into(),
        Constraint::Pattern { .. } => {
            "sh:pattern constrains but does not determine the value".into()
        }
        Constraint::MinLength(_) | Constraint::MaxLength(_) => {
            "a length bound constrains but does not determine the value".into()
        }
        Constraint::MinExclusive(_)
        | Constraint::MinInclusive(_)
        | Constraint::MaxExclusive(_)
        | Constraint::MaxInclusive(_) => {
            "a range bound constrains but does not determine the value".into()
        }
        Constraint::LanguageIn(_) => {
            "sh:languageIn constrains but does not determine the value".into()
        }
        Constraint::NodeKind(_) => "sh:nodeKind constrains but does not determine the value".into(),
        Constraint::UniqueLang(_) => "sh:uniqueLang: which value to drop is undetermined".into(),
        Constraint::Equals(_)
        | Constraint::Disjoint(_)
        | Constraint::LessThan(_)
        | Constraint::LessThanOrEquals(_) => {
            "a property-pair constraint: which side is authoritative is undetermined".into()
        }
        Constraint::Or(_) | Constraint::Xone(_) => {
            "a disjunction: which branch to satisfy is undetermined".into()
        }
        Constraint::Not(_) => "sh:not needs an anti-repair, which is not made".into(),
        Constraint::QualifiedValueShape { .. } => {
            "a qualified value shape needs a disjunct choice".into()
        }
        Constraint::HasValue(_) => {
            "sh:hasValue on a node shape: the focus node itself would have to change".into()
        }
        Constraint::SparqlConstraint { .. } => {
            "out of reach: a SPARQL constraint has no head to invert".into()
        }
        Constraint::Custom(_) => {
            "out of reach: a constraint component has no head to invert".into()
        }
        Constraint::Expression { .. } => "out of reach: sh:expression has no head to invert".into(),
        other => format!("{} is not repaired", constraint_text(other)),
    }
}

// ── OWL ─────────────────────────────────────────────────────────────────────

fn select_rows(
    store: &TripleStore,
    graphs: &[String],
    query: &str,
    vars: &[&str],
) -> Result<Vec<Vec<Option<Term>>>, String> {
    let mut prepared = store
        .query_options()
        .parse_query(query)
        .map_err(|e| e.to_string())?;
    {
        let ds = prepared.dataset_mut();
        ds.set_default_graph(
            graphs
                .iter()
                .filter_map(|g| NamedNode::new(g).ok())
                .map(oxigraph::model::GraphName::NamedNode)
                .collect(),
        );
        ds.set_available_named_graphs(Vec::new());
    }
    let QueryResults::Solutions(sols) = prepared
        .on_store(store.store())
        .execute()
        .map_err(|e| e.to_string())?
    else {
        return Ok(Vec::new());
    };
    let mut rows = Vec::new();
    for s in sols {
        let s = s.map_err(|e| e.to_string())?;
        rows.push(vars.iter().map(|v| s.get(*v).cloned()).collect());
    }
    Ok(rows)
}

fn iri_of(t: &Option<Term>) -> Option<String> {
    match t {
        Some(Term::NamedNode(n)) => Some(n.as_str().to_string()),
        _ => None,
    }
}

/// The members of an RDF list, at most 64 cells (`has_keys()`'s cap).
fn list_members(store: &TripleStore, graphs: &[String], head: &Term) -> Vec<Term> {
    const FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
    const REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
    const NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
    let mut out = Vec::new();
    let mut cur = head.clone();
    let gs: Vec<NamedNode> = graphs
        .iter()
        .filter_map(|g| NamedNode::new(g).ok())
        .collect();
    let one = |s: &Term, p: &str| -> Option<Term> {
        let subject = match s {
            Term::NamedNode(n) => oxigraph::model::NamedOrBlankNodeRef::NamedNode(n.as_ref()),
            Term::BlankNode(b) => oxigraph::model::NamedOrBlankNodeRef::BlankNode(b.as_ref()),
            _ => return None,
        };
        let p = NamedNode::new_unchecked(p);
        gs.iter().find_map(|g| {
            store
                .store()
                .quads_for_pattern(
                    Some(subject),
                    Some(p.as_ref()),
                    None,
                    Some(g.as_ref().into()),
                )
                .flatten()
                .next()
                .map(|q| q.object)
        })
    };
    for _ in 0..64 {
        if matches!(&cur, Term::NamedNode(n) if n.as_str() == NIL) {
            break;
        }
        let Some(first) = one(&cur, FIRST) else { break };
        out.push(first);
        let Some(rest) = one(&cur, REST) else { break };
        cur = rest;
    }
    out
}

/// Rules from the OWL axioms in `premises` (§3.7): `owl:hasKey` and
/// (inverse) functional properties as EGDs, `owl:someValuesFrom` and
/// `owl:minCardinality 1` restrictions as existential TGDs, and — when
/// `entailment_rules` — `rdfs:domain` / `rdfs:range` / `rdfs:subClassOf` as
/// ground TGDs.
pub fn compile_owl(
    store: &TripleStore,
    premises: &[String],
    entailment_rules: bool,
) -> Result<Compiled, String> {
    let mut out = Compiled::default();
    // owl:hasKey → EGD over two keyed instances (prp-key; IRIs only).
    let keys = select_rows(
        store,
        premises,
        &format!("SELECT ?c ?l WHERE {{ ?c <{OWL}hasKey> ?l }}"),
        &["c", "l"],
    )?;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for row in keys {
        let (Some(c), Some(l)) = (iri_of(&row[0]), row[1].clone()) else {
            continue;
        };
        let props: Vec<String> = list_members(store, premises, &l)
            .into_iter()
            .filter_map(|t| match t {
                Term::NamedNode(n) => Some(n.as_str().to_string()),
                _ => None,
            })
            .collect();
        if props.is_empty() || !seen.insert(format!("{c}|{}", props.join(" "))) {
            continue;
        }
        let atoms_x: Vec<String> = props
            .iter()
            .enumerate()
            .map(|(i, p)| format!("?x <{p}> ?k{i} ."))
            .collect();
        let atoms_y: Vec<String> = props
            .iter()
            .enumerate()
            .map(|(i, p)| format!("?y <{p}> ?k{i} ."))
            .collect();
        let iri = format!(
            "urn:ots:rule:haskey:{}:{}",
            short_hash(&[&c, &props.join(" ")]),
            local(&c)
        );
        let mut spec = RuleSpec::new(
            iri,
            format!(
                "CONSTRUCT {{ }} WHERE {{ ?x a <{c}> . {} ?y a <{c}> . {} FILTER(?x != ?y) FILTER(isIRI(?x)) FILTER(isIRI(?y)) }}",
                atoms_x.join(" "),
                atoms_y.join(" ")
            ),
            Origin::Compiled,
        );
        spec.equate = Some(("x".into(), "y".into()));
        spec.merge_mode = MergeMode::SameAsOnly;
        spec.graph_scope = GraphScope::Union;
        spec.derived_from = Some(c.clone());
        let keys_text: Vec<String> = props.iter().map(|p| format!("<{p}>")).collect();
        spec.message = Some(format!(
            "{{?x}} and {{?y}} share the key {} of <{c}>",
            keys_text.join(", ")
        ));
        out.rules.push(spec);
    }
    // owl:FunctionalProperty → EGD on two objects of one subject (prp-fp).
    for row in select_rows(
        store,
        premises,
        &format!("SELECT DISTINCT ?p WHERE {{ ?p a <{OWL}FunctionalProperty> }}"),
        &["p"],
    )? {
        let Some(p) = iri_of(&row[0]) else { continue };
        let iri = format!(
            "urn:ots:rule:functional:{}:{}",
            short_hash(&[&p]),
            local(&p)
        );
        let mut spec = RuleSpec::new(
            iri,
            format!("CONSTRUCT {{ }} WHERE {{ ?s <{p}> ?y1 . ?s <{p}> ?y2 . FILTER(?y1 != ?y2) FILTER(!isBlank(?y1) && !isBlank(?y2)) }}"),
            Origin::Compiled,
        );
        spec.equate = Some(("y1".into(), "y2".into()));
        spec.graph_scope = GraphScope::Union;
        spec.derived_from = Some(p.clone());
        spec.message = Some(format!(
            "<{p}> is functional: {{?s}} has both {{?y1}} and {{?y2}}"
        ));
        out.rules.push(spec);
    }
    // owl:InverseFunctionalProperty → EGD on two subjects of one object (prp-ifp).
    for row in select_rows(
        store,
        premises,
        &format!("SELECT DISTINCT ?p WHERE {{ ?p a <{OWL}InverseFunctionalProperty> }}"),
        &["p"],
    )? {
        let Some(p) = iri_of(&row[0]) else { continue };
        let iri = format!(
            "urn:ots:rule:inverse-functional:{}:{}",
            short_hash(&[&p]),
            local(&p)
        );
        let mut spec = RuleSpec::new(
            iri,
            format!("CONSTRUCT {{ }} WHERE {{ ?x1 <{p}> ?o . ?x2 <{p}> ?o . FILTER(?x1 != ?x2) FILTER(isIRI(?x1) && isIRI(?x2)) }}"),
            Origin::Compiled,
        );
        spec.equate = Some(("x1".into(), "x2".into()));
        spec.graph_scope = GraphScope::Union;
        spec.derived_from = Some(p.clone());
        spec.message = Some(format!(
            "<{p}> is inverse-functional: {{?x1}} and {{?x2}} share {{?o}}"
        ));
        out.rules.push(spec);
    }
    // owl:someValuesFrom / minCardinality 1 → existential TGD.
    let restrictions = select_rows(
        store,
        premises,
        &format!(
            "SELECT DISTINCT ?c ?p ?d ?n ?q WHERE {{ ?c <{RDFS_SUBCLASS}> ?r . ?r <{OWL}onProperty> ?p . \
             {{ ?r <{OWL}someValuesFrom> ?d }} UNION {{ ?r <{OWL}minCardinality> ?n }} \
             UNION {{ ?r <{OWL}minQualifiedCardinality> ?q ; <{OWL}onClass> ?d }} }}"
        ),
        &["c", "p", "d", "n", "q"],
    )?;
    for row in restrictions {
        let (Some(c), Some(p)) = (iri_of(&row[0]), iri_of(&row[1])) else {
            continue;
        };
        let d = iri_of(&row[2]).filter(|d| d != &format!("{OWL}Thing"));
        let card = match (&row[3], &row[4]) {
            (Some(Term::Literal(l)), _) | (_, Some(Term::Literal(l))) => {
                l.value().trim().parse::<u64>().ok()
            }
            _ => None,
        };
        let constraint = match card {
            Some(0) => continue,
            Some(1) => format!("owl:minCardinality 1 on <{p}>"),
            Some(n) => {
                out.report_only.push(ReportOnly {
                    rule: format!("urn:ots:rule:report:{}:{}:mincardinality", short_hash(&[&c, &p]), local(&c)),
                    source_shape: c.clone(),
                    path: Some(format!("<{p}>")),
                    source_constraint_component: format!("{OWL}minCardinality"),
                    reason: format!("owl:minCardinality {n} needs {n} distinct witnesses, which OWL's open world cannot make distinct without owl:differentFrom"),
                    triggers: None,
                });
                continue;
            }
            None if row[2].is_some() => format!("owl:someValuesFrom on <{p}>"),
            None => continue,
        };
        let typing = d
            .as_ref()
            .map(|d| format!(" ?w a <{d}> ."))
            .unwrap_or_default();
        let iri = format!(
            "urn:ots:rule:somevalues:{}:{}:{}",
            short_hash(&[&c, &p, d.as_deref().unwrap_or("")]),
            local(&c),
            local(&p)
        );
        let mut spec = RuleSpec::new(
            iri,
            format!("CONSTRUCT {{ ?this <{p}> ?w .{typing} }} WHERE {{ GRAPH ?__fg {{ ?this a <{c}> }} }}"),
            Origin::Compiled,
        );
        spec.nulls = vec!["w".into()];
        spec.graph_scope = GraphScope::PerGraph;
        spec.derived_from = Some(c.clone());
        // Satisfied by any value of the right class (any value for owl:Thing).
        spec.guard = Some(match &d {
            Some(d) => format!("{{ ?this <{p}> ?__v . ?__v a <{d}> }}"),
            None => format!("{{ ?this <{p}> ?__v }}"),
        });
        spec.message = Some(match &d {
            Some(d) => format!("<{c}> instances need a <{p}> value of class <{d}> ({constraint})"),
            None => format!("<{c}> instances need a <{p}> value ({constraint})"),
        });
        out.rules.push(spec);
    }
    if entailment_rules {
        for row in select_rows(
            store,
            premises,
            "SELECT DISTINCT ?p ?d WHERE { ?p <http://www.w3.org/2000/01/rdf-schema#domain> ?d }",
            &["p", "d"],
        )? {
            let (Some(p), Some(d)) = (iri_of(&row[0]), iri_of(&row[1])) else {
                continue;
            };
            let mut spec = RuleSpec::new(
                format!(
                    "urn:ots:rule:domain:{}:{}",
                    short_hash(&[&p, &d]),
                    local(&p)
                ),
                format!("CONSTRUCT {{ ?s a <{d}> }} WHERE {{ ?s <{p}> ?o FILTER(isIRI(?s)) }}"),
                Origin::Compiled,
            );
            spec.graph_scope = GraphScope::Union;
            spec.derived_from = Some(p.clone());
            spec.message = Some(format!("rdfs:domain of <{p}> is <{d}>"));
            out.rules.push(spec);
        }
        for row in select_rows(
            store,
            premises,
            "SELECT DISTINCT ?p ?r WHERE { ?p <http://www.w3.org/2000/01/rdf-schema#range> ?r }",
            &["p", "r"],
        )? {
            let (Some(p), Some(r)) = (iri_of(&row[0]), iri_of(&row[1])) else {
                continue;
            };
            let mut spec = RuleSpec::new(
                format!("urn:ots:rule:range:{}:{}", short_hash(&[&p, &r]), local(&p)),
                format!("CONSTRUCT {{ ?o a <{r}> }} WHERE {{ ?s <{p}> ?o FILTER(isIRI(?o)) }}"),
                Origin::Compiled,
            );
            spec.graph_scope = GraphScope::Union;
            spec.derived_from = Some(p.clone());
            spec.message = Some(format!("rdfs:range of <{p}> is <{r}>"));
            out.rules.push(spec);
        }
        for row in select_rows(store, premises, &format!("SELECT DISTINCT ?c ?d WHERE {{ ?c <{RDFS_SUBCLASS}> ?d FILTER(isIRI(?c) && isIRI(?d) && ?c != ?d) }}"), &["c", "d"])? {
            let (Some(c), Some(d)) = (iri_of(&row[0]), iri_of(&row[1])) else { continue };
            let mut spec = RuleSpec::new(
                format!("urn:ots:rule:subclass:{}:{}", short_hash(&[&c, &d]), local(&c)),
                format!("CONSTRUCT {{ ?x a <{d}> }} WHERE {{ ?x a <{c}> FILTER(isIRI(?x)) }}"),
                Origin::Compiled,
            );
            spec.graph_scope = GraphScope::Union;
            spec.derived_from = Some(c.clone());
            spec.message = Some(format!("<{c}> rdfs:subClassOf <{d}>"));
            out.rules.push(spec);
        }
    }
    Ok(out)
}

/// The `owl:sameAs` pairs among `premises` (IRIs only): the equality
/// structure's seeds when the identity policy propagates them.
pub fn same_as_pairs(
    store: &TripleStore,
    premises: &[String],
) -> Result<Vec<(Term, Term)>, String> {
    let rows = select_rows(
        store,
        premises,
        "SELECT DISTINCT ?a ?b WHERE { ?a <http://www.w3.org/2002/07/owl#sameAs> ?b FILTER(isIRI(?a) && isIRI(?b) && ?a != ?b) }",
        &["a", "b"],
    )?;
    Ok(rows
        .into_iter()
        .filter_map(|r| Some((r[0].clone()?, r[1].clone()?)))
        .collect())
}

// ── SHACL-AF ────────────────────────────────────────────────────────────────

/// One SHACL-AF rule as the import reads it from the shapes graph.
enum AfBody {
    /// `sh:SPARQLRule`: the `sh:construct` text with its `sh:prefixes`
    /// prologue in front.
    Construct(String),
    /// `sh:TripleRule`: subject, predicate, object; `None` is `sh:this`.
    Triple(Option<Term>, Term, Option<Term>),
}

struct AfRule {
    shape_iri: String,
    body: AfBody,
    order: f64,
    conditions: Vec<Shape>,
}

/// The value of `node`'s `predicate` in the shapes graph, through the quad
/// index so a blank-node subject (`sh:rule [ … ]`) resolves.
fn af_value(store: &TripleStore, graph: &str, node: &str, predicate: &str) -> Option<Term> {
    store
        .objects_for_subject_in_graph(node, predicate, Some(graph))
        .into_iter()
        .next()
}

/// A term as `objects_for_subject_in_graph` addresses a subject.
fn af_node(t: &Term) -> String {
    match t {
        Term::NamedNode(n) => n.as_str().to_string(),
        Term::BlankNode(b) => format!("_:{}", b.as_str()),
        other => other.to_string(),
    }
}

/// The `PREFIX` prologue of a node's `sh:prefixes` (SHACL §5.2.1), following
/// `owl:imports` between prefix owners.
fn af_prefixes(store: &TripleStore, graph: &str, node: &str) -> String {
    let mut out = String::new();
    let mut owners: Vec<String> = store
        .objects_for_subject_in_graph(node, &format!("{SH}prefixes"), Some(graph))
        .iter()
        .map(af_node)
        .collect();
    let mut i = 0;
    while i < owners.len() {
        let owner = owners[i].clone();
        i += 1;
        for imported in store
            .objects_for_subject_in_graph(&owner, &format!("{OWL}imports"), Some(graph))
            .iter()
            .map(af_node)
        {
            if owners.len() < 64 && !owners.contains(&imported) {
                owners.push(imported);
            }
        }
        for decl in store
            .objects_for_subject_in_graph(&owner, &format!("{SH}declare"), Some(graph))
            .iter()
            .map(af_node)
        {
            let lexical = |p: &str| match af_value(store, graph, &decl, &format!("{SH}{p}")) {
                Some(Term::Literal(l)) => Some(l.value().to_string()),
                Some(Term::NamedNode(n)) => Some(n.as_str().to_string()),
                _ => None,
            };
            if let (Some(p), Some(ns)) = (lexical("prefix"), lexical("namespace")) {
                out.push_str(&format!("PREFIX {p}: <{ns}>\n"));
            }
        }
    }
    out
}

/// Read the shapes graph's `sh:rule`s. A rule whose `sh:condition` is not a
/// named shape of the graph, or whose triple-rule term is a node expression,
/// is left out with the reason in `skipped`: a rule imported without its
/// condition would fire where SHACL-AF says it does not.
fn af_rules(
    store: &TripleStore,
    graph: &str,
    shapes: &std::collections::HashMap<String, Shape>,
    skipped: &mut Vec<String>,
) -> Result<Vec<AfRule>, String> {
    let q = format!(
        "SELECT ?shape ?rule WHERE {{ GRAPH <{}> {{ ?shape <{SH}rule> ?rule }} }}",
        crate::store::escape_sparql_iri(graph)
    );
    let mut pairs: Vec<(String, Term)> = Vec::new();
    if let QueryResults::Solutions(sols) = store.query(&q).map_err(|e| e.to_string())? {
        for sol in sols {
            let sol = sol.map_err(|e| e.to_string())?;
            let (Some(Term::NamedNode(shape)), Some(rule)) = (sol.get("shape"), sol.get("rule"))
            else {
                continue;
            };
            pairs.push((shape.as_str().to_string(), rule.clone()));
        }
    }
    let truthy = |t: Option<Term>| match t {
        Some(Term::Literal(l)) => l.value() == "true" || l.value() == "1",
        _ => false,
    };
    let mut rules = Vec::new();
    for (shape_iri, rule) in pairs {
        let node = af_node(&rule);
        if truthy(af_value(store, graph, &node, &format!("{SH}deactivated")))
            || truthy(af_value(
                store,
                graph,
                &shape_iri,
                &format!("{SH}deactivated"),
            ))
        {
            continue;
        }
        let order = match af_value(store, graph, &node, &format!("{SH}order")) {
            Some(Term::Literal(l)) => l.value().trim().parse::<f64>().unwrap_or(0.0),
            _ => 0.0,
        };
        let mut conditions = Vec::new();
        let mut unresolved = None;
        for c in store.objects_for_subject_in_graph(&node, &format!("{SH}condition"), Some(graph)) {
            match &c {
                Term::NamedNode(n) if shapes.contains_key(n.as_str()) => {
                    conditions.push(shapes[n.as_str()].clone())
                }
                other => unresolved = Some(other.to_string()),
            }
        }
        if let Some(c) = unresolved {
            skipped.push(format!(
                "a rule of <{shape_iri}>: its sh:condition {c} is not a named shape of the shapes graph"
            ));
            continue;
        }
        let body = if let Some(Term::Literal(c)) =
            af_value(store, graph, &node, &format!("{SH}construct"))
        {
            AfBody::Construct(format!("{}{}", af_prefixes(store, graph, &node), c.value()))
        } else {
            let term = |p: &str| af_value(store, graph, &node, &format!("{SH}{p}"));
            let (Some(s), Some(p), Some(o)) = (term("subject"), term("predicate"), term("object"))
            else {
                continue;
            };
            let this = |t: Term| match t {
                Term::NamedNode(n) if n.as_str() == format!("{SH}this") => Ok(None),
                Term::NamedNode(_) | Term::Literal(_) => Ok(Some(t)),
                other => Err(other),
            };
            match (this(s), p, this(o)) {
                (Ok(s), p @ Term::NamedNode(_), Ok(o)) => AfBody::Triple(s, p, o),
                _ => {
                    skipped.push(format!(
                        "a triple rule of <{shape_iri}>: a node expression is not imported"
                    ));
                    continue;
                }
            }
        };
        rules.push(AfRule {
            shape_iri,
            body,
            order,
            conditions,
        });
    }
    let text = |r: &AfRule| match &r.body {
        AfBody::Construct(c) => c.clone(),
        AfBody::Triple(s, p, o) => format!("{s:?} {p} {o:?}"),
    };
    rules.sort_by(|a, b| (a.shape_iri.as_str(), text(a)).cmp(&(b.shape_iri.as_str(), text(b))));
    Ok(rules)
}

/// Import a shapes graph's SHACL-AF `sh:rule`s as TGDs: the rule's targets
/// bind `?this`, a template blank node becomes a labelled null (so an
/// existential rule that never converges under `/infer` converges here),
/// `sh:order` is the priority and `sh:condition` is checked per focus node.
/// A rule's IRI is derived from its shape and its text, so it is the same
/// across runs.
pub fn import_shacl_af(
    store: &TripleStore,
    shapes_graph: &str,
    closure_graphs: &[String],
) -> Result<Compiled, String> {
    use spargebra::term::{TermPattern, TriplePattern, Variable};
    let mut out = Compiled::default();
    let shapes: std::collections::HashMap<String, Shape> =
        crate::shacl::engine::load_shapes(store, shapes_graph)?
            .into_iter()
            .map(|s| (s.iri.clone(), s))
            .collect();
    let rules = af_rules(store, shapes_graph, &shapes, &mut out.skipped)?;
    let shape = |iri: &str| Shape {
        iri: iri.to_string(),
        name: None,
        shape_type: crate::shacl::shapes::ShapeType::NodeShape,
        targets: shapes
            .get(iri)
            .map(|s| s.targets.clone())
            .unwrap_or_default(),
        constraints: Vec::new(),
        property_shapes: Vec::new(),
        severity: None,
        message: None,
        deactivated: false,
    };
    for rule in &rules {
        let body_text = match &rule.body {
            AfBody::Construct(c) => c.clone(),
            AfBody::Triple(s, p, o) => format!("{s:?} {p} {o:?}"),
        };
        let rule_key = short_hash(&[&rule.shape_iri, &body_text]);
        let mut c = ShaclCompiler {
            store,
            closure_graphs,
            policies: Policies::default(),
            shapes: &[],
            out: Compiled::default(),
            seen_report: HashSet::new(),
        };
        let foci = c.foci(&shape(&rule.shape_iri));
        out.skipped.extend(c.out.skipped);
        let (template, pattern, nulls) = match &rule.body {
            AfBody::Construct(text) => {
                let query = super::rules::parse_construct(text)
                    .map_err(|e| format!("rule of shape <{}>: {e}", rule.shape_iri))?;
                let spargebra::Query::Construct {
                    template, pattern, ..
                } = query
                else {
                    continue;
                };
                let mut nulls: Vec<String> = Vec::new();
                let mut rename = |t: &TermPattern| -> TermPattern {
                    match t {
                        TermPattern::BlankNode(b) => {
                            let name =
                                format!("__bn_{}", local(b.as_str()).replace(['-', '.'], "_"));
                            if !nulls.contains(&name) {
                                nulls.push(name.clone());
                            }
                            TermPattern::Variable(Variable::new_unchecked(name))
                        }
                        other => other.clone(),
                    }
                };
                let template: Vec<TriplePattern> = template
                    .iter()
                    .map(|tp| TriplePattern {
                        subject: rename(&tp.subject),
                        predicate: tp.predicate.clone(),
                        object: rename(&tp.object),
                    })
                    .collect();
                (template, super::rules::unwrap_star(pattern), nulls)
            }
            AfBody::Triple(s, p, o) => {
                let term = |t: &Option<Term>| match t {
                    None => TermPattern::Variable(Variable::new_unchecked("this")),
                    Some(Term::NamedNode(n)) => TermPattern::NamedNode(n.clone()),
                    Some(Term::Literal(l)) => TermPattern::Literal(l.clone()),
                    Some(_) => unreachable!("af_rules keeps IRIs, literals and sh:this"),
                };
                let Term::NamedNode(p) = p else {
                    unreachable!("af_rules keeps an IRI predicate");
                };
                (
                    vec![TriplePattern {
                        subject: term(s),
                        predicate: spargebra::term::NamedNodePattern::NamedNode(p.clone()),
                        object: term(o),
                    }],
                    spargebra::algebra::GraphPattern::Bgp {
                        patterns: Vec::new(),
                    },
                    Vec::new(),
                )
            }
        };
        for (fi, focus) in foci.iter().enumerate() {
            let focus_pattern = {
                let q = spargebra::SparqlParser::new()
                    .parse_query(&format!("ASK WHERE {{ {} }}", focus.pattern))
                    .map_err(|e| format!("rule of shape <{}>: {e}", rule.shape_iri))?;
                match q {
                    spargebra::Query::Ask { pattern, .. } => super::rules::unwrap_star(pattern),
                    _ => continue,
                }
            };
            let joined = spargebra::algebra::GraphPattern::Join {
                left: Box::new(focus_pattern),
                right: Box::new(pattern.clone()),
            };
            // Written out by hand: spargebra prints a CONSTRUCT's WHERE clause
            // as a `SELECT *` sub-query, which the rule checker refuses.
            let head: String = template.iter().map(|t| format!("{t} . ")).collect();
            let construct = format!("CONSTRUCT {{ {head}}} WHERE {{ {joined} }}");
            let iri = format!(
                "urn:ots:rule:shacl-af:{}:{}{}",
                short_hash(&[&rule_key, &focus.key]),
                local(&rule.shape_iri),
                if foci.len() > 1 {
                    format!(":{fi}")
                } else {
                    String::new()
                }
            );
            let mut spec = RuleSpec::new(iri, construct, Origin::ShaclAf);
            spec.nulls = nulls.clone();
            spec.null_key = Some(format!("urn:ots:null-key:{rule_key}"));
            spec.priority = rule.order;
            spec.derived_from = Some(rule.shape_iri.clone());
            spec.conditions = rule.conditions.clone();
            spec.policy = Policy::Repair;
            out.rules.push(spec);
        }
    }
    Ok(out)
}

#[cfg(test)]
/// Count, per `sh:minCount` occurrence in `shapes`, whether its siblings
/// admit an IRI witness (§3.6 "sibling-pair census"). Returns
/// `(total, witness, ground, report)`.
pub fn min_count_census(shapes: &[Shape]) -> (usize, usize, usize, usize) {
    let (mut total, mut witness, mut ground, mut report) = (0, 0, 0, 0);
    let mut visit = |constraints: &[Constraint]| {
        for c in constraints {
            let Constraint::MinCount(n) = c else { continue };
            if *n == 0 {
                continue;
            }
            total += 1;
            let determined = constraints.iter().any(|s| match s {
                Constraint::HasValue(_) => true,
                Constraint::In(m) => m.len() == 1 && *n == 1,
                _ => false,
            });
            if determined {
                ground += 1;
            } else if witness_plan(constraints, *n).is_ok() {
                witness += 1;
            } else {
                report += 1;
            }
        }
    };
    fn walk(s: &Shape, visit: &mut dyn FnMut(&[Constraint]), depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        for ps in &s.property_shapes {
            visit(&ps.constraints);
            for c in &ps.constraints {
                if let Constraint::Property(nested) = c {
                    visit(&nested.constraints);
                }
            }
        }
    }
    for s in shapes {
        walk(s, &mut visit, 0);
    }
    (total, witness, ground, report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_this_replaces_whole_tokens_only() {
        assert_eq!(
            rename_this("GRAPH ?__fg { ?this a ?__tc } ?thisX ?this.", "?__o0"),
            "GRAPH ?__fg { ?__o0 a ?__tc } ?thisX ?__o0."
        );
    }

    #[test]
    fn policies_parse_and_refuse_unknown_names() {
        let p = Policies::parse(&["closed-delete".into(), "datatype-relabel".into()]).unwrap();
        assert!(p.closed_delete && p.datatype_relabel && !p.max_count_keep_lexmin);
        assert!(Policies::parse(&["delete-everything".into()])
            .unwrap_err()
            .contains("unknown policy"));
    }

    #[test]
    fn witness_admissibility_follows_the_table() {
        use crate::shacl::shapes::NodeKind;
        assert!(witness_plan(&[Constraint::MinCount(1)], 1).is_ok());
        assert_eq!(
            witness_plan(
                &[Constraint::MinCount(1), Constraint::Class("urn:C".into())],
                1
            )
            .unwrap()
            .classes,
            vec!["urn:C".to_string()]
        );
        assert!(witness_plan(&[Constraint::NodeKind(NodeKind::IRI)], 1).is_ok());
        assert!(witness_plan(&[Constraint::NodeKind(NodeKind::Literal)], 1).is_err());
        assert!(witness_plan(&[Constraint::NodeKind(NodeKind::IRIOrLiteral)], 1).is_err());
        assert!(witness_plan(&[Constraint::Datatype("urn:dt".into())], 1).is_err());
        assert!(witness_plan(&[Constraint::MaxCount(1)], 2).is_err());
        assert!(witness_plan(&[Constraint::MaxCount(2)], 2).is_ok());
        assert!(witness_plan(&[Constraint::Or(Vec::new())], 1).is_err());
    }
}
