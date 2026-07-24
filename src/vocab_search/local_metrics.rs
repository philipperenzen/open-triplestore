//! Local usage metrics — how often vocabulary terms are actually used in this
//! instance's data.
//!
//! Complements the LOV LOD-corpus metrics with an instance-local signal: a
//! predicate used by thousands of local triples, or a class with many local
//! instances, ranks above an equally-matching term nobody here uses.  Counts
//! are aggregated per named graph; only graphs in the caller-supplied
//! **publicly accessible** set are counted (the ranking signal feeds
//! anonymous search, so private-dataset usage must not influence it), and
//! system graphs (the model registry and version graphs, `urn:` graphs) are
//! excluded so vocabulary *definitions* don't count as vocabulary *usage*.
//!
//! Disable with `VOCAB_LOCAL_METRICS=off` (e.g. for very large stores).

use std::collections::HashMap;

use oxigraph::sparql::QueryResults;
use tracing::{debug, warn};

use crate::store::TripleStore;

/// Graph-IRI prefixes whose triples are definitions, not usage.
fn is_system_graph(graph: &str, base_url: &str) -> bool {
    graph.starts_with("urn:") || graph.starts_with(&format!("{base_url}/data-model/"))
}

pub fn enabled() -> bool {
    !matches!(
        std::env::var("VOCAB_LOCAL_METRICS")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "off" | "false" | "0" | "no"
    )
}

/// Count term usage across the given **publicly accessible** data graphs:
/// predicates by triple count, classes by instance count.  Returns IRI → count.
pub fn compute_local_usage(
    store: &TripleStore,
    base_url: &str,
    public_graphs: &std::collections::HashSet<String>,
) -> HashMap<String, u64> {
    let mut usage: HashMap<String, u64> = HashMap::new();
    if !enabled() || public_graphs.is_empty() {
        return usage;
    }

    let queries = [
        // Predicate usage per graph.
        "SELECT ?g ?term (COUNT(*) AS ?c) WHERE { GRAPH ?g { ?s ?term ?o } } GROUP BY ?g ?term",
        // Class usage (typed instances) per graph.
        "SELECT ?g ?term (COUNT(*) AS ?c) WHERE { GRAPH ?g { ?s a ?term } } GROUP BY ?g ?term",
    ];

    for query in queries {
        let results = match store.query(query) {
            Ok(r) => r,
            Err(e) => {
                warn!("local vocab-usage query failed: {e}");
                continue;
            }
        };
        if let QueryResults::Solutions(solutions) = results {
            for sol in solutions.flatten() {
                let graph = match sol.get("g") {
                    Some(oxigraph::model::Term::NamedNode(nn)) => nn.as_str(),
                    _ => continue,
                };
                if is_system_graph(graph, base_url) || !public_graphs.contains(graph) {
                    continue;
                }
                let term = match sol.get("term") {
                    Some(oxigraph::model::Term::NamedNode(nn)) => nn.as_str().to_string(),
                    _ => continue,
                };
                let count = match sol.get("c") {
                    Some(oxigraph::model::Term::Literal(lit)) => {
                        lit.value().parse::<u64>().unwrap_or(0)
                    }
                    _ => 0,
                };
                if count > 0 {
                    *usage.entry(term).or_default() += count;
                }
            }
        }
    }
    debug!("local vocab usage: {} distinct terms", usage.len());
    usage
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_graphs_excluded() {
        let base = "http://localhost:7878";
        assert!(is_system_graph("urn:system:data-model-registry", base));
        assert!(is_system_graph(
            "http://localhost:7878/data-model/foaf/version/0.99",
            base
        ));
        assert!(!is_system_graph(
            "https://opentriplestore.org/demo/bridges/instances",
            base
        ));
    }

    #[test]
    fn counts_usage_not_definitions() {
        let store = TripleStore::in_memory().unwrap();
        let base = "http://localhost:7878";
        const RDF_TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
        store
            .load_str(
                &format!(
                    r#"<http://ex.org/a> <http://xmlns.com/foaf/0.1/name> "A" .
<http://ex.org/a> {RDF_TYPE} <http://xmlns.com/foaf/0.1/Person> .
<http://ex.org/b> {RDF_TYPE} <http://xmlns.com/foaf/0.1/Person> ."#
                ),
                oxigraph::io::RdfFormat::NTriples,
                Some("http://ex.org/graph/data"),
            )
            .unwrap();
        // Definitions inside a version graph must not count.
        store
            .load_str(
                &format!(
                    "<http://xmlns.com/foaf/0.1/name> {RDF_TYPE} <http://www.w3.org/2002/07/owl#DatatypeProperty> ."
                ),
                oxigraph::io::RdfFormat::NTriples,
                Some("http://localhost:7878/data-model/foaf/version/0.99"),
            )
            .unwrap();

        let public: std::collections::HashSet<String> = ["http://ex.org/graph/data".to_string()]
            .into_iter()
            .collect();
        let usage = compute_local_usage(&store, base, &public);
        assert_eq!(usage.get("http://xmlns.com/foaf/0.1/name"), Some(&1));
        assert_eq!(usage.get("http://xmlns.com/foaf/0.1/Person"), Some(&2));
        assert!(!usage.contains_key("http://www.w3.org/2002/07/owl#DatatypeProperty"));

        // A graph outside the public set contributes nothing.
        let empty = std::collections::HashSet::new();
        assert!(compute_local_usage(&store, base, &empty).is_empty());
    }
}
