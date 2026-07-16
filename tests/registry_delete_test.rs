//! Regression: `registry::delete_data_model` must fully remove a model and all of
//! its versions using *bounded* batches, and must not touch other models.
//!
//! Previously the delete issued a single unbounded `DELETE WHERE { ?v ver:dataModel
//! <ont> . ?v ?vp ?vo }`. A model with thousands of versions produced one giant
//! transaction that could pin RocksDB under write pressure and stall unrelated
//! writes. The delete now collects the version subjects and deletes them in
//! fixed-size batches (256 subjects/transaction). This test seeds far more versions
//! than one batch and asserts the model is gone while a sibling model survives.

use open_triplestore::data_models::models::{DataModelVersion, VersionStatus};
use open_triplestore::data_models::registry;
use open_triplestore::store::TripleStore;

const BASE: &str = "http://localhost:7878";

fn make_version(model_id: &str, n: usize) -> DataModelVersion {
    DataModelVersion {
        data_model_id: model_id.to_string(),
        version: format!("1.{n}"),
        status: VersionStatus::Draft,
        graph_iri: format!("{BASE}/data-model/{model_id}/version/1.{n}/data"),
        sub_graphs: vec![],
        created_at: "2026-01-01T00:00:00Z".to_string(),
        created_by: None,
        derived_from: None,
        notes: Some(format!("seed note {n}")),
        branch: None,
        sub_graph_status: vec![],
    }
}

fn seed_model(store: &TripleStore, id: &str, versions: usize) {
    registry::insert_data_model(
        store,
        BASE,
        id,
        "Test Model",
        &format!("{BASE}/ns/{id}#"),
        Some("a model"),
        true,
        None,
        None,
        None,
        "2026-01-01T00:00:00Z",
    )
    .expect("insert_data_model");
    for n in 0..versions {
        registry::insert_version(store, BASE, &make_version(id, n)).expect("insert_version");
    }
}

#[test]
fn delete_removes_all_versions_across_multiple_batches_and_spares_siblings() {
    let store = TripleStore::in_memory().unwrap();

    // Well past the 256-subject batch size, so the delete must span multiple batches.
    const VICTIM_VERSIONS: usize = 600;
    seed_model(&store, "victim", VICTIM_VERSIONS);
    seed_model(&store, "keeper", 5);

    assert_eq!(
        registry::list_versions(&store, BASE, "victim").len(),
        VICTIM_VERSIONS,
        "sanity: victim should have all seeded versions before delete"
    );
    assert!(registry::data_model_exists(&store, BASE, "victim"));

    registry::delete_data_model(&store, BASE, "victim").expect("delete_data_model");

    // Every version record AND the model record itself must be gone — no residue
    // left by a partial/off-by-one batch.
    assert!(
        registry::list_versions(&store, BASE, "victim").is_empty(),
        "all victim versions must be deleted"
    );
    assert!(
        !registry::data_model_exists(&store, BASE, "victim"),
        "victim model record must be deleted"
    );

    // The sibling model and its versions must be untouched — the VALUES batching must
    // only delete the targeted model's subjects.
    assert!(registry::data_model_exists(&store, BASE, "keeper"));
    assert_eq!(
        registry::list_versions(&store, BASE, "keeper").len(),
        5,
        "sibling model versions must survive"
    );
}
