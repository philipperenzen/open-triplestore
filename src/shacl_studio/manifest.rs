//! The form-manifest contract. The triplestore does not render forms — it
//! *publishes* everything the external form platform needs to load a
//! dataset and its attached SHACL shapes on its own: dataset metadata,
//! prefixes, the shapes (Turtle + SHACLC) with their target classes, the data
//! graph IRIs, and the SPARQL / Graph-Store endpoints. Access is gated by the
//! dataset's existing ACL at the handler; this module just assembles the body.

use std::collections::BTreeSet;

use serde_json::json;

use crate::auth::db::AuthDb;
use crate::auth::models::{Dataset, Visibility};
use crate::store::TripleStore;

use super::store::ShaclStudioStore;

/// Common namespaces published so form platform can resolve terms without guessing.
/// The shapes Turtle also carries its own `@prefix` lines.
fn standard_prefixes() -> serde_json::Value {
    json!({
        "rdf": "http://www.w3.org/1999/02/22-rdf-syntax-ns#",
        "rdfs": "http://www.w3.org/2000/01/rdf-schema#",
        "xsd": "http://www.w3.org/2001/XMLSchema#",
        "owl": "http://www.w3.org/2002/07/owl#",
        "sh": "http://www.w3.org/ns/shacl#",
        "skos": "http://www.w3.org/2004/02/skos/core#",
        "dcterms": "http://purl.org/dc/terms/",
        "geo": "http://www.opengis.net/ont/geosparql#",
        "qudt": "http://qudt.org/schema/qudt/"
    })
}

/// Collect the shapes graphs attached to a dataset: its legacy
/// `shapes_graph_iri` plus the *effective* shape graphs bound to it in the
/// validation layer — its own bindings and those inherited from every graph it
/// contains. Returned as `(graph_iri, target_classes)` pairs.
fn attached_shape_graphs(
    store: &TripleStore,
    auth_db: &AuthDb,
    base_url: &str,
    studio: &ShaclStudioStore,
    dataset: &Dataset,
) -> Vec<(String, Vec<String>)> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out: Vec<(String, Vec<String>)> = Vec::new();

    // Legacy single shapes graph — kept during the transition; deduped against
    // the validation-layer bindings below.
    if let Some(iri) = dataset.shapes_graph_iri.as_ref().filter(|s| !s.is_empty()) {
        if seen.insert(iri.clone()) {
            let tc = studio
                .get_shape_graph_by_iri(iri)
                .ok()
                .flatten()
                .map(|s| s.target_classes)
                .unwrap_or_default();
            out.push((iri.clone(), tc));
        }
    }

    for set in super::bindings::effective_shape_graphs_for_dataset(store, auth_db, studio, base_url, dataset) {
        if seen.insert(set.graph_iri.clone()) {
            out.push((set.graph_iri, set.target_classes));
        }
    }
    out
}

/// Assemble the form-manifest JSON for `dataset`.
pub fn build_manifest(
    store: &TripleStore,
    auth_db: &AuthDb,
    base_url: &str,
    studio: &ShaclStudioStore,
    dataset: &Dataset,
) -> serde_json::Value {
    let data_graphs = auth_db.list_dataset_graphs(&dataset.id).unwrap_or_default();

    let mut shapes = Vec::new();
    let mut all_targets: BTreeSet<String> = BTreeSet::new();
    for (graph_iri, targets) in attached_shape_graphs(store, auth_db, base_url, studio, dataset) {
        let turtle = store
            .graph_store_get(Some(&graph_iri), oxigraph::io::RdfFormat::Turtle)
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .unwrap_or_default();
        let shaclc = crate::shaclc::serialize(store, &graph_iri).ok();
        for t in &targets {
            all_targets.insert(t.clone());
        }
        shapes.push(json!({
            "graph_iri": graph_iri,
            "target_classes": targets,
            "turtle": turtle,
            "shaclc": shaclc,
        }));
    }

    let base = base_url.trim_end_matches('/');
    let public = dataset.visibility == Visibility::Public;

    json!({
        "version": 1,
        "dataset": {
            "id": dataset.id,
            "name": dataset.name,
            "description": dataset.description,
            "visibility": dataset.visibility.as_str(),
            "owner_type": dataset.owner_type.as_str(),
            "owner_id": dataset.owner_id,
        },
        "base_url": base,
        "prefixes": standard_prefixes(),
        "shapes": shapes,
        "target_classes": all_targets.into_iter().collect::<Vec<_>>(),
        "data_graphs": data_graphs,
        "endpoints": {
            "sparql": format!("{base}/sparql"),
            "graph_store": format!("{base}/store"),
            "shapes": format!("{base}/api/datasets/{}/shapes", dataset.id),
            "manifest": format!("{base}/api/datasets/{}/form-manifest", dataset.id),
        },
        "access": {
            "public": public,
            "auth_required": !public,
            // form platform obtains a scoped, expiring token via the share-link API
            // for member/private datasets or to submit edits.
            "share_link_api": format!("{base}/api/datasets/{}/share-links", dataset.id),
        }
    })
}
