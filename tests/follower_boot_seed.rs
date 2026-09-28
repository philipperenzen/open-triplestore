//! A follower does not run the boot seed.
//!
//! The boot seed writes the Studio's meta-shapes, the per-standard shape
//! graphs, the bundled demo organisation and its datasets, the standard
//! vocabularies and the built-in documentation. Every one of those writes is
//! refused on a follower, which keeps its store read-only, and every refusal
//! was logged as a warning — so a follower's first boot printed a wall of
//! warnings describing a node working exactly as designed, with no way for an
//! operator to tell them from a real fault.
//!
//! Skipping is not a workaround: the same graphs and the same identity rows
//! arrive from the leader, and a follower's identity database is replaced
//! wholesale by the leader's snapshot, so anything seeded locally would be
//! overwritten within the first catch-up anyway.

mod common;

use common::*;
use open_triplestore::server::{run_boot_seed, BootSeed};
use open_triplestore::store::replication::{Mode, ReplicationConfig, Role, Scope};
use open_triplestore::store::TripleStore;

fn follower_state() -> open_triplestore::server::AppState {
    let store = TripleStore::in_memory()
        .unwrap()
        .with_replication(ReplicationConfig::follower(
            "http://leader.internal:7878",
            Mode::Hot,
            Scope::All,
        ));
    admin_state_with_store(store).0
}

#[tokio::test]
async fn a_follower_skips_the_boot_seed_entirely() {
    let state = follower_state();
    assert_eq!(state.store.replication().role(), Role::Follower);

    let outcome = tokio::task::spawn_blocking({
        let state = state.clone();
        move || run_boot_seed(&state, "http://follower.internal:7878", None)
    })
    .await
    .unwrap();

    assert_eq!(outcome, BootSeed::SkippedReadOnly);

    // Nothing was written, so there is nothing to have been refused.
    let datasets = state.auth_db.list_datasets().unwrap();
    assert!(
        datasets.is_empty(),
        "a follower seeded {} dataset(s) locally: {:?}",
        datasets.len(),
        datasets.iter().map(|d| &d.name).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn a_node_that_is_not_a_follower_still_seeds() {
    let state = admin_state().0;
    assert_eq!(state.store.replication().role(), Role::None);

    let outcome = tokio::task::spawn_blocking({
        let state = state.clone();
        move || run_boot_seed(&state, "http://node.internal:7878", None)
    })
    .await
    .unwrap();

    assert_eq!(outcome, BootSeed::Ran);
    assert!(
        !state.auth_db.list_datasets().unwrap().is_empty(),
        "the boot seed should have created the bundled demo datasets"
    );
}
