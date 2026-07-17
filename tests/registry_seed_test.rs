//! Standard-vocabulary seeding: multi-version + idempotency regression.
//!
//! `seed_standard_vocabularies` runs on every startup. It seeds every bundled
//! vocabulary with **all of its real published versions** (e.g. RDF 1.0/1.1/1.2,
//! OWL 1.0/2.0, DCAT 1.0/2.0/3.0, GeoSPARQL 1.0/1.1) exactly once, and is a cheap
//! no-op thereafter — never accumulating duplicate registry entries or versions
//! across restarts.

mod common;

use common::test_state;
use open_triplestore::data_models::{registry, seed_vocab};

#[test]
fn seeding_standard_vocabularies_is_idempotent() {
    let state = test_state();

    // First run seeds the full bundled set (every vocab × every version).
    let first = seed_vocab::seed_standard_vocabularies(&state);
    assert!(first > 0, "first seed should create versions");

    // Second run must be a no-op — every version already exists.
    let second = seed_vocab::seed_standard_vocabularies(&state);
    assert_eq!(second, 0, "re-seeding must not create duplicate versions");

    assert!(registry::data_model_exists(
        &state.store,
        &state.base_url,
        "rdf"
    ));

    // Multi-version standards ship all of their versions out of the box; none
    // should be duplicated on the second (no-op) seed.
    for (id, ver) in [
        ("rdf", "1.0"),
        ("rdf", "1.1"),
        ("rdf", "1.2"),
        ("owl", "1.0"),
        ("owl", "2.0"),
        ("dcat", "1.0"),
        ("dcat", "2.0"),
        ("dcat", "3.0"),
        ("geosparql", "1.0"),
        ("geosparql", "1.1"),
    ] {
        assert!(
            registry::version_exists(&state.store, &state.base_url, id, ver),
            "{id} should have version {ver} seeded"
        );
    }

    assert_eq!(
        registry::list_versions(&state.store, &state.base_url, "rdf").len(),
        3,
        "rdf must have exactly three versions and no duplicates after re-seed"
    );
    assert_eq!(
        registry::list_versions(&state.store, &state.base_url, "owl").len(),
        2,
        "owl must have exactly two versions (OWL 1 + OWL 2)"
    );
    assert_eq!(
        registry::list_versions(&state.store, &state.base_url, "dcat").len(),
        3,
        "dcat must have exactly three versions"
    );

    // The canonical latest must be the stable RDF 1.1 — never the 1.2 draft.
    let rdf =
        registry::get_data_model(&state.store, &state.base_url, "rdf").expect("rdf registry entry");
    let latest = rdf
        .latest_published
        .expect("rdf has a latest published version");
    assert!(
        latest.ends_with("1.1"),
        "rdf latest_published should be 1.1 (the stable REC), got {latest}"
    );
    assert!(
        !latest.ends_with("1.2"),
        "a draft (RDF 1.2) must never be the latest published version"
    );
}
