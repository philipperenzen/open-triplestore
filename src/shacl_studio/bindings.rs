//! The **validation layer**: shape↔target bindings stored as RDF in the single
//! system graph `<urn:system:validation-layer>`. This is the source of truth
//! for "what validates what" — read back via SPARQL (the same approach the
//! dataset-version registry uses), so there is no SQL mirror table to drift.
//!
//! A binding is `<targetIri> ots:validatedBy <shapeGraphIri>` (with a
//! standards-friendly `dct:conformsTo` mirror). The *object* is always a shape
//! set's data graph IRI, so it round-trips back to a [`ShapeGraph`] via
//! [`ShaclStudioStore::get_shape_graph_by_iri`]. The *subject* is whatever is
//! being validated:
//!
//! * a dataset → `{base}/datasets/{id}` (the DCAT dataset IRI),
//! * a named graph → the graph's own IRI,
//! * a shape graph (meta-validation) → that set's data graph IRI.
//!
//! Because a graph carries its bindings wherever it is mounted,
//! [`effective_shape_graphs_for_dataset`] gives dynamic inheritance: a dataset
//! validates against its own bindings *plus* those of every graph it contains.

use oxigraph::model::Term;
use oxigraph::sparql::QueryResults;

use crate::auth::db::AuthDb;
use crate::auth::models::Dataset;
use crate::store::engine::StoreError;
use crate::store::TripleStore;

use super::models::ShapeGraph;
use super::store::ShaclStudioStore;

/// The single system graph holding every shape↔target binding.
pub const VALIDATION_GRAPH: &str = "urn:system:validation-layer";

const OTS: &str = "https://opentriplestore.org/ontology/";
const DCT: &str = "http://purl.org/dc/terms/";

fn col0(store: &TripleStore, q: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(QueryResults::Solutions(sols)) = store.query(q) {
        for row in sols.flatten() {
            if let Some(Term::NamedNode(nn)) = row.values().first().and_then(|t| t.as_ref()) {
                out.push(nn.as_str().to_string());
            }
        }
    }
    out
}

/// Validation-layer target IRI for a whole dataset (the DCAT dataset IRI, i.e.
/// the same subject `write_dataset_metadata_graph` uses).
pub fn dataset_target_iri(base_url: &str, dataset_id: &str) -> String {
    // Singular `{base}/dataset/{id}` — the canonical dataset IRI (styleguide §3.3),
    // matching `write_dataset_metadata_graph` and the catalogue.
    format!("{}/dataset/{}", base_url.trim_end_matches('/'), dataset_id)
}

/// Record that `target_iri` is validated by the shape graph whose data lives in
/// `shape_graph_graph_iri`. Idempotent — re-inserting an existing triple is a
/// no-op. Also writes a `dct:conformsTo` mirror.
pub fn add_binding(
    store: &TripleStore,
    target_iri: &str,
    shape_graph_graph_iri: &str,
) -> Result<(), StoreError> {
    let q = format!(
        r#"
        PREFIX ots: <{OTS}>
        PREFIX dct: <{DCT}>
        INSERT DATA {{
          GRAPH <{VALIDATION_GRAPH}> {{
            <{target_iri}> ots:validatedBy <{shape_graph_graph_iri}> ;
                           dct:conformsTo <{shape_graph_graph_iri}> .
          }}
        }}
        "#
    );
    store.update(&q)
}

/// Remove a binding (both the `ots:validatedBy` triple and its `dct:conformsTo`
/// mirror). Removing an absent binding is a no-op.
pub fn remove_binding(
    store: &TripleStore,
    target_iri: &str,
    shape_graph_graph_iri: &str,
) -> Result<(), StoreError> {
    let q = format!(
        r#"
        PREFIX ots: <{OTS}>
        PREFIX dct: <{DCT}>
        DELETE DATA {{
          GRAPH <{VALIDATION_GRAPH}> {{
            <{target_iri}> ots:validatedBy <{shape_graph_graph_iri}> ;
                           dct:conformsTo <{shape_graph_graph_iri}> .
          }}
        }}
        "#
    );
    store.update(&q)
}

/// Shape-set graph IRIs bound to `target_iri` (its `ots:validatedBy` objects).
pub fn bindings_for_target(store: &TripleStore, target_iri: &str) -> Vec<String> {
    let q = format!(
        r#"PREFIX ots: <{OTS}>
        SELECT ?ss WHERE {{ GRAPH <{VALIDATION_GRAPH}> {{ <{target_iri}> ots:validatedBy ?ss }} }}"#
    );
    col0(store, &q)
}

/// Target IRIs validated by the shape graph whose data lives in
/// `shape_graph_graph_iri` (reverse lookup — powers impact display).
pub fn targets_for_shape_graph(store: &TripleStore, shape_graph_graph_iri: &str) -> Vec<String> {
    let q = format!(
        r#"PREFIX ots: <{OTS}>
        SELECT ?t WHERE {{ GRAPH <{VALIDATION_GRAPH}> {{ ?t ots:validatedBy <{shape_graph_graph_iri}> }} }}"#
    );
    col0(store, &q)
}

/// The shape graphs that effectively apply to a dataset: the union of the
/// dataset's own bindings and the bindings of every named graph it contains —
/// giving dynamic inheritance. Deduped by shape-graph id. This single resolver
/// backs write-gating, pipeline runs, and the form-manifest.
pub fn effective_shape_graphs_for_dataset(
    store: &TripleStore,
    auth_db: &AuthDb,
    studio: &ShaclStudioStore,
    base_url: &str,
    dataset: &Dataset,
) -> Vec<ShapeGraph> {
    let mut graph_iris: Vec<String> = Vec::new();

    // Dataset-level bindings.
    let ds_iri = dataset_target_iri(base_url, &dataset.id);
    graph_iris.extend(bindings_for_target(store, &ds_iri));

    // Per-graph (inherited) bindings.
    if let Ok(graphs) = auth_db.list_dataset_graphs(&dataset.id) {
        for g in graphs {
            graph_iris.extend(bindings_for_target(store, &g));
        }
    }

    // Resolve shape-graph graph IRIs back to records, deduped by id.
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for giri in graph_iris {
        if let Ok(Some(set)) = studio.get_shape_graph_by_iri(&giri) {
            if seen.insert(set.id.clone()) {
                out.push(set);
            }
        }
    }
    out
}

// ─── Versioning: snapshot/restore the validation layer with a dataset version ─

/// Two-column SELECT → `(subject, object)` pairs (both must be IRIs).
fn pairs01(store: &TripleStore, q: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Ok(QueryResults::Solutions(sols)) = store.query(q) {
        for row in sols.flatten() {
            let vals: Vec<_> = row.values().to_vec();
            if let (Some(Term::NamedNode(s)), Some(Term::NamedNode(o))) = (
                vals.first().and_then(|t| t.as_ref()),
                vals.get(1).and_then(|t| t.as_ref()),
            ) {
                out.push((s.as_str().to_string(), o.as_str().to_string()));
            }
        }
    }
    out
}

/// Version-scoped graph that snapshots a dataset version's validation-layer
/// bindings, so the "what validates what" state travels with the version. A
/// sibling of the version's data snapshot graphs under the same version IRI.
pub fn version_validation_graph(base_url: &str, dataset_id: &str, version: &str) -> String {
    format!(
        "{}/dataset/{}/version/{}/validation",
        base_url.trim_end_matches('/'),
        dataset_id,
        version
    )
}

/// Copy the dataset's bindings (dataset-level + each of `source_graphs`) into
/// the version-scoped validation graph. Returns the snapshot graph IRI when any
/// binding was captured, else `None`. Idempotent on retry (clears first).
pub fn snapshot_dataset_bindings(
    store: &TripleStore,
    base_url: &str,
    dataset_id: &str,
    version: &str,
    source_graphs: &[String],
) -> Result<Option<String>, StoreError> {
    let mut targets: Vec<String> = vec![dataset_target_iri(base_url, dataset_id)];
    targets.extend(source_graphs.iter().cloned());

    let snap_graph = version_validation_graph(base_url, dataset_id, version);
    store.update(&format!("CLEAR SILENT GRAPH <{snap_graph}>"))?;

    let mut body = String::new();
    let mut any = false;
    for t in &targets {
        for ss in bindings_for_target(store, t) {
            body.push_str(&format!(
                "<{t}> ots:validatedBy <{ss}> ; dct:conformsTo <{ss}> .\n"
            ));
            any = true;
        }
    }
    if !any {
        return Ok(None);
    }
    let q = format!(
        r#"PREFIX ots: <{OTS}>
        PREFIX dct: <{DCT}>
        INSERT DATA {{ GRAPH <{snap_graph}> {{ {body} }} }}"#
    );
    store.update(&q)?;
    Ok(Some(snap_graph))
}

/// Re-apply a version's snapshotted bindings to the live validation layer.
/// Additive (never removes current bindings) because a graph's bindings are
/// shared across every dataset that mounts it — clearing would have cross-
/// dataset side effects. Tolerant: a binding whose shape graph no longer exists
/// is skipped. Returns the number of bindings re-applied.
pub fn restore_dataset_bindings(
    store: &TripleStore,
    studio: &ShaclStudioStore,
    base_url: &str,
    dataset_id: &str,
    version: &str,
) -> Result<usize, StoreError> {
    let snap_graph = version_validation_graph(base_url, dataset_id, version);
    let q = format!(
        r#"PREFIX ots: <{OTS}>
        SELECT ?t ?ss WHERE {{ GRAPH <{snap_graph}> {{ ?t ots:validatedBy ?ss }} }}"#
    );
    let mut applied = 0;
    for (target, ss) in pairs01(store, &q) {
        if matches!(studio.get_shape_graph_by_iri(&ss), Ok(Some(_))) {
            add_binding(store, &target, &ss)?;
            applied += 1;
        }
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::db::AuthDb;
    use crate::auth::models::{OwnerType, Visibility};
    use crate::shacl_studio::models::ShapeSource;

    fn mk_set(studio: &ShaclStudioStore, name: &str, graph_iri: &str) -> ShapeGraph {
        studio
            .create_shape_graph(
                name,
                None,
                OwnerType::User,
                "u1",
                Visibility::Private,
                graph_iri,
                &[],
                ShapeSource::Manual,
                None,
            )
            .unwrap()
    }

    #[test]
    fn binding_round_trip() {
        let store = TripleStore::in_memory().unwrap();
        let target = "http://x/datasets/d1";
        let ss = "urn:shapes:s1";

        assert!(bindings_for_target(&store, target).is_empty());
        add_binding(&store, target, ss).unwrap();
        assert_eq!(bindings_for_target(&store, target), vec![ss.to_string()]);
        assert_eq!(
            targets_for_shape_graph(&store, ss),
            vec![target.to_string()]
        );

        // Idempotent: re-inserting the same triple does not duplicate it.
        add_binding(&store, target, ss).unwrap();
        assert_eq!(bindings_for_target(&store, target).len(), 1);

        remove_binding(&store, target, ss).unwrap();
        assert!(bindings_for_target(&store, target).is_empty());
    }

    #[test]
    fn effective_unions_dataset_and_graph_bindings_deduped() {
        let store = TripleStore::in_memory().unwrap();
        let auth = AuthDb::in_memory().unwrap();
        let studio = ShaclStudioStore::new(auth.pool());
        let base = "http://x";

        let ds_set = mk_set(&studio, "ds-level", "urn:shapes:ds");
        let g_set = mk_set(&studio, "graph-level", "urn:shapes:g");

        let dataset = auth
            .create_dataset(
                "d1",
                "D1",
                None,
                OwnerType::User,
                "u1",
                Visibility::Private,
                None,
            )
            .unwrap();
        auth.add_dataset_graph("d1", "urn:data:g1").unwrap();

        // Dataset-level binding + a graph-level (inherited) binding.
        add_binding(&store, &dataset_target_iri(base, "d1"), &ds_set.graph_iri).unwrap();
        add_binding(&store, "urn:data:g1", &g_set.graph_iri).unwrap();

        let eff = effective_shape_graphs_for_dataset(&store, &auth, &studio, base, &dataset);
        let mut ids: Vec<String> = eff.iter().map(|s| s.id.clone()).collect();
        ids.sort();
        let mut want = vec![ds_set.id.clone(), g_set.id.clone()];
        want.sort();
        assert_eq!(
            ids, want,
            "union of dataset-level + graph-inherited shape graphs"
        );

        // Overlap: attach ds_set to the graph too — must still dedupe by id.
        add_binding(&store, "urn:data:g1", &ds_set.graph_iri).unwrap();
        let eff = effective_shape_graphs_for_dataset(&store, &auth, &studio, base, &dataset);
        assert_eq!(eff.len(), 2, "deduped by shape-graph id");
    }

    #[test]
    fn snapshot_then_restore_bindings_round_trips() {
        let store = TripleStore::in_memory().unwrap();
        let auth = AuthDb::in_memory().unwrap();
        let studio = ShaclStudioStore::new(auth.pool());
        let base = "http://x";

        let ds_set = mk_set(&studio, "ds-level", "urn:shapes:ds");
        let g_set = mk_set(&studio, "graph-level", "urn:shapes:g");
        let ds_iri = dataset_target_iri(base, "d1");
        add_binding(&store, &ds_iri, &ds_set.graph_iri).unwrap();
        add_binding(&store, "urn:data:g1", &g_set.graph_iri).unwrap();

        // Snapshot captures the dataset-level + the source graph's bindings.
        let snap =
            snapshot_dataset_bindings(&store, base, "d1", "1.0.0", &["urn:data:g1".to_string()])
                .unwrap()
                .expect("bindings were captured");
        assert_eq!(snap, version_validation_graph(base, "d1", "1.0.0"));

        // Drop the live bindings, then restore from the version snapshot.
        remove_binding(&store, &ds_iri, &ds_set.graph_iri).unwrap();
        remove_binding(&store, "urn:data:g1", &g_set.graph_iri).unwrap();
        assert!(bindings_for_target(&store, &ds_iri).is_empty());

        let applied = restore_dataset_bindings(&store, &studio, base, "d1", "1.0.0").unwrap();
        assert_eq!(applied, 2, "both snapshotted bindings re-applied");
        assert_eq!(
            bindings_for_target(&store, &ds_iri),
            vec![ds_set.graph_iri.clone()]
        );
        assert_eq!(
            bindings_for_target(&store, "urn:data:g1"),
            vec![g_set.graph_iri.clone()]
        );

        // Tolerant: a binding whose shape graph is gone is skipped on restore.
        studio.delete_shape_graph(&g_set.id).unwrap();
        remove_binding(&store, "urn:data:g1", &g_set.graph_iri).unwrap();
        let applied = restore_dataset_bindings(&store, &studio, base, "d1", "1.0.0").unwrap();
        assert_eq!(
            applied, 1,
            "only the still-existing shape graph is re-applied"
        );
        assert!(
            bindings_for_target(&store, "urn:data:g1").is_empty(),
            "missing shape graph was skipped"
        );
    }

    #[test]
    fn snapshot_with_no_bindings_is_none() {
        let store = TripleStore::in_memory().unwrap();
        let snap = snapshot_dataset_bindings(&store, "http://x", "d-empty", "1.0.0", &[]).unwrap();
        assert!(snap.is_none(), "no bindings → no snapshot graph");
    }
}
