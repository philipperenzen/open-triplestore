//! The repair layer: rules that say how to complete or correct a dataset,
//! a restricted chase that runs them in a throwaway copy, and a proposal —
//! an RDF Patch plus a report saying why every line is there — that a
//! reviewer applies after its base and write gates are checked
//! (`docs/notes/repair-layer-design.md`, user documentation in
//! `docs/repair.md`).
//!
//! It **proposes** and never applies on its own: nothing here writes the
//! store except the proposal `apply` resource, which checks the proposal's
//! base and runs the write gates first.
//!
//! * [`rules`] — the rule model, parsing and checking;
//! * [`vocab`] — `ots:Rule` resources in a graph, and back to Turtle;
//! * [`compile`] — rules from SHACL Core shapes and OWL axioms;
//! * [`stratify`] — strata from the rules' dependencies;
//! * [`chase`] — the restricted chase over the sandbox;
//! * [`sandbox`] — what is read, and the copy it is read into;
//! * [`proposal`] — the patch and the report;
//! * [`persist`] — proposals kept for review, as files;
//! * [`run`] — one run, end to end (blocking);
//! * [`handlers`] — the HTTP routes.

pub mod chase;
pub mod compile;
pub mod handlers;
pub mod persist;
pub mod proposal;
pub mod rules;
pub mod run;
pub mod sandbox;
pub mod stratify;
#[cfg(test)]
mod tests;
pub mod vocab;

use std::sync::{Arc, OnceLock};

use dashmap::DashMap;

use crate::store::TripleStore;

/// `OTS_REPAIR_CONCURRENCY`: how many repair runs may hold a sandbox at
/// once (default 1, §5.4). Taken in addition to the expensive-operation
/// permit, so resident sandbox memory stays bounded by this times the cap.
pub fn concurrency() -> usize {
    std::env::var("OTS_REPAIR_CONCURRENCY")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(1)
}

/// What the repair layer keeps per store: its semaphore and, once the server
/// configures it, where proposals are written.
pub struct RepairRuntime {
    pub semaphore: Arc<tokio::sync::Semaphore>,
    proposal_root: std::sync::RwLock<Option<std::path::PathBuf>>,
    /// Overrides `OTS_REPAIR_MAX_QUADS` for this store (tests).
    max_quads: std::sync::atomic::AtomicUsize,
    /// Proposals of a runtime with no root (tests, an in-memory store).
    memory: std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>,
}

impl RepairRuntime {
    fn new() -> Self {
        Self {
            semaphore: Arc::new(tokio::sync::Semaphore::new(concurrency())),
            proposal_root: std::sync::RwLock::new(None),
            max_quads: std::sync::atomic::AtomicUsize::new(0),
            memory: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// The premises cap: the override when one is set, else
    /// [`sandbox::max_quads`].
    pub fn max_quads(&self) -> usize {
        match self.max_quads.load(std::sync::atomic::Ordering::Relaxed) {
            0 => sandbox::max_quads(),
            n => n,
        }
    }

    /// Set the premises cap for this store (`0` restores the default).
    pub fn set_max_quads(&self, n: usize) {
        self.max_quads
            .store(n, std::sync::atomic::Ordering::Relaxed);
    }

    /// The directory proposals are written to (`None`: kept in memory).
    pub fn proposal_root(&self) -> Option<std::path::PathBuf> {
        self.proposal_root.read().ok().and_then(|r| r.clone())
    }

    pub fn set_proposal_root(&self, root: Option<std::path::PathBuf>) {
        if let Ok(mut r) = self.proposal_root.write() {
            *r = root;
        }
    }

    pub(crate) fn memory(&self) -> &std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>> {
        &self.memory
    }
}

/// The runtime of `store` (one per store instance, shared by its clones),
/// created on first use. Kept beside the store rather than in `AppState`, so
/// no state constructor changes.
pub fn runtime(store: &TripleStore) -> Arc<RepairRuntime> {
    static RUNTIMES: OnceLock<DashMap<u64, Arc<RepairRuntime>>> = OnceLock::new();
    RUNTIMES
        .get_or_init(DashMap::new)
        .entry(store.instance_id())
        .or_insert_with(|| Arc::new(RepairRuntime::new()))
        .clone()
}

/// Called once at server start: proposals of `store` are written under
/// `{data_dir}/repair-proposals/` (§7.3).
pub fn configure(store: &TripleStore, data_dir: &std::path::Path) {
    runtime(store).set_proposal_root(Some(data_dir.join("repair-proposals")));
}
