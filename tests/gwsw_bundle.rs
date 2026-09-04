//! The GWSW seed bundle (examples/seed-bundles/gwsw) on RIONED's real
//! "Totaal" export: the model graph loads through the manifest-driven seed
//! engine, is registered as a model-kind data model, and answers a query the
//! way the published ontology does. The Turtle is not vendored (run the
//! bundle's fetch.sh once, it is CC0); without it this test reports that it
//! skipped and passes, so CI stays green.

mod common;

use std::path::Path;

use common::*;
use open_triplestore::data_models::registry::list_data_models;
use open_triplestore::seed_bundles::load_seed_dir;
use oxigraph::sparql::QueryResults;

const GWSW: &str = "http://data.gwsw.nl/1.7/totaal/";

#[tokio::test]
async fn gwsw_bundle_loads_the_totaal_export_as_a_model() {
    let bundles = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/seed-bundles");
    if !bundles.join("gwsw/gwsw-totaal.ttl").exists() {
        eprintln!("SKIP: gwsw-totaal.ttl is not present — run examples/seed-bundles/gwsw/fetch.sh");
        return;
    }

    let (state, _token) = admin_state();
    load_seed_dir(&state, &bundles);

    let n = state.store.graph_count_cached(Some(GWSW)).unwrap_or(0);
    assert!(
        n > 20_000,
        "GWSW Totaal loaded into its model graph ({n} triples)"
    );

    let ask = |q: &str| matches!(state.store.query(q), Ok(QueryResults::Boolean(true)));
    // A sewer manhole is a class of the dictionary, and the export carries the
    // OWL restrictions that give the model its structure.
    assert!(ask(&format!(
        "ASK {{ GRAPH <{GWSW}> {{ <{GWSW}Rioolput> a <http://www.w3.org/2002/07/owl#Class> }} }}"
    )));
    assert!(ask(&format!(
        "ASK {{ GRAPH <{GWSW}> {{ ?r <http://www.w3.org/2002/07/owl#onClass> <{GWSW}Rioolput> }} }}"
    )));

    // Registered as a data model under the bundle's namespace.
    let models = list_data_models(&state.store);
    let gwsw = models
        .iter()
        .find(|m| m.id == "gwsw")
        .expect("gwsw data model registered");
    assert_eq!(gwsw.namespace, GWSW);
    assert!(gwsw.title.contains("GWSW"), "{}", gwsw.title);
}
