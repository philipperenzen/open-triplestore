//! Shared types for all reasoning engines.

use thiserror::Error;

/// Describes the outcome of a successful materialization run.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ReasoningReport {
    /// The entailment regime that was applied (e.g. `"rdfs"`, `"owl2-rl"`).
    pub regime: String,
    /// Number of new triples written to the target graph.
    pub triples_added: usize,
    /// Number of fixed-point iterations executed.
    pub iterations: usize,
    /// Wall-clock time in milliseconds.
    pub elapsed_ms: u64,
    /// IRI of the named graph that received the entailed triples.
    pub target_graph: String,
    /// How many axioms outside the regime's profile the run did not use
    /// (reported by OWL 2 QL; omitted when zero).
    #[serde(skip_serializing_if = "is_zero")]
    pub ignored_axioms: usize,
    /// The first few of those axioms (at most [`IGNORED_SAMPLE`]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ignored_sample: Vec<IgnoredAxiom>,
    /// Axioms the regime read but could not use: constructs outside its
    /// profile, by construct. Empty (and not serialized) for a regime that
    /// does not report them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignored: Vec<IgnoredAxioms>,
}

/// Axioms of one construct that a reasoning run left out.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IgnoredAxioms {
    /// The OWL 2 construct, e.g. `ObjectUnionOf` or `FunctionalObjectProperty`.
    pub construct: String,
    /// How many axioms or expressions used it.
    pub count: usize,
    /// One of them: the subject term of the first one read.
    pub example: String,
}

/// How many ignored axioms a [`ReasoningReport`] lists by name.
pub const IGNORED_SAMPLE: usize = 20;

/// An axiom a reasoner read but did not use because it lies outside the
/// regime's profile (an `owl:TransitiveProperty` under OWL 2 QL, say).
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
pub struct IgnoredAxiom {
    /// The construct, as a prefixed name (`owl:TransitiveProperty`,
    /// `rdfs:subClassOf`).
    pub axiom: String,
    /// The axiom's subject: an IRI, or `_:` and a blank-node label.
    pub subject: String,
    /// Why it was not used.
    pub reason: String,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// Errors that can occur during reasoning.
// Variants are used by feature-gated reasoning engines; when those features
// are not enabled the compiler would otherwise warn about dead variants.
#[cfg_attr(not(any(feature = "owl2-rl", feature = "owl2-dl")), allow(dead_code))]
#[derive(Debug, Error)]
pub enum ReasoningError {
    #[error("Store error: {0}")]
    Store(String),
    #[error("Query error: {0}")]
    Query(String),
    /// The input entails `false`. `rule` names the check that fired (an
    /// OWL 2 RL/RDF rule such as `cax-dw`, or the DL check); the consequences
    /// derived before the check stay in the target graph.
    #[error("Inconsistency detected ({rule}): {detail}")]
    Inconsistency { rule: String, detail: String },
    /// The fixed point was not reached within the iteration limit, so the
    /// target graph holds only part of the closure.
    #[error("{regime} materialisation did not reach a fixed point within {iterations} iterations")]
    NotConverged { regime: String, iterations: usize },
    /// The request cannot be served as asked (e.g. `sameas-off` with an
    /// external DL backend, which cannot switch equality off).
    #[error("Not supported: {0}")]
    NotSupported(String),
    /// No OWL 2 DL backend is configured, or the configured one cannot be
    /// reached. Never answered by falling back to another backend.
    #[error("OWL 2 DL backend unavailable: {0}")]
    Unavailable(String),
    /// The backend did not answer in time: the result is unknown.
    #[error("{backend} did not answer within {seconds} s")]
    Timeout { backend: String, seconds: u64 },
    /// The input exceeds the backend's size cap (`OTS_REASONER_MAX_TRIPLES`).
    #[error("the input has {triples} triples; the {backend} backend accepts at most {limit}")]
    TooLarge {
        backend: String,
        triples: usize,
        limit: usize,
    },
    /// The input is not an OWL 2 DL ontology.
    #[error("the input is not in OWL 2 DL ({} violation(s))", violations.len())]
    NotInProfile { violations: Vec<ProfileViolation> },
    /// The backend ran and failed (crash, unreadable answer).
    #[error("{backend} failed: {detail}")]
    Backend { backend: String, detail: String },
}

/// One reason an input is not in OWL 2 DL: a typing constraint, a global
/// restriction of the Structural Specification (§11), or a triple with no
/// OWL 2 reading (`unmapped-triple`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProfileViolation {
    pub rule: String,
    pub detail: String,
}

impl ProfileViolation {
    pub fn new(rule: &str, detail: impl Into<String>) -> Self {
        ProfileViolation {
            rule: rule.to_string(),
            detail: detail.into(),
        }
    }
}

/// Whether `regime` checks consistency, i.e. can fail with
/// [`ReasoningError::Inconsistency`]. A run of any other regime says nothing
/// about consistency either way.
pub fn checks_consistency(regime: &str) -> bool {
    matches!(regime, "owl2-rl" | "owl2-el" | "owl2-ql" | "owl2-dl")
}

impl ReasoningError {
    /// An [`Inconsistency`](Self::Inconsistency) found by `rule`.
    pub fn inconsistency(rule: &str, detail: impl Into<String>) -> Self {
        ReasoningError::Inconsistency {
            rule: rule.to_string(),
            detail: detail.into(),
        }
    }
}

impl From<crate::store::StoreError> for ReasoningError {
    fn from(e: crate::store::StoreError) -> Self {
        ReasoningError::Store(e.to_string())
    }
}

// ─── Well-known entailment graph IRIs ─────────────────────────────────────────

/// Default named graph IRI for RDFS-entailed triples.
pub const RDFS_ENTAILMENT_GRAPH: &str = "urn:entailment:rdfs";
/// Default named graph IRI for OWL 2 RL-entailed triples.
pub const OWL2_RL_ENTAILMENT_GRAPH: &str = "urn:entailment:owl2-rl";
/// Default named graph IRI for OWL 2 EL-entailed triples.
pub const OWL2_EL_ENTAILMENT_GRAPH: &str = "urn:entailment:owl2-el";
/// Default named graph IRI for OWL 2 QL-rewriting artifacts.
pub const OWL2_QL_ENTAILMENT_GRAPH: &str = "urn:entailment:owl2-ql";
/// Default named graph IRI for OWL 2 DL-entailed triples.
pub const OWL2_DL_ENTAILMENT_GRAPH: &str = "urn:entailment:owl2-dl";

// ─── SPARQL helpers ───────────────────────────────────────────────────────────

/// Count the number of triples in a named graph.
pub fn count_graph(
    store: &crate::store::TripleStore,
    graph: &str,
) -> Result<usize, ReasoningError> {
    let query = format!("SELECT (COUNT(*) AS ?c) WHERE {{ GRAPH <{graph}> {{ ?s ?p ?o }} }}");
    match store.query(&query)? {
        oxigraph::sparql::QueryResults::Solutions(mut sols) => {
            let count = sols
                .next()
                .and_then(|r| r.ok())
                .and_then(|s| {
                    s.get("c").and_then(|v| {
                        // Numeric literal value extraction
                        match v {
                            oxigraph::model::Term::Literal(lit) => {
                                lit.value().parse::<usize>().ok()
                            }
                            _ => None,
                        }
                    })
                })
                .unwrap_or(0);
            Ok(count)
        }
        _ => Ok(0),
    }
}
