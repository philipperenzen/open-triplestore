//! SKOS / DCAT-aware version metadata for a published vocabulary.
//!
//! On publish, attach `dcat:hasVersion`, `dcterms:issued`, `dcterms:modified`,
//! `pav:version`, and (when applicable) `dcterms:isReplacedBy` to the
//! `skos:ConceptScheme` subject inside the version's named graph. Falls back
//! to the namespace IRI itself as subject when the graph contains no
//! ConceptScheme declaration.

use oxigraph::model::{NamedNodeRef, Term};
use oxigraph::sparql::QueryResults;

use crate::store::engine::StoreError;
use crate::store::TripleStore;

const SKOS_CONCEPT_SCHEME: &str = "http://www.w3.org/2004/02/skos/core#ConceptScheme";
const DCAT_HAS_VERSION: &str = "http://www.w3.org/ns/dcat#hasVersion";
const DCT_ISSUED: &str = "http://purl.org/dc/terms/issued";
const DCT_MODIFIED: &str = "http://purl.org/dc/terms/modified";
const DCT_IS_REPLACED_BY: &str = "http://purl.org/dc/terms/isReplacedBy";
const PAV_VERSION: &str = "http://purl.org/pav/version";
const XSD_DATE_TIME: &str = "http://www.w3.org/2001/XMLSchema#dateTime";

/// Build the canonical version IRI for a vocabulary version.
pub fn build_version_iri(namespace: &str, version: &str) -> String {
    let ns = namespace.trim_end_matches('/').trim_end_matches('#');
    format!("{ns}/{version}")
}

fn find_scheme_subject(store: &TripleStore, graph_iri: &str) -> Option<String> {
    let q = format!(
        r#"
        SELECT ?s WHERE {{
          GRAPH <{graph_iri}> {{
            ?s a <{SKOS_CONCEPT_SCHEME}> .
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

/// Stamp DCAT/PAV/SKOS version metadata onto the ConceptScheme.
pub fn stamp(
    store: &TripleStore,
    graph_iri: &str,
    namespace: &str,
    version: &str,
    issued_at: &str,
    replaces_version_iri: Option<&str>,
) -> Result<String, StoreError> {
    let subject = find_scheme_subject(store, graph_iri).unwrap_or_else(|| {
        namespace.trim_end_matches('/').trim_end_matches('#').to_string()
    });
    let version_iri = build_version_iri(namespace, version);

    NamedNodeRef::new(&subject)
        .map_err(|e| StoreError::Parse(format!("Invalid scheme subject IRI '{subject}': {e}")))?;
    NamedNodeRef::new(&version_iri)
        .map_err(|e| StoreError::Parse(format!("Invalid version IRI '{version_iri}': {e}")))?;

    // Replace any prior version metadata on the scheme inside this graph.
    let del = format!(
        r#"
        DELETE WHERE {{
          GRAPH <{graph_iri}> {{
            <{subject}> <{DCAT_HAS_VERSION}> ?old1 .
          }}
        }};
        DELETE WHERE {{
          GRAPH <{graph_iri}> {{
            <{subject}> <{PAV_VERSION}> ?old2 .
          }}
        }};
        DELETE WHERE {{
          GRAPH <{graph_iri}> {{
            <{subject}> <{DCT_ISSUED}> ?old3 .
          }}
        }};
        DELETE WHERE {{
          GRAPH <{graph_iri}> {{
            <{subject}> <{DCT_MODIFIED}> ?old4 .
          }}
        }}
        "#
    );
    store.update(&del)?;

    let replaces_triple = match replaces_version_iri {
        Some(r) if NamedNodeRef::new(r).is_ok() => format!(
            "<{subject}> <{DCT_IS_REPLACED_BY}> <{r}> ."
        ),
        _ => String::new(),
    };

    let ins = format!(
        r#"
        INSERT DATA {{
          GRAPH <{graph_iri}> {{
            <{subject}> <{DCAT_HAS_VERSION}> <{version_iri}> .
            <{subject}> <{PAV_VERSION}> "{version}" .
            <{subject}> <{DCT_ISSUED}> "{issued_at}"^^<{XSD_DATE_TIME}> .
            <{subject}> <{DCT_MODIFIED}> "{issued_at}"^^<{XSD_DATE_TIME}> .
            {replaces_triple}
          }}
        }}
        "#
    );
    store.update(&ins)?;
    Ok(version_iri)
}

/// Mark a list of concept IRIs as deprecated within a graph by adding
/// `owl:deprecated true` and a `skos:historyNote`.
#[allow(dead_code)]
pub fn mark_concepts_deprecated(
    store: &TripleStore,
    graph_iri: &str,
    concept_iris: &[String],
    note: &str,
) -> Result<(), StoreError> {
    if concept_iris.is_empty() {
        return Ok(());
    }
    let escaped = note.replace('\\', "\\\\").replace('"', "\\\"");
    let mut triples = String::new();
    for iri in concept_iris {
        if NamedNodeRef::new(iri).is_err() {
            continue;
        }
        triples.push_str(&format!(
            "<{iri}> <http://www.w3.org/2002/07/owl#deprecated> \"true\"^^<http://www.w3.org/2001/XMLSchema#boolean> .\n\
             <{iri}> <http://www.w3.org/2004/02/skos/core#historyNote> \"{escaped}\"@en .\n"
        ));
    }
    let q = format!(
        "INSERT DATA {{ GRAPH <{graph_iri}> {{ {triples} }} }}"
    );
    store.update(&q)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_iri_format() {
        assert_eq!(build_version_iri("https://ex.org/vocab/", "2.0.0"), "https://ex.org/vocab/2.0.0");
        assert_eq!(build_version_iri("https://ex.org/vocab#", "2.0.0"), "https://ex.org/vocab/2.0.0");
    }
}
