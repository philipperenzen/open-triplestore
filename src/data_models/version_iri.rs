//! Mint `owl:versionIRI` for a data-model version.
//!
//! Pattern: `{namespace}/{version}` written into the version's named graph,
//! attached to the `owl:Ontology` subject. Falls back to the namespace IRI
//! itself as subject when the graph contains no `owl:Ontology` declaration.

use oxigraph::model::{NamedNodeRef, Term};
use oxigraph::sparql::QueryResults;

use crate::kind_detector::RegistryKind;
use crate::store::engine::StoreError;
use crate::store::TripleStore;

const OWL_VERSION_IRI: &str = "http://www.w3.org/2002/07/owl#versionIRI";
const OWL_PRIOR_VERSION: &str = "http://www.w3.org/2002/07/owl#priorVersion";

const SKOS_CONCEPT_SCHEME: &str = "http://www.w3.org/2004/02/skos/core#ConceptScheme";
const DCAT_HAS_VERSION: &str = "http://www.w3.org/ns/dcat#hasVersion";
const DCT_ISSUED: &str = "http://purl.org/dc/terms/issued";
const DCT_MODIFIED: &str = "http://purl.org/dc/terms/modified";
const DCT_IS_REPLACED_BY: &str = "http://purl.org/dc/terms/isReplacedBy";
const PAV_VERSION: &str = "http://purl.org/pav/version";
const XSD_DATE_TIME: &str = "http://www.w3.org/2001/XMLSchema#dateTime";

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
    let QueryResults::Solutions(sols) = store.query(&q).ok()? else {
        return None;
    };
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
        namespace
            .trim_end_matches('/')
            .trim_end_matches('#')
            .to_string()
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
        Some(prior) if NamedNodeRef::new(prior).is_ok() => {
            format!("<{subject}> <{OWL_PRIOR_VERSION}> <{prior}> .")
        }
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

/// Locate the `skos:ConceptScheme` subject inside a named graph; return its IRI.
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
    let QueryResults::Solutions(sols) = store.query(&q).ok()? else {
        return None;
    };
    for row in sols.flatten() {
        if let Some(Some(Term::NamedNode(nn))) = row.values().first() {
            return Some(nn.as_str().to_string());
        }
    }
    None
}

/// Stamp DCAT/PAV/SKOS version metadata onto the `skos:ConceptScheme` subject
/// (falling back to the namespace IRI): `dcat:hasVersion`, `pav:version`,
/// `dcterms:issued`/`dcterms:modified`, and optional `dcterms:isReplacedBy`.
fn stamp(
    store: &TripleStore,
    graph_iri: &str,
    namespace: &str,
    version: &str,
    issued_at: &str,
    replaces_version_iri: Option<&str>,
) -> Result<String, StoreError> {
    let subject = find_scheme_subject(store, graph_iri).unwrap_or_else(|| {
        namespace
            .trim_end_matches('/')
            .trim_end_matches('#')
            .to_string()
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
        Some(r) if NamedNodeRef::new(r).is_ok() => {
            format!("<{subject}> <{DCT_IS_REPLACED_BY}> <{r}> .")
        }
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

/// Stamp version metadata into a freshly published version graph, choosing the
/// right vocabulary by **graph content**:
///
/// * an `owl:Ontology` subject ⇒ OWL `owl:versionIRI` / `owl:priorVersion`
///   ([`mint`]);
/// * a `skos:ConceptScheme` subject ⇒ DCAT/PAV/SKOS metadata ([`stamp`]);
/// * a graph carrying **both** gets both (mixed model + vocabulary packages);
/// * a graph with neither falls back to the entry's recorded [`RegistryKind`].
///
/// Returns the canonical version IRI (`{namespace}/{version}`).
#[allow(clippy::too_many_arguments)]
pub fn stamp_version(
    store: &TripleStore,
    graph_iri: &str,
    namespace: &str,
    version: &str,
    issued_at: &str,
    prior_version_iri: Option<&str>,
    kind: RegistryKind,
) -> Result<String, StoreError> {
    let has_ontology = find_ontology_subject(store, graph_iri).is_some();
    let has_scheme = find_scheme_subject(store, graph_iri).is_some();
    let mut version_iri = build_version_iri(namespace, version);

    if has_ontology {
        version_iri = mint(store, graph_iri, namespace, version, prior_version_iri)?;
    }
    if has_scheme {
        version_iri = stamp(
            store,
            graph_iri,
            namespace,
            version,
            issued_at,
            prior_version_iri,
        )?;
    }
    if !has_ontology && !has_scheme {
        version_iri = match kind {
            RegistryKind::Vocabulary => stamp(
                store,
                graph_iri,
                namespace,
                version,
                issued_at,
                prior_version_iri,
            )?,
            _ => mint(store, graph_iri, namespace, version, prior_version_iri)?,
        };
    }
    Ok(version_iri)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_iri_strips_trailing_slash() {
        assert_eq!(
            build_version_iri("https://ex.org/ont/", "1.2.0"),
            "https://ex.org/ont/1.2.0"
        );
        assert_eq!(
            build_version_iri("https://ex.org/ont", "1.2.0"),
            "https://ex.org/ont/1.2.0"
        );
        assert_eq!(
            build_version_iri("https://ex.org/ont#", "1.2.0"),
            "https://ex.org/ont/1.2.0"
        );
    }
}
