//! Upload / import of RDF files into versioned named graphs.

use oxigraph::io::{JsonLdProfileSet, RdfFormat, RdfParser};
use oxigraph::model::*;
use std::collections::HashMap;
use std::io::BufReader;

use crate::store::engine::StoreError;
use crate::store::TripleStore;

/// Detect RDF format from MIME type. The canonical extension/MIME → `RdfFormat`
/// mapping shared by every upload and bulk-import path.
pub fn format_from_media_type(mime: &str) -> Option<RdfFormat> {
    let mime = mime.split(';').next().unwrap_or(mime).trim();
    match mime {
        "text/turtle" | "application/turtle" => Some(RdfFormat::Turtle),
        "application/n-triples" => Some(RdfFormat::NTriples),
        "application/rdf+xml" => Some(RdfFormat::RdfXml),
        "application/n-quads" => Some(RdfFormat::NQuads),
        "application/trig" | "application/x-trig" => Some(RdfFormat::TriG),
        "application/ld+json" | "application/json" => Some(RdfFormat::JsonLd {
            profile: JsonLdProfileSet::empty(),
        }),
        _ => None,
    }
}

/// Detect format from a filename/extension fallback.
pub fn format_from_filename(name: &str) -> Option<RdfFormat> {
    let ext = name.rsplit('.').next()?.to_lowercase();
    match ext.as_str() {
        "ttl" | "turtle" => Some(RdfFormat::Turtle),
        "nt" | "ntriples" => Some(RdfFormat::NTriples),
        // `.owl` is Protégé's RDF/XML export extension.
        "rdf" | "xml" | "rdfxml" | "owl" => Some(RdfFormat::RdfXml),
        "nq" | "nquads" => Some(RdfFormat::NQuads),
        "trig" => Some(RdfFormat::TriG),
        "jsonld" | "json" => Some(RdfFormat::JsonLd {
            profile: JsonLdProfileSet::empty(),
        }),
        _ => None,
    }
}

/// Try to detect `owl:versionInfo` from parsed quad subjects.
fn extract_owl_version_info(quads: &[Quad]) -> Option<String> {
    const OWL_VERSION_INFO: &str = "http://www.w3.org/2002/07/owl#versionInfo";
    for quad in quads {
        if quad.predicate.as_str() == OWL_VERSION_INFO {
            if let Term::Literal(lit) = &quad.object {
                return Some(lit.value().to_string());
            }
        }
    }
    None
}

/// Return the subject IRI of the first `rdf:type owl:Ontology` declaration, if any.
fn detect_ontology_subject(quads: &[Quad]) -> Option<NamedNode> {
    const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
    const OWL_ONTOLOGY: &str = "http://www.w3.org/2002/07/owl#Ontology";
    for quad in quads {
        if quad.predicate.as_str() == RDF_TYPE {
            if let Term::NamedNode(obj) = &quad.object {
                if obj.as_str() == OWL_ONTOLOGY {
                    if let Subject::NamedNode(subj) = &quad.subject {
                        return Some(subj.clone());
                    }
                }
            }
        }
    }
    None
}

/// Slugify the last path segment of an IRI for use as a sub-graph suffix.
fn slugify_last_segment(iri: &str) -> String {
    let last = iri
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("data");
    // Replace anything that isn't alphanumeric or hyphen with hyphen
    let slug: String = last
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "data".to_string()
    } else {
        slug
    }
}

/// Parse bytes as RDF quads.
pub fn parse_quads(bytes: &[u8], format: RdfFormat) -> Result<Vec<Quad>, String> {
    let reader = BufReader::new(bytes);
    RdfParser::from_format(format)
        .for_reader(reader)
        .map(|r| r.map_err(|e| e.to_string()))
        .collect::<Result<Vec<Quad>, String>>()
}

/// Parse bytes as RDF without loading into store. Used for format detection and kind detection.
pub fn parse_rdf(bytes: &[u8], content_type: &str, filename: &str) -> Result<Vec<Quad>, String> {
    let format = format_from_media_type(content_type)
        .or_else(|| format_from_filename(filename))
        .ok_or_else(|| format!("Cannot detect RDF format from content-type '{content_type}' or filename '{filename}'"))?;
    parse_quads(bytes, format)
}

/// Result of a parse-and-load operation.
pub struct LoadResult {
    /// Detected or provided version string.
    pub version: String,
    /// IRIs of loaded sub-graphs (may be a single base graph if merged).
    pub sub_graphs: Vec<String>,
}

/// Parse an uploaded file and load its contents into versioned named graphs.
///
/// # Parameters
/// - `base_url`          — triplestore base URL (no trailing slash)
/// - `data_model_id`       — ontology identifier
/// - `version_override`  — if provided, overrides detected `owl:versionInfo`
/// - `bytes`             — raw file content
/// - `content_type`      — MIME type (e.g. "application/trig")  
/// - `filename`          — original filename, used as format fallback
/// - `merge`             — if true, all quads are merged into a single graph
#[allow(clippy::too_many_arguments)]
pub fn parse_and_load(
    store: &TripleStore,
    base_url: &str,
    data_model_id: &str,
    version_override: Option<&str>,
    bytes: &[u8],
    content_type: &str,
    filename: &str,
    merge: bool,
) -> Result<LoadResult, String> {
    // 1. Detect format
    let format = format_from_media_type(content_type)
        .or_else(|| format_from_filename(filename))
        .ok_or_else(|| format!("Cannot detect RDF format from content-type '{content_type}' or filename '{filename}'"))?;

    // 2. Parse all quads
    let mut quads = parse_quads(bytes, format)?;

    // 3. Determine version
    let file_has_version_info = extract_owl_version_info(&quads).is_some();
    let version = version_override
        .map(|v| v.to_string())
        .or_else(|| extract_owl_version_info(&quads))
        .ok_or_else(|| "Version not found in file and no version override provided".to_string())?;

    // Validate version string is safe for use in an IRI
    if version.contains('/') || version.contains(' ') || version.contains('#') {
        return Err(format!("Invalid version string: '{version}'"));
    }

    let base_graph = format!("{base_url}/data-model/{data_model_id}/version/{version}");

    // Inject owl:versionInfo into the linked data if not already present.
    // Use the declared owl:Ontology subject, or fall back to the ontology's canonical IRI.
    if !file_has_version_info {
        let subject_nn = detect_ontology_subject(&quads).unwrap_or_else(|| {
            NamedNode::new(format!("{base_url}/data-model/{data_model_id}").as_str())
                .expect("base ontology IRI is always valid")
        });
        let version_info_pred = NamedNode::new("http://www.w3.org/2002/07/owl#versionInfo")
            .expect("owl:versionInfo IRI is always valid");
        let version_literal = Literal::new_simple_literal(version.as_str());
        // Insert into the default graph slot; it will be re-routed to base_graph below.
        quads.push(Quad::new(
            Subject::NamedNode(subject_nn),
            version_info_pred,
            Term::Literal(version_literal),
            GraphName::DefaultGraph,
        ));
    }

    // 4. Group quads by target graph
    let mut graph_quads: HashMap<String, Vec<Quad>> = HashMap::new();

    for quad in quads {
        let target_graph = if merge {
            base_graph.clone()
        } else {
            match &quad.graph_name {
                GraphName::NamedNode(nn) => {
                    let suffix = slugify_last_segment(nn.as_str());
                    format!("{base_graph}/{suffix}")
                }
                GraphName::DefaultGraph | GraphName::BlankNode(_) => base_graph.clone(),
            }
        };
        graph_quads.entry(target_graph).or_default().push(quad);
    }

    // If no quads were parsed, treat as empty upload into base graph
    if graph_quads.is_empty() {
        graph_quads.insert(base_graph.clone(), Vec::new());
    }

    // 5. Load each target graph into the store
    let mut sub_graphs: Vec<String> = graph_quads.keys().cloned().collect();
    sub_graphs.sort();

    // Clear all target graphs in one batched transaction.
    let iri_refs: Vec<&str> = sub_graphs.iter().map(|s| s.as_str()).collect();
    store
        .bulk_delete_graphs(&iri_refs)
        .map_err(|e| format!("Failed to clear target graphs: {e}"))?;

    // Remap all quads to their target graph and bulk-insert in one pass.
    let mut all_quads: Vec<Quad> = Vec::new();
    for (target_iri, quads) in &graph_quads {
        let target_nn = NamedNode::new(target_iri.as_str())
            .map_err(|e| format!("Invalid graph IRI '{target_iri}': {e}"))?;
        let target_graph = GraphName::NamedNode(target_nn);
        for quad in quads {
            all_quads.push(Quad::new(
                quad.subject.clone(),
                quad.predicate.clone(),
                quad.object.clone(),
                target_graph.clone(),
            ));
        }
    }
    store
        .bulk_insert_quads(all_quads, &sub_graphs)
        .map_err(|e| format!("Failed to insert quads: {e}"))?;

    Ok(LoadResult {
        version,
        sub_graphs,
    })
}

/// Copy all quads from source sub-graphs to draft sub-graphs.
/// Source graph IRIs have `source_version` replaced with `target_version` to produce draft IRIs.
pub fn clone_graphs_as_draft(
    store: &TripleStore,
    base_url: &str,
    data_model_id: &str,
    source_version: &str,
    target_version: &str,
) -> Result<Vec<String>, crate::store::engine::StoreError> {
    let source_prefix = format!("{base_url}/data-model/{data_model_id}/version/{source_version}");
    let target_prefix = format!("{base_url}/data-model/{data_model_id}/version/{target_version}");

    // List all named graphs whose IRI starts with source_prefix
    let all_graphs = store.named_graphs()?;
    let source_graphs: Vec<String> = all_graphs
        .iter()
        .map(|nn| nn.as_str().to_string())
        .filter(|iri| iri.starts_with(&source_prefix))
        .collect();

    // Determine all (draft_iri, src_iri) pairs.
    let graph_pairs: Vec<(String, String)> = if source_graphs.is_empty() {
        // Fall back to root graph if no sub-graphs exist.
        vec![(target_prefix.clone(), source_prefix.clone())]
    } else {
        source_graphs
            .iter()
            .map(|src_iri| {
                let draft_iri = format!("{}{}", target_prefix, &src_iri[source_prefix.len()..]);
                (draft_iri, src_iri.clone())
            })
            .collect()
    };

    // Clear all draft graphs in one batched transaction (handles retries).
    let draft_iris: Vec<String> = graph_pairs.iter().map(|(d, _)| d.clone()).collect();
    let draft_refs: Vec<&str> = draft_iris.iter().map(|s| s.as_str()).collect();
    store.bulk_delete_graphs(&draft_refs)?;

    // Collect all remapped quads, then bulk-insert in one pass.
    let mut all_quads: Vec<Quad> = Vec::new();
    for (draft_iri, src_iri) in &graph_pairs {
        let draft_target = GraphName::NamedNode(
            NamedNode::new(draft_iri)
                .map_err(|e| StoreError::Parse(format!("Invalid IRI: {}", e)))?,
        );
        let src_graph = GraphNameRef::NamedNode(
            NamedNodeRef::new(src_iri)
                .map_err(|e| StoreError::Parse(format!("Invalid IRI: {}", e)))?,
        );
        for q in store.quads_for_graph(src_graph)? {
            all_quads.push(Quad::new(
                q.subject,
                q.predicate,
                q.object,
                draft_target.clone(),
            ));
        }
    }
    store.bulk_insert_quads(all_quads, &draft_iris)?;

    let mut draft_graphs = draft_iris;
    draft_graphs.sort();
    Ok(draft_graphs)
}
