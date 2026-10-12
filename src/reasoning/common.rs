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
    /// Axioms the regime read but could not use: constructs outside its
    /// profile, by construct (decision D9). Reported by OWL 2 EL and OWL 2
    /// QL; empty, and not serialized, when nothing was left out and for the
    /// regimes that use every triple (RDFS, OWL 2 RL/RDF, SKOS) or refuse
    /// input outside their profile (OWL 2 DL).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignored: Vec<IgnoredAxioms>,
}

/// Axioms of one construct that a reasoning run left out.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IgnoredAxioms {
    /// The construct outside the profile, named as in the OWL 2 Structural
    /// Specification where one name fits (`ObjectUnionOf`,
    /// `TransitiveObjectProperty`, `SameIndividual`), otherwise described
    /// (`cardinality restriction`, `datatype outside the OWL 2 QL datatype
    /// map`).
    pub construct: String,
    /// How many axioms or expressions used it.
    pub count: usize,
    /// One of them, the first one read: an IRI, or `_:` and a blank-node
    /// label.
    pub example: String,
}

impl IgnoredAxioms {
    /// The report rows for `construct → (count, example)`, the most frequent
    /// construct first (ties by name), so a report reads the same each run.
    pub fn rows<'a>(
        by_construct: impl IntoIterator<Item = (&'a str, &'a (usize, String))>,
    ) -> Vec<IgnoredAxioms> {
        let mut rows: Vec<IgnoredAxioms> = by_construct
            .into_iter()
            .map(|(construct, (count, example))| IgnoredAxioms {
                construct: construct.to_string(),
                count: *count,
                example: example.clone(),
            })
            .collect();
        rows.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then_with(|| a.construct.cmp(&b.construct))
        });
        rows
    }
}

/// How a term reads as the `example` of an [`IgnoredAxioms`] row: an IRI
/// bare, anything else in N-Triples form (`_:b0`).
pub fn example_label(t: &oxigraph::model::Term) -> String {
    match t {
        oxigraph::model::Term::NamedNode(n) => n.as_str().to_string(),
        other => other.to_string(),
    }
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
    // RDFS checks datatype clashes (RDF 1.1 Semantics, recognized datatypes).
    matches!(
        regime,
        "rdfs" | "owl2-rl" | "owl2-el" | "owl2-ql" | "owl2-dl"
    )
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
