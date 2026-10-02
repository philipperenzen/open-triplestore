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

/// `(class IRI, key property IRIs)` for every `owl:hasKey` list in `scope`
/// (the default graph when `None`), read from the quad index — the
/// `rdf:rest*` walk is done here rather than as a SPARQL property path.
/// Blank-node class expressions are skipped; the order of a key's properties
/// does not matter. Shared by the RL (`prp-key`) and EL hasKey rules.
#[cfg(any(feature = "owl2-rl", feature = "owl2-el"))]
pub fn has_keys(
    store: &crate::store::TripleStore,
    scope: Option<&[String]>,
) -> Result<Vec<(String, Vec<String>)>, ReasoningError> {
    use oxigraph::model::{GraphNameRef, NamedNodeRef, NamedOrBlankNode, Term};
    const OWL_HAS_KEY: &str = "http://www.w3.org/2002/07/owl#hasKey";
    const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
    const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
    const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
    let graphs: Vec<Option<&str>> = match scope {
        Some(scope) => scope.iter().map(|g| Some(g.as_str())).collect(),
        None => vec![None],
    };
    let has_key = NamedNodeRef::new_unchecked(OWL_HAS_KEY);
    let first = NamedNodeRef::new_unchecked(RDF_FIRST);
    let rest = NamedNodeRef::new_unchecked(RDF_REST);
    let mut keys: Vec<(String, Vec<String>)> = Vec::new();
    for graph in graphs {
        let graph_ref = match graph {
            Some(g) => match NamedNodeRef::new(g) {
                Ok(nn) => GraphNameRef::NamedNode(nn),
                Err(_) => continue,
            },
            None => GraphNameRef::DefaultGraph,
        };
        let object_of = |subject: &NamedOrBlankNode, pred: NamedNodeRef<'_>| {
            store
                .store()
                .quads_for_pattern(Some(subject.as_ref()), Some(pred), None, Some(graph_ref))
                .next()
                .and_then(|q| q.ok())
                .map(|q| q.object)
        };
        for quad in store
            .store()
            .quads_for_pattern(None, Some(has_key), None, Some(graph_ref))
        {
            let quad = quad.map_err(|e| ReasoningError::Store(e.to_string()))?;
            let NamedOrBlankNode::NamedNode(class) = quad.subject else {
                continue;
            };
            let mut props: Vec<String> = Vec::new();
            let mut cell = match quad.object {
                Term::NamedNode(n) => NamedOrBlankNode::NamedNode(n),
                Term::BlankNode(b) => NamedOrBlankNode::BlankNode(b),
                _ => continue,
            };
            // Walk the list; a malformed list simply ends.
            for _ in 0..64 {
                if let NamedOrBlankNode::NamedNode(n) = &cell {
                    if n.as_str() == RDF_NIL {
                        break;
                    }
                }
                if let Some(Term::NamedNode(p)) = object_of(&cell, first) {
                    props.push(p.as_str().to_string());
                }
                cell = match object_of(&cell, rest) {
                    Some(Term::NamedNode(n)) => NamedOrBlankNode::NamedNode(n),
                    Some(Term::BlankNode(b)) => NamedOrBlankNode::BlankNode(b),
                    _ => break,
                };
            }
            props.sort();
            props.dedup();
            if !props.is_empty() {
                keys.push((class.as_str().to_string(), props));
            }
        }
    }
    keys.sort();
    keys.dedup();
    Ok(keys)
}
