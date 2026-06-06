//! In-memory read accelerator over the persistent store — two RAM copies of the
//! data that, within a triple-count cap, serve the live `/sparql` path far faster
//! than RocksDB.
//!
//! `TripleStore` already keeps in-memory derived indexes over its single persistent
//! Oxigraph store (`GraphIndex`, `SpatialIndex`); this adds two more, both rebuilt
//! lazily after writes (a dirty flag, like `SpatialIndex`):
//!
//! 1. **Subject-hash shards** — for a *decomposable aggregate* (global non-distinct
//!    `COUNT`, subject-star join `COUNT`, row-local `FILTER` `COUNT`, a mergeable
//!    `GROUP BY` — `COUNT` and, via a per-shard rewrite re-merged through the engine,
//!    `SUM`/`MIN`/`MAX`/`AVG` — and `ASK`, classified conservatively by
//!    [`opengraph::parallel`]) the query runs on `N` shards concurrently (Rayon) and
//!    the partials merge, turning a single-core scan into an `N`-core one
//!    ([`Self::try_query`]).
//! 2. **An unsharded full copy** — for everything the shards can't decompose (joins,
//!    large `SELECT`s, `COUNT(DISTINCT)`, ordered/limited results, `CONSTRUCT`), the
//!    query runs against a single in-memory `Store` ([`Self::try_full_query`]). This
//!    is the bigger surprise win: the persistent (RocksDB) store answers a
//!    multi-pattern join with one point lookup *per result row*, so a 2-way join over
//!    500k triples that takes ~150 ms in RAM takes **~6 s** on RocksDB. Serving it
//!    from the full copy closes that ~40x gap.
//!
//! Both copies are faithful mirrors evaluated by the same engine over the same
//! quads, so results are byte-identical to single-store evaluation — parallelism and
//! the RAM copy are never traded for correctness. The one subtlety is IEEE-754:
//! `SUM`/`AVG` over `xsd:double`/`float` is non-associative, and neither a shard
//! merge nor a re-ordered copy can reproduce the persistent store's exact last bit,
//! so those are declined by **both** layers (the shards by datatype at runtime, the
//! full copy via [`opengraph::parallel::has_sum_or_avg`]) and the persistent store
//! answers them itself — still byte-identical. Anything not provably safe, or a shard
//! error, falls back; over the cap, both copies stay off and RocksDB answers.
//!
//! ## Why it is bounded
//!
//! The two copies cost ~2x the dataset in RAM, so the accelerator is **gated by a
//! triple-count cap** (default 2M, `OTS_PARALLEL_QUERY_MAX_TRIPLES`): above it the
//! mirror is never built — it stays disabled and the persistent store answers,
//! leaving the large/100M tiers (data > RAM, the reason the store is persistent at
//! all) on their normal path. A read-heavy workload within the cap pays the one-time
//! build after a write, then reuses warm copies.

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
    /// The warm subject-hash shards, or `None` before the first build / over cap.
    /// Used for decomposable aggregates (parallel across cores).
    shards: RwLock<Option<Arc<ParallelStore>>>,
    /// An **unsharded** in-memory copy of the whole store, used for everything the
    /// shards can't decompose (joins, `COUNT(DISTINCT)`, ordered/limited results,
    /// large `SELECT`s). RocksDB answers a multi-pattern join with one point lookup
    /// per row — ~40× slower than the same join in RAM — so serving these reads from
    /// this copy is the single biggest win for non-aggregate queries.
    full: RwLock<Option<Arc<Store>>>,
    /// Set on every write; the next query rebuilds before using either copy.
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
                full: RwLock::new(None),
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
            *self.inner.full.write().ok()? = None;
            self.inner.built_len.store(0, Ordering::Release);
            self.inner.dirty.store(false, Ordering::Release);
            debug!(
                "parallel mirror inactive: {total} triples (cap {})",
                self.inner.max_triples
            );
            return None;
        }
        // Build both copies before publishing either, so a build error leaves the
        // previous (or empty) state untouched.
        let ps = Arc::new(build_from_store(store, self.inner.shard_count)?);
        let full = Arc::new(build_full_store(store)?);
        *self.inner.shards.write().ok()? = Some(ps.clone());
        *self.inner.full.write().ok()? = Some(full);
        self.inner.built_len.store(total, Ordering::Release);
        self.inner.dirty.store(false, Ordering::Release);
        debug!(
            "parallel mirror built: {total} triples ({} shards + 1 full copy)",
            self.inner.shard_count
        );
        Some(ps)
    }

    /// Try to answer `sparql` from the **unsharded** in-memory copy — the path for
    /// reads the shards can't decompose (joins, `GROUP BY` with non-`COUNT`
    /// aggregates, large `SELECT`s). The copy is a faithful mirror of the persistent
    /// store evaluated by the same engine, so results are identical; it is just in
    /// RAM, avoiding RocksDB's per-row join lookups. Returns `None` (→ persistent
    /// store) for a disabled mirror, an over-cap store, or any evaluation error.
    pub fn try_full_query<F>(&self, store: &Store, sparql: &str, options: F) -> Option<QueryResults>
    where
        F: FnOnce() -> QueryOptions,
    {
        if !self.inner.enabled {
            return None;
        }
        // Fidelity: `SUM`/`AVG` over `xsd:double`/`float` is IEEE-754 non-associative,
        // and the full copy iterates quads in a different order than the persistent
        // store, so it could differ in the last ULP. Decline these so the persistent
        // store answers them itself — byte-identical to single-store evaluation.
        // (Grouped int/decimal `SUM`/`AVG` are already served exactly by the shards in
        // [`Self::try_query`] before this point; this only defers global/complex ones.)
        if parallel::has_sum_or_avg(sparql) {
            return None;
        }
        self.get_or_build(store)?; // ensures both copies are built (None if over cap)
        let full = self.inner.full.read().ok()?.clone()?;
        full.query_opt(sparql, options()).ok()
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

/// Build an unsharded in-memory `Store` holding every quad of `store` (all graphs
/// preserved, so `FROM`/default-graph scoping evaluates identically).
fn build_full_store(store: &Store) -> Option<Store> {
    let full = Store::new().ok()?;
    full.bulk_loader()
        .load_quads(store.iter().filter_map(Result::ok))
        .ok()?;
    Some(full)
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
