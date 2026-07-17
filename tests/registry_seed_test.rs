//! Standard-vocabulary seeding: idempotency regression.
//!
//! `seed_standard_vocabularies` runs on every startup. It must seed the bundled
//! vocabularies exactly once and be a cheap no-op thereafter — never accumulating
//! duplicate registry entries or versions across restarts.

mod common;

use common::test_state;
use open_triplestore::data_models::{registry, seed_vocab};

#[test]
fn seeding_standard_vocabularies_is_idempotent() {
    let state = test_state();

    // First run seeds the full bundled set.
    let first = seed_vocab::seed_standard_vocabularies(&state);
    assert!(first > 0, "first seed should create entries");

    // Second run must be a no-op — everything already exists.
    let second = seed_vocab::seed_standard_vocabularies(&state);
    assert_eq!(second, 0, "re-seeding must not create duplicate entries");

    // A known vocab is present with exactly one published 1.0.0 version (no dupes).
    assert!(registry::data_model_exists(
        &state.store,
        &state.base_url,
        "rdf"
    ));
    assert!(registry::version_exists(
        &state.store,
        &state.base_url,
        "rdf",
        "1.0.0"
    ));
    let versions = registry::list_versions(&state.store, &state.base_url, "rdf");
    assert_eq!(
        versions.len(),
        1,
        "re-seeding must not accumulate duplicate versions"
    );
}
