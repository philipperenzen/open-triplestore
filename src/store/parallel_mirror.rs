//! In-memory subject-sharded read accelerator over the persistent store.
//!
//! `TripleStore` already keeps in-memory derived indexes over its single
//! persistent Oxigraph store (`GraphIndex`, `SpatialIndex`); this is one more.
//! A *decomposable aggregate* query (a global non-distinct `COUNT`/`SUM`, a
//! subject-star join `COUNT`, a row-local `FILTER` `COUNT`, a mergeable `GROUP BY`,
//! or an `ASK` — classified conservatively by [`opengraph::parallel`]) is
//! evaluated on `N` subject-hash shards concurrently (Rayon) and merged, turning a
//! single-core scan into an `N`-core one. Anything the classifier cannot prove
//! safe — and every row-returning `SELECT` (whose shard-concat order would differ
//! from the single store) — falls back to single-store evaluation, so the result
//! is always identical; parallelism is never traded for correctness.
//!
//! ## Why a mirror, and why it is bounded
//!
//! Oxigraph evaluates one query on one thread, so the live `/sparql` path leaves
//! every other core idle on a large scan/aggregation. Subject-hash sharding gives
//! true `1/N`-per-core work, but only physical partitioning delivers that — hence
//! a mirror. It is **gated by a triple-count cap** (default 2M, configurable): the
//! mirror is never built for a store larger than the cap, so it cannot exhaust
//! memory on the large/100M tiers — it simply stays disabled and the single store
//! answers. It is rebuilt lazily after writes (a dirty flag, exactly like
//! `SpatialIndex`), so a read-heavy analytic workload pays the build once and then
//! reuses warm shards.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use opengraph::parallel::{self, ParAnswer, ParClass, ParallelStore};
use oxigraph::sparql::{QueryOptions, QueryResults, QuerySolutionIter, Variable};
use oxigraph::store::Store;
use tracing::debug;

/// Default ceiling on the number of triples the mirror will hold in memory.
const DEFAULT_MAX_TRIPLES: usize = 2_000_000;
/// Hard cap on shard count regardless of core count / configuration.
const MAX_SHARDS: usize = 16;

/// Subject-sharded in-memory accelerator, shared (`Arc`) inside `TripleStore`.
#[derive(Clone)]
pub struct ParallelMirror {
    inner: Arc<Inner>,
}

struct Inner {
    /// The warm shards, or `None` before the first build / when over the cap.
    shards: RwLock<Option<Arc<ParallelStore>>>,
    /// Set on every write; the next aggregate query rebuilds before using shards.
    dirty: AtomicBool,
    /// Serializes (re)builds so concurrent queries don't each rebuild.
    build_lock: Mutex<()>,
    /// Triple count of the live shards (diagnostics).
    built_len: AtomicUsize,
    shard_count: usize,
    max_triples: usize,
    enabled: bool,
}

impl ParallelMirror {
    /// Build from environment configuration:
    ///   * `OTS_PARALLEL_QUERY`             — `0`/`false`/`off`/`no` disables it (default on)
    ///   * `OTS_PARALLEL_QUERY_MAX_TRIPLES` — memory cap in triples (default 2,000,000)
    ///   * `OTS_PARALLEL_QUERY_SHARDS`      — shard count (default = cores, capped at 16)
    pub fn from_env() -> Self {
        let enabled = std::env::var("OTS_PARALLEL_QUERY")
            .map(|v| {
                !matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "0" | "false" | "off" | "no"
                )
            })
            .unwrap_or(true);
        let max_triples = std::env::var("OTS_PARALLEL_QUERY_MAX_TRIPLES")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(DEFAULT_MAX_TRIPLES);
        let shard_count = std::env::var("OTS_PARALLEL_QUERY_SHARDS")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or_else(default_shards)
            .clamp(1, MAX_SHARDS);
        Self::new(enabled, shard_count, max_triples)
    }

    /// Explicit construction (used by tests).
    pub fn new(enabled: bool, shard_count: usize, max_triples: usize) -> Self {
        Self {
            inner: Arc::new(Inner {
                shards: RwLock::new(None),
                dirty: AtomicBool::new(true),
                build_lock: Mutex::new(()),
                built_len: AtomicUsize::new(0),
                shard_count: shard_count.clamp(1, MAX_SHARDS),
                max_triples,
                enabled,
            }),
        }
    }

    /// Mark the mirror stale after any write to the persistent store.
    pub fn mark_dirty(&self) {
        self.inner.dirty.store(true, Ordering::Release);
    }

    /// Try to answer `sparql` in parallel across the shards, returning `None` to
    /// mean "fall back to single-store evaluation" — for a disabled mirror, a
    /// query that is not a decomposable **aggregate** (row-returning SELECTs keep
    /// single-store order), an over-cap store, or any shard error.
    ///
    /// `options` is a factory (called at most once) so the relatively expensive
    /// `QueryOptions` build is skipped entirely for non-accelerable queries.
    pub fn try_query<F>(&self, store: &Store, sparql: &str, options: F) -> Option<QueryResults>
    where
        F: FnOnce() -> QueryOptions,
    {
        if !self.inner.enabled {
            return None;
        }
        // Only order-insensitive aggregates/ASK are accelerated on the live path.
        if parallel::classify(sparql) != Some(ParClass::Aggregate) {
            return None;
        }
        let shards = self.get_or_build(store)?;
        match shards.query_with_options(sparql, options()) {
            Ok(Some(ans)) => Some(par_answer_to_results(ans)),
            // Classifier/merge mismatch or a shard error → single-store fallback.
            _ => None,
        }
    }

    /// Get warm shards, (re)building from `store` if dirty, or `None` if the store
    /// is empty or larger than the cap.
    fn get_or_build(&self, store: &Store) -> Option<Arc<ParallelStore>> {
        // Fast path: clean and present — no lock contention with other readers.
        if !self.inner.dirty.load(Ordering::Acquire) {
            if let Some(ps) = self.inner.shards.read().ok()?.clone() {
                return Some(ps);
            }
        }
        // (Re)build under the build lock so concurrent queries build at most once.
        let _guard = self.inner.build_lock.lock().ok()?;
        if !self.inner.dirty.load(Ordering::Acquire) {
            if let Some(ps) = self.inner.shards.read().ok()?.clone() {
                return Some(ps);
            }
        }
        let total = store.len().unwrap_or(usize::MAX);
        if total == 0 || total > self.inner.max_triples {
            // Over the cap (or empty): keep the accelerator off for this state.
            *self.inner.shards.write().ok()? = None;
            self.inner.built_len.store(0, Ordering::Release);
            self.inner.dirty.store(false, Ordering::Release);
            debug!(
                "parallel mirror inactive: {total} triples (cap {})",
                self.inner.max_triples
            );
            return None;
        }
        let ps = Arc::new(build_from_store(store, self.inner.shard_count)?);
        *self.inner.shards.write().ok()? = Some(ps.clone());
        self.inner.built_len.store(total, Ordering::Release);
        self.inner.dirty.store(false, Ordering::Release);
        debug!(
            "parallel mirror built: {total} triples across {} shards",
            self.inner.shard_count
        );
        Some(ps)
    }
}

fn default_shards() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, MAX_SHARDS)
}

/// Build a subject-sharded `ParallelStore` mirroring every quad in `store`
/// (all graphs preserved, so `FROM`/default-graph scoping is identical per shard).
fn build_from_store(store: &Store, shards: usize) -> Option<ParallelStore> {
    let ps = ParallelStore::new(shards);
    ps.load_quads(store.iter().filter_map(Result::ok)).ok()?;
    Some(ps)
}

/// Convert a merged parallel answer back into Oxigraph `QueryResults`, so the
/// caller cannot tell the answer was produced across shards.
fn par_answer_to_results(ans: ParAnswer) -> QueryResults {
    match ans {
        ParAnswer::Boolean(b) => QueryResults::Boolean(b),
        ParAnswer::Solutions { variables, rows } => {
            let vars: Arc<[Variable]> = Arc::from(variables);
            QueryResults::Solutions(QuerySolutionIter::new(vars, rows.into_iter().map(Ok)))
        }
    }
}
