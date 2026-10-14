//! The locks version operations take (delta note §4.2, §3.3).
//!
//! * One per dataset, held across a whole version operation — label check,
//!   snapshot or range selection, registry insert, bindings snapshot, commit
//!   record — in create, branch, publish, restore, delete, gc and the
//!   pipeline's auto-versions. Without it two concurrent creates of one label
//!   both passed the existence check, the second's clear wiped the first's
//!   copy, and both wrote the record (duplicate `ver:status` triples); a gc
//!   could drop the graphs a restore was reading.
//! * One per version, held by a materialisation build: reads of other
//!   versions, and of physical ones, never wait for a cold read.
//!
//! Both are `tokio` mutexes because the dataset lock spans awaits between
//! blocking sections. A synchronous caller on a blocking thread takes it with
//! `blocking_lock`. The store is single-process (RocksDB's `LOCK` file), so
//! no cross-process lock is needed.

use std::sync::{Arc, OnceLock};

use dashmap::DashMap;

type Locks = DashMap<String, Arc<tokio::sync::Mutex<()>>>;

static DATASET_LOCKS: OnceLock<Locks> = OnceLock::new();
static BUILD_LOCKS: OnceLock<Locks> = OnceLock::new();

fn get(map: &'static OnceLock<Locks>, key: &str) -> Arc<tokio::sync::Mutex<()>> {
    map.get_or_init(DashMap::new)
        .entry(key.to_string())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

/// The version lock of `dataset_id`.
pub fn dataset(dataset_id: &str) -> Arc<tokio::sync::Mutex<()>> {
    get(&DATASET_LOCKS, dataset_id)
}

/// The build mutex of one version (by its IRI).
pub fn build(version_iri: &str) -> Arc<tokio::sync::Mutex<()>> {
    get(&BUILD_LOCKS, version_iri)
}
