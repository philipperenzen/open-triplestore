//! Shared types for all reasoning engines.

use thiserror::Error;

/// Describes the outcome of a successful materialization run.
#[derive(Debug, Clone, serde::Serialize)]
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
    #[error("Inconsistency detected: {0}")]
    Inconsistency(String),
    #[error("Not supported: {0}")]
    NotSupported(String),
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
