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

/// Longest accepted version identifier.
pub const MAX_VERSION_LEN: usize = 64;

/// Reject any version string that is not safe to interpolate into SPARQL.
///
/// A version reaches the store through two different constructs — IRIs
/// (`…/version/{version}`) and string literals (`owl:versionInfo "{version}"`)
/// — so a blocklist has to anticipate the escape characters of both. This is an
/// allowlist instead: ASCII alphanumerics plus `.`, `-`, `_`, `~` and `+`, which
/// is a subset of both the IRI `unreserved` set and the characters that carry no
/// meaning inside a quoted literal. `"`, `\`, `<`, `>`, `{`, `}`, `;` and every
/// whitespace/control character are therefore impossible by construction, so no
/// caller can terminate a literal or an IRI and append its own SPARQL operations.
///
/// Callers should run this at the API boundary to return a clean 400; every
/// SPARQL sink runs it again so a missed boundary cannot become an injection.
pub fn validate_version(version: &str) -> Result<(), String> {
    if version.is_empty() {
        return Err("Version must not be empty".to_string());
    }
    if version.len() > MAX_VERSION_LEN {
        return Err(format!(
            "Version must be at most {MAX_VERSION_LEN} characters (got {})",
            version.len()
        ));
    }
    if let Some(bad) = version
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '~' | '+')))
    {
        return Err(format!(
            "Invalid version string: character {bad:?} is not allowed \
             (use letters, digits, '.', '-', '_', '~' or '+')"
        ));
    }
    // `.` / `..` are relative-path segments: harmless in a literal, but they make
    // `…/version/..` normalise away in an IRI, so a version may not be dots alone.
    if version.chars().all(|c| c == '.') {
        return Err("Version must not consist only of dots".to_string());
    }
    Ok(())
}

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
    validate_version(version).map_err(StoreError::Parse)?;
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
    validate_version(version).map_err(StoreError::Parse)?;
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
    fn accepts_the_version_shapes_actually_in_use() {
        for ok in [
            "1",
            "1.0",
            "0.0.1",
            "2.3.1",
            "v1.2.0",
            "2008-01-14",
            "latest",
            "1.0.0-rc.1",
            "1.0.0+build_7",
            "a~b",
        ] {
            assert!(validate_version(ok).is_ok(), "{ok} should be accepted");
        }
    }

    #[test]
    fn rejects_sparql_injection_payloads() {
        // The reported payload: closes the literal, then appends its own operations.
        // It contains no '/', ' ' or '#', so the previous blocklist let it through.
        let payload = "x\"}}\t;\tDELETE\tWHERE{GRAPH?g{?s?p?o}}\t;\tINSERT\tDATA\t{GRAPH<urn:x>{<urn:s><urn:p>\"";
        assert!(validate_version(payload).is_err());

        for bad in [
            "1.0\"", "1.0\\", "a<b", "a>b", "a{b", "a}b", "a;b", "a b", "a\tb", "a\nb", "a/b",
            "a#b", "", ".", "..",
        ] {
            assert!(validate_version(bad).is_err(), "{bad:?} should be rejected");
        }
        assert!(validate_version(&"1".repeat(MAX_VERSION_LEN + 1)).is_err());
    }

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
