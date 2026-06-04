//! Mint `owl:versionIRI` for a data-model version.
//!
//! Pattern: `{namespace}/{version}` written into the version's named graph,
//! attached to the `owl:Ontology` subject. Falls back to the namespace IRI
//! itself as subject when the graph contains no `owl:Ontology` declaration.

use oxigraph::model::{NamedNodeRef, Term};
use oxigraph::sparql::QueryResults;

use crate::store::engine::StoreError;
use crate::store::TripleStore;

const OWL_VERSION_IRI: &str = "http://www.w3.org/2002/07/owl#versionIRI";
const OWL_PRIOR_VERSION: &str = "http://www.w3.org/2002/07/owl#priorVersion";

/// Build a version IRI by concatenating namespace and version with a single `/`.
pub fn build_version_iri(namespace: &str, version: &str) -> String {
    let ns = namespace.trim_end_matches('/').trim_end_matches('#');
    format!("{ns}/{version}")
}

/// Locate the `owl:Ontology` subject inside a named graph; return its IRI.
fn find_ontology_subject(store: &TripleStore, graph_iri: &str) -> Option<String> {
    let q = format!(
        r#"
        SELECT ?s WHERE {{
          GRAPH <{graph_iri}> {{
            ?s a <http://www.w3.org/2002/07/owl#Ontology> .
          }}
        }}
        LIMIT 1
        "#
    );
    let QueryResults::Solutions(sols) = store.query(&q).ok()? else { return None };
    for row in sols.flatten() {
        if let Some(Some(Term::NamedNode(nn))) = row.values().first() {
            return Some(nn.as_str().to_string());
        }
    }
    None
}

/// Mint `owl:versionIRI` (and optionally `owl:priorVersion`) into the version graph.
pub fn mint(
    store: &TripleStore,
    graph_iri: &str,
    namespace: &str,
    version: &str,
    prior_version_iri: Option<&str>,
) -> Result<String, StoreError> {
    let subject = find_ontology_subject(store, graph_iri).unwrap_or_else(|| {
        namespace.trim_end_matches('/').trim_end_matches('#').to_string()
    });
    let version_iri = build_version_iri(namespace, version);

    // Validate IRIs before constructing the SPARQL update.
    NamedNodeRef::new(&subject)
        .map_err(|e| StoreError::Parse(format!("Invalid ontology subject IRI '{subject}': {e}")))?;
    NamedNodeRef::new(&version_iri)
        .map_err(|e| StoreError::Parse(format!("Invalid version IRI '{version_iri}': {e}")))?;

    // Replace any existing versionIRI for this subject in this graph.
    let del = format!(
        r#"
        DELETE WHERE {{
          GRAPH <{graph_iri}> {{ <{subject}> <{OWL_VERSION_IRI}> ?old }}
        }}
        "#
    );
    store.update(&del)?;

    let prior_triple = match prior_version_iri {
        Some(prior) if NamedNodeRef::new(prior).is_ok() => format!(
            "<{subject}> <{OWL_PRIOR_VERSION}> <{prior}> ."
        ),
        _ => String::new(),
    };

    let ins = format!(
        r#"
        INSERT DATA {{
          GRAPH <{graph_iri}> {{
            <{subject}> <{OWL_VERSION_IRI}> <{version_iri}> .
            {prior_triple}
          }}
        }}
        "#
    );
    store.update(&ins)?;
    Ok(version_iri)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_iri_strips_trailing_slash() {
        assert_eq!(build_version_iri("https://ex.org/ont/", "1.2.0"), "https://ex.org/ont/1.2.0");
        assert_eq!(build_version_iri("https://ex.org/ont", "1.2.0"), "https://ex.org/ont/1.2.0");
        assert_eq!(build_version_iri("https://ex.org/ont#", "1.2.0"), "https://ex.org/ont/1.2.0");
    }
}
