//! The OWL 2 DL backend protocol: one request in, one structured outcome out.
//!
//! [`DlBackend`] is implemented by the Konclude bridge
//! ([`super::konclude_bridge`]) and the HTTP sidecar client
//! ([`super::dl_sidecar`]); the native rules ([`super::owl2_dl`]) are driven
//! directly because they reason inside the store. [`materialize`] and [`check`]
//! are what the HTTP layer calls. Every path:
//!
//! 1. refuses when no backend is configured (`Unavailable`, 503) — there is no
//!    silent fallback from an external backend to the native rules;
//! 2. refuses `sameas-off` with an external backend (`NotSupported`, 422);
//! 3. caps the input handed to an external backend (`TooLarge`, 413);
//! 4. maps the RDF to OWL 2 and checks the OWL 2 DL profile (`NotInProfile`,
//!    422 with the violations);
//! 5. runs the backend under a time limit (`Timeout`, 504: the answer is
//!    unknown), and
//! 6. writes only triples about named entities into the target graph.

use std::collections::HashSet;
use std::time::Instant;

use oxigraph::model::{GraphNameRef, NamedNode, NamedOrBlankNode, Quad, Term, Triple};

use super::common::{count_graph, ProfileViolation, ReasoningError, ReasoningReport};
use super::dl_config::{DlBackendKind, DlConfig};
use super::identity::IdentityPolicy;
use super::owl_mapping::{map_triples, Mapped};
use super::owl_model::Ontology;
use crate::store::TripleStore;

/// What a backend is given: the asserted triples and their OWL 2 reading.
pub struct DlInput {
    pub triples: Vec<Triple>,
    pub ontology: Ontology,
}

/// What a classification + realisation run found.
#[derive(Debug, Clone, Default)]
pub struct DlOutcome {
    /// `Some(false)`: inconsistent — `inconsistency` says why when the
    /// backend can; `None`: unknown.
    pub consistent: Option<bool>,
    pub inconsistency: Option<String>,
    /// Named classes equivalent to `owl:Nothing`.
    pub unsatisfiable: Vec<String>,
    /// Entailed triples. The caller keeps only those about named entities.
    pub inferred: Vec<Triple>,
    pub version: Option<String>,
    pub warnings: Vec<String>,
    /// The backend may have missed entailments (`warnings` says why).
    pub incomplete: bool,
}

/// A yes/no/unknown answer (OWL 2 Conformance §2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Tri {
    True,
    False,
    Unknown,
}

impl Tri {
    pub fn from_bool(b: bool) -> Self {
        if b {
            Tri::True
        } else {
            Tri::False
        }
    }
}

/// What `POST /api/reasoning/check` asks.
#[derive(Debug, Clone)]
pub enum CheckTask {
    Consistency,
    /// Does the input entail every axiom of `conclusion`?
    Entailment {
        conclusion: Vec<Triple>,
    },
    /// Is class `class` satisfiable?
    Satisfiability {
        class: String,
    },
    /// Is the input in OWL 2 DL?
    Profile,
}

impl CheckTask {
    pub fn name(&self) -> &'static str {
        match self {
            CheckTask::Consistency => "consistency",
            CheckTask::Entailment { .. } => "entailment",
            CheckTask::Satisfiability { .. } => "satisfiability",
            CheckTask::Profile => "profile",
        }
    }
}

/// A check's answer.
#[derive(Debug, Clone)]
pub struct CheckOutcome {
    pub result: Tri,
    /// When the input is inconsistent: `(rule, detail)`.
    pub inconsistency: Option<(String, String)>,
    pub violations: Vec<ProfileViolation>,
    pub detail: Option<String>,
    pub version: Option<String>,
    pub warnings: Vec<String>,
}

impl CheckOutcome {
    pub fn of(result: Tri) -> Self {
        CheckOutcome {
            result,
            inconsistency: None,
            violations: Vec::new(),
            detail: None,
            version: None,
            warnings: Vec::new(),
        }
    }
}

/// An external OWL 2 DL reasoner.
pub trait DlBackend: Send + Sync {
    fn name(&self) -> &'static str;
    /// Classify and realise `input`.
    fn reason(&self, input: &DlInput) -> Result<DlOutcome, ReasoningError>;
    /// Answer `task` for `input` (never [`CheckTask::Profile`]: the server
    /// answers that itself).
    fn check(&self, input: &DlInput, task: &CheckTask) -> Result<CheckOutcome, ReasoningError>;
}

/// The external backend `cfg` names (`None` for native / unconfigured).
pub fn external(cfg: &DlConfig) -> Option<Box<dyn DlBackend>> {
    match cfg.backend? {
        DlBackendKind::Native => None,
        DlBackendKind::Konclude => {
            Some(Box::new(super::konclude_bridge::KoncludeBackend::new(cfg)))
        }
        DlBackendKind::Sidecar => Some(Box::new(super::dl_sidecar::SidecarBackend::new(cfg))),
    }
}

/// A materialisation's report plus what the DL layer adds to it.
#[derive(Debug, Clone)]
pub struct DlRun {
    pub report: ReasoningReport,
    pub backend: &'static str,
    pub version: Option<String>,
    /// `false` for the native rules (sound but not complete) and for a backend
    /// that says it may have missed entailments.
    pub complete: bool,
    pub warnings: Vec<String>,
}

/// The graph the native rules write their cardinality obligations
/// (`urn:dl:*`) to: beside the target, never folded into queries.
pub fn diagnostics_graph(target: &str) -> String {
    format!("{target}:diagnostics")
}

const NATIVE_WARNING: &str = "the native backend is OWL 2 RL plus DL-syntax rules: sound but not \
     complete for OWL 2 DL (no existential witnesses, case splits or nominal reasoning)";

fn configured(cfg: &DlConfig) -> Result<DlBackendKind, ReasoningError> {
    cfg.backend.ok_or_else(|| {
        ReasoningError::Unavailable(
            "no OWL 2 DL backend is configured (set OTS_DL_BACKEND to native, konclude or sidecar)"
                .into(),
        )
    })
}

/// The asserted triples a run reads: the `sources` graphs, or the unnamed
/// default graph when unscoped.
pub fn export(
    store: &TripleStore,
    sources: Option<&[String]>,
) -> Result<Vec<Triple>, ReasoningError> {
    let mut out: Vec<Triple> = Vec::new();
    let mut seen: HashSet<Triple> = HashSet::new();
    let mut take = |quads: Vec<Quad>| {
        for q in quads {
            let t = Triple::new(q.subject, q.predicate, q.object);
            if seen.insert(t.clone()) {
                out.push(t);
            }
        }
    };
    match sources {
        Some(graphs) => {
            for g in graphs {
                let n = NamedNode::new(g.as_str())
                    .map_err(|e| ReasoningError::Store(format!("graph <{g}>: {e}")))?;
                take(store.quads_for_graph(GraphNameRef::NamedNode(n.as_ref()))?);
            }
        }
        None => take(store.quads_for_graph(GraphNameRef::DefaultGraph)?),
    }
    Ok(out)
}

/// Map `triples` and check the OWL 2 DL profile; the violations, if any.
pub fn profile(triples: &[Triple]) -> (Mapped, Vec<ProfileViolation>) {
    let mapped = map_triples(triples);
    let mut v: Vec<ProfileViolation> = mapped
        .unmapped
        .iter()
        .take(50)
        .map(|t| ProfileViolation::new("unmapped-triple", format!("{t} has no OWL 2 reading")))
        .collect();
    if mapped.unmapped.len() > 50 {
        v.push(ProfileViolation::new(
            "unmapped-triple",
            format!(
                "{} further triple(s) with no OWL 2 reading",
                mapped.unmapped.len() - 50
            ),
        ));
    }
    v.extend(super::owl_profile::check(&mapped.ontology));
    (mapped, v)
}

fn prepare(
    cfg: &DlConfig,
    kind: DlBackendKind,
    triples: Vec<Triple>,
    identity: IdentityPolicy,
) -> Result<(DlInput, Vec<String>), ReasoningError> {
    if kind != DlBackendKind::Native {
        if !identity.propagates_same_as() {
            return Err(ReasoningError::NotSupported(format!(
                "identity policy `{}` cannot be honoured by the {} backend: OWL 2 DL reasoning \
                 always treats owl:sameAs as equality",
                identity.as_str(),
                kind.as_str()
            )));
        }
        if triples.len() > cfg.max_triples {
            return Err(ReasoningError::TooLarge {
                backend: kind.as_str().to_string(),
                triples: triples.len(),
                limit: cfg.max_triples,
            });
        }
    }
    let (mapped, violations) = profile(&triples);
    if !violations.is_empty() {
        return Err(ReasoningError::NotInProfile { violations });
    }
    Ok((
        DlInput {
            triples,
            ontology: mapped.ontology,
        },
        mapped.warnings,
    ))
}

/// Whether `t` is about named entities only (no blank node anywhere).
pub fn is_named(t: &Triple) -> bool {
    matches!(t.subject, NamedOrBlankNode::NamedNode(_))
        && matches!(t.object, Term::NamedNode(_) | Term::Literal(_))
}

/// Run the `owl2-dl` regime over `sources` (None: the default graph) into
/// `target`.
pub fn materialize(
    store: &TripleStore,
    cfg: &DlConfig,
    sources: Option<&[String]>,
    target: &str,
    identity: IdentityPolicy,
) -> Result<DlRun, ReasoningError> {
    let start = Instant::now();
    let kind = configured(cfg)?;
    let triples = export(store, sources)?;
    let n_input = triples.len();
    let (input, mut warnings) = prepare(cfg, kind, triples, identity)?;
    let initial = count_graph(store, target)?;
    let diagnostics = diagnostics_graph(target);
    if diagnostics.starts_with("urn:entailment:") {
        store.update(&format!("CLEAR SILENT GRAPH <{diagnostics}>"))?;
    }

    if kind == DlBackendKind::Native {
        let r = super::owl2_dl::Owl2DLReasoner::new(store)
            .with_target(target)
            .with_diagnostics(diagnostics)
            .with_identity_policy(identity);
        let r = match sources {
            Some(s) => r.with_sources(s.to_vec()),
            None => r,
        };
        let report = r.materialize()?;
        warnings.push(NATIVE_WARNING.into());
        return Ok(DlRun {
            report,
            backend: "native",
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
            complete: false,
            warnings,
        });
    }

    let backend = external(cfg).expect("an external backend kind");
    let outcome = backend.reason(&input)?;
    warnings.extend(outcome.warnings.iter().cloned());
    if outcome.consistent == Some(false) {
        return Err(ReasoningError::inconsistency(
            "external-reasoner",
            outcome.inconsistency.unwrap_or_else(|| {
                format!(
                    "the {} backend found the ontology inconsistent",
                    backend.name()
                )
            }),
        ));
    }
    if outcome.consistent.is_none() {
        warnings.push(format!(
            "the {} backend could not decide consistency",
            backend.name()
        ));
    }
    write_inferred(store, &input.triples, outcome.inferred, target)?;
    let total = count_graph(store, target)?;
    let _ = n_input;
    Ok(DlRun {
        report: ReasoningReport {
            regime: "owl2-dl".into(),
            triples_added: total.saturating_sub(initial),
            iterations: 1,
            elapsed_ms: start.elapsed().as_millis() as u64,
            target_graph: target.to_string(),
        },
        backend: backend.name(),
        version: outcome.version,
        complete: !outcome.incomplete,
        warnings,
    })
}

/// Insert the named-entity triples of `inferred` that are not asserted.
fn write_inferred(
    store: &TripleStore,
    asserted: &[Triple],
    inferred: Vec<Triple>,
    target: &str,
) -> Result<(), ReasoningError> {
    let asserted: HashSet<&Triple> = asserted.iter().collect();
    let g =
        NamedNode::new(target).map_err(|e| ReasoningError::Store(format!("<{target}>: {e}")))?;
    let mut seen = HashSet::new();
    let quads: Vec<Quad> = inferred
        .into_iter()
        .filter(is_named)
        .filter(|t| !asserted.contains(t))
        .filter(|t| seen.insert(t.clone()))
        .map(|t| Quad::new(t.subject, t.predicate, t.object, g.clone()))
        .collect();
    if !quads.is_empty() {
        store.insert_quads(quads)?;
    }
    Ok(())
}

/// Answer `task` over `premise` (or, without one, the `sources` graphs /
/// the default graph).
pub fn check(
    store: &TripleStore,
    cfg: &DlConfig,
    sources: Option<&[String]>,
    premise: Option<Vec<Triple>>,
    task: &CheckTask,
    identity: IdentityPolicy,
) -> Result<(CheckOutcome, &'static str, bool), ReasoningError> {
    let triples = match premise {
        Some(t) => t,
        None => export(store, sources)?,
    };
    if let CheckTask::Profile = task {
        // The server's own check: no backend needed.
        let (mapped, violations) = profile(&triples);
        let mut o = CheckOutcome::of(Tri::from_bool(violations.is_empty()));
        o.violations = violations;
        o.warnings = mapped.warnings;
        return Ok((o, "server", true));
    }
    let kind = configured(cfg)?;
    let (input, warnings) = prepare(cfg, kind, triples, identity)?;
    if kind == DlBackendKind::Native {
        let mut o = native_check(&input, task, identity)?;
        o.warnings.extend(warnings);
        o.warnings.push(NATIVE_WARNING.into());
        return Ok((o, "native", false));
    }
    let backend = external(cfg).expect("an external backend kind");
    let mut o = backend.check(&input, task)?;
    o.warnings.extend(warnings);
    Ok((o, backend.name(), true))
}

const PROBE: &str = "urn:ots:dl:probe";

/// The native rules over a scratch store: `false` answers are sound; a run
/// that finds nothing proves nothing, so the answer is then `unknown`.
fn native_check(
    input: &DlInput,
    task: &CheckTask,
    identity: IdentityPolicy,
) -> Result<CheckOutcome, ReasoningError> {
    let scratch = TripleStore::in_memory().map_err(|e| ReasoningError::Store(e.to_string()))?;
    let mut quads: Vec<Quad> = input
        .triples
        .iter()
        .map(|t| {
            Quad::new(
                t.subject.clone(),
                t.predicate.clone(),
                t.object.clone(),
                GraphNameRef::DefaultGraph,
            )
        })
        .collect();
    if let CheckTask::Satisfiability { class } = task {
        let c = NamedNode::new(class.as_str())
            .map_err(|e| ReasoningError::Query(format!("class <{class}>: {e}")))?;
        quads.push(Quad::new(
            NamedNode::new_unchecked(PROBE),
            NamedNode::new_unchecked("http://www.w3.org/1999/02/22-rdf-syntax-ns#type"),
            c,
            GraphNameRef::DefaultGraph,
        ));
    }
    scratch.insert_quads(quads)?;
    let tg = super::common::OWL2_DL_ENTAILMENT_GRAPH;
    let run = super::owl2_dl::Owl2DLReasoner::new(&scratch)
        .with_target(tg)
        .with_identity_policy(identity)
        .materialize();
    let inconsistency = match run {
        Ok(_) => None,
        Err(ReasoningError::Inconsistency { rule, detail }) => Some((rule, detail)),
        Err(e) => return Err(e),
    };
    let mut o = match (task, &inconsistency) {
        (CheckTask::Consistency, Some(_)) => CheckOutcome::of(Tri::False),
        (CheckTask::Consistency, None) => CheckOutcome::of(Tri::Unknown),
        // Everything follows from an inconsistent premise.
        (CheckTask::Entailment { .. }, Some(_)) => CheckOutcome::of(Tri::True),
        (CheckTask::Entailment { conclusion }, None) => {
            let found = ask_closure(&scratch, tg, conclusion)?;
            CheckOutcome::of(if found { Tri::True } else { Tri::Unknown })
        }
        (CheckTask::Satisfiability { .. }, Some(_)) => CheckOutcome::of(Tri::False),
        (CheckTask::Satisfiability { .. }, None) => CheckOutcome::of(Tri::Unknown),
        (CheckTask::Profile, _) => unreachable!("answered by the server"),
    };
    if matches!(task, CheckTask::Consistency) {
        o.inconsistency = inconsistency;
    }
    Ok(o)
}

/// Whether the conclusion graph matches the premise plus its closure, blank
/// nodes read as existential variables (simple entailment over the closure).
fn ask_closure(
    store: &TripleStore,
    tg: &str,
    conclusion: &[Triple],
) -> Result<bool, ReasoningError> {
    if conclusion.is_empty() {
        return Ok(true);
    }
    let term = |t: &Term| match t {
        Term::BlankNode(b) => format!("?b_{}", b.as_str()),
        other => other.to_string(),
    };
    let mut bgp = String::new();
    for t in conclusion {
        let s = match &t.subject {
            NamedOrBlankNode::BlankNode(b) => format!("?b_{}", b.as_str()),
            NamedOrBlankNode::NamedNode(n) => n.to_string(),
        };
        bgp.push_str(&format!("{s} {} {} . ", t.predicate, term(&t.object)));
    }
    match store.query_over(&format!("ASK {{ {bgp} }}"), &[tg.to_string()])? {
        oxigraph::sparql::QueryResults::Boolean(b) => Ok(b),
        _ => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxigraph::io::{RdfFormat, RdfParser};

    fn triples(ttl: &str) -> Vec<Triple> {
        RdfParser::from_format(RdfFormat::Turtle)
            .for_slice(ttl.as_bytes())
            .map(|q| q.unwrap().into())
            .collect()
    }

    #[test]
    fn blank_node_triples_are_not_named() {
        let ts =
            triples("<http://e/a> <http://e/p> _:x . <http://e/a> <http://e/p> <http://e/b> .");
        assert_eq!(ts.iter().filter(|t| is_named(t)).count(), 1);
    }

    #[test]
    fn unconfigured_backend_is_unavailable() {
        let store = TripleStore::in_memory().unwrap();
        let cfg = DlConfig::default();
        let r = materialize(
            &store,
            &cfg,
            None,
            "urn:entailment:owl2-dl",
            IdentityPolicy::Full,
        );
        assert!(matches!(r, Err(ReasoningError::Unavailable(_))), "{r:?}");
    }
}
