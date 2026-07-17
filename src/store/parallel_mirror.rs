//! In-memory read accelerator over the persistent store — two RAM copies of the
//! data that, within a triple-count cap, serve the live `/sparql` path far faster
//! than RocksDB.
//!
//! `TripleStore` already keeps in-memory derived indexes over its single persistent
//! Oxigraph store (`GraphIndex`, `SpatialIndex`); this adds two more, both rebuilt
//! lazily after writes (a dirty flag, like `SpatialIndex`):
//!
//! 1. **Subject-hash shards** — for a *decomposable aggregate* (non-distinct `COUNT`
//!    — global, subject-star join or grouped; `SUM`/`MIN`/`MAX`/`AVG` — global or
//!    grouped — via a per-shard rewrite re-merged through the engine; `COUNT(DISTINCT)`
//!    via per-shard distinct sets unioned through the engine; and `ASK` — classified
//!    conservatively by [`opengraph::parallel`]) the query runs on `N` shards
//!    concurrently (Rayon) and the partials merge, turning a single-core scan into an
//!    `N`-core one ([`Self::try_query`]).
//! 2. **An unsharded full copy** — for everything the shards can't decompose (joins
//!    that return rows, ordered/limited results, large `SELECT`s, `CONSTRUCT`), the
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

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use opengraph::parallel::{self, ParAnswer, ParClass, ParallelStore};
use oxigraph::sparql::{QueryResults, QuerySolution, QuerySolutionIter, SparqlEvaluator, Variable};
use oxigraph::store::Store;
use tracing::{debug, warn};

/// Floor for the in-memory mirror cap when no explicit override is set. The
/// RAM-aware default ([`ram_aware_default_max_triples`]) never drops below this,
/// preserving the historical behaviour on hosts whose memory budget can't be
/// detected or is small.
const DEFAULT_MAX_TRIPLES: usize = 2_000_000;
/// Absolute ceiling on the auto-derived cap, regardless of how much RAM is
/// available: past this, RocksDB (the reason the store is persistent) is the
/// right tier, and building two ever-larger RAM copies stops paying off.
const ABSOLUTE_MAX_TRIPLES: usize = 24_000_000;
/// Fraction of the detected memory budget the mirror is allowed to use for its
/// two copies (1/`MIRROR_BUDGET_DIVISOR`). Conservative so RocksDB's block
/// cache, the spatial indexes, and transient import buffers still fit.
const MIRROR_BUDGET_DIVISOR: u64 = 4;
/// Deliberately HIGH estimate of the bytes both RAM copies (the subject-hash
/// shards together with the unsharded full store, each carrying several index
/// permutations) cost per triple. Over-estimating errs toward leaving the
/// accelerator OFF (slower but safe) rather than OOM-killing the process — the
/// failure mode that flapped the container during a large seed.
const BYTES_PER_TRIPLE_BOTH_COPIES: u64 = 1024;
/// Hard cap on shard count regardless of core count / configuration.
const MAX_SHARDS: usize = 16;
/// Default quiet period (ms) a write burst must clear before the mirror is
/// (re)built. After any write the two RAM copies are stale; rebuilding them over a
/// multi-million-triple store costs seconds, so doing it on the very next query —
/// then again after the next write — turns a write-heavy phase (the boot seed's
/// vocabulary/metadata writes interleaved with its registry existence checks, or a
/// bulk import) into dozens of back-to-back full-store rebuilds that peg a core for
/// minutes with no client traffic. Holding off until writes go quiet collapses that
/// to a single rebuild; queries in the meantime fall back to the persistent store
/// (correct, just unaccelerated). `0` disables the debounce (rebuild eagerly).
/// Tunable via `OTS_PARALLEL_QUERY_REBUILD_QUIET_MS`.
const DEFAULT_REBUILD_QUIET_MS: u64 = 500;

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
    /// Set once the store is first seen over `max_triples`, so the operator-facing
    /// "accelerator OFF" warning is logged exactly once instead of on every query.
    over_cap_warned: AtomicBool,
    shard_count: usize,
    max_triples: usize,
    enabled: bool,
    /// Monotonic base for the millisecond clock behind the rebuild debounce.
    base: Instant,
    /// Milliseconds since `base` of the most recent write (recorded by `mark_dirty`),
    /// or `0` if there has been none since construction. Read on the query path so a
    /// rebuild is deferred while writes are still churning.
    last_write_ms: AtomicU64,
    /// Quiet period (ms) writes must clear before a (re)build; `0` rebuilds eagerly.
    rebuild_quiet_ms: AtomicU64,
    /// Count of full (re)builds performed — diagnostics + regression guard.
    build_count: AtomicUsize,
}

impl Inner {
    /// Milliseconds elapsed since `base` (monotonic; never goes backwards).
    fn now_ms(&self) -> u64 {
        self.base.elapsed().as_millis() as u64
    }

    /// Whether a write landed within the rebuild quiet period — i.e. the store is
    /// still being actively written and a full mirror rebuild would likely be
    /// invalidated by the next write. `false` before the first write (so the initial
    /// build is never delayed) and when the debounce is disabled (`quiet == 0`).
    fn recently_written(&self) -> bool {
        let quiet = self.rebuild_quiet_ms.load(Ordering::Relaxed);
        if quiet == 0 {
            return false;
        }
        let last = self.last_write_ms.load(Ordering::Acquire);
        if last == 0 {
            return false;
        }
        self.now_ms().saturating_sub(last) < quiet
    }
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
            .unwrap_or_else(ram_aware_default_max_triples);
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
                over_cap_warned: AtomicBool::new(false),
                shard_count: shard_count.clamp(1, MAX_SHARDS),
                max_triples,
                enabled,
                base: Instant::now(),
                last_write_ms: AtomicU64::new(0),
                rebuild_quiet_ms: AtomicU64::new(env_rebuild_quiet_ms()),
                build_count: AtomicUsize::new(0),
            }),
        }
    }

    /// Mark the mirror stale after any write to the persistent store.
    pub fn mark_dirty(&self) {
        // Record the write time first, then flip the dirty flag: a query that
        // observes `dirty == true` is then guaranteed to read a write timestamp at
        // least this fresh, so the debounce in `get_or_build` never rebuilds against
        // a stale "last write" during an active burst.
        self.inner
            .last_write_ms
            .store(self.inner.now_ms().max(1), Ordering::Release);
        self.inner.dirty.store(true, Ordering::Release);
    }

    /// Number of full (re)builds performed since construction. Diagnostics, and the
    /// hook the regression test uses to prove a write burst does not thrash rebuilds.
    pub fn build_count(&self) -> usize {
        self.inner.build_count.load(Ordering::Relaxed)
    }

    /// Override the rebuild quiet period (ms) — used by tests to drive the debounce
    /// deterministically without touching the process-wide environment variable.
    #[cfg(test)]
    fn set_rebuild_quiet_ms(&self, ms: u64) {
        self.inner.rebuild_quiet_ms.store(ms, Ordering::Relaxed);
    }

    /// Try to answer `sparql` in parallel across the shards, returning `None` to
    /// mean "fall back to single-store evaluation" — for a disabled mirror, a
    /// query that is not a decomposable **aggregate** (row-returning SELECTs keep
    /// single-store order), an over-cap store, or any shard error.
    ///
    /// `options` is a factory (called at most once) so the relatively expensive
    /// `QueryOptions` build is skipped entirely for non-accelerable queries.
    pub fn try_query<F>(
        &self,
        store: &Store,
        sparql: &str,
        options: F,
    ) -> Option<QueryResults<'static>>
    where
        F: FnOnce() -> SparqlEvaluator,
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
        // Debounce: while writes are still churning, decline (the persistent store
        // answers this query — correct, just unaccelerated) instead of paying for a
        // full-store rebuild that the next write would immediately invalidate. This
        // is what keeps a write-heavy boot seed (or bulk import) from rebuilding two
        // multi-million-triple RAM copies dozens of times back-to-back; the mirror
        // builds once, on the first query after writes go quiet.
        if self.inner.recently_written() {
            return None;
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
            // An over-cap store silently disabling the accelerator was the cause of
            // a hard-to-spot perf regression (large joins fell back to RocksDB,
            // ~40× slower per row). Surface it ONCE at warn! so it is never silent,
            // naming the knob to raise it. Empty stores are not a regression — keep
            // those at debug!.
            if total > self.inner.max_triples
                && !self.inner.over_cap_warned.swap(true, Ordering::AcqRel)
            {
                warn!(
                    "in-memory query accelerator OFF: store has {total} triples, over the \
                     {} cap — large joins/SELECTs fall back to RocksDB (slower). Raise \
                     OTS_PARALLEL_QUERY_MAX_TRIPLES (and the container memory budget) to \
                     re-enable it; each held triple costs ~2× in RAM.",
                    self.inner.max_triples
                );
            } else {
                debug!(
                    "parallel mirror inactive: {total} triples (cap {})",
                    self.inner.max_triples
                );
            }
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
        self.inner.build_count.fetch_add(1, Ordering::Relaxed);
        // Built successfully (under cap): re-arm the over-cap warning so a later
        // growth back over the cap is surfaced again.
        self.inner.over_cap_warned.store(false, Ordering::Release);
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
    pub fn try_full_query<F>(
        &self,
        store: &Store,
        sparql: &str,
        options: F,
    ) -> Option<QueryResults<'static>>
    where
        F: FnOnce() -> SparqlEvaluator,
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

/// Resolve the rebuild quiet period (ms) from the environment, falling back to
/// [`DEFAULT_REBUILD_QUIET_MS`]. `0` disables the debounce (eager rebuild).
fn env_rebuild_quiet_ms() -> u64 {
    std::env::var("OTS_PARALLEL_QUERY_REBUILD_QUIET_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_REBUILD_QUIET_MS)
}

/// Best-effort detection (Linux) of the memory budget, in bytes, this process is
/// allowed to use: the cgroup limit if one is set (the containerized case that
/// matters for the flap), else total system RAM. Returns `None` when neither can
/// be read (e.g. on non-Linux hosts), where the caller falls back to the fixed
/// floor.
fn detect_memory_limit_bytes() -> Option<u64> {
    // cgroup v2 (Docker default on modern hosts): a numeric byte limit, or the
    // literal "max" when unconstrained.
    if let Ok(s) = std::fs::read_to_string("/sys/fs/cgroup/memory.max") {
        let t = s.trim();
        if t != "max" {
            if let Ok(n) = t.parse::<u64>() {
                if n > 0 && n < u64::MAX / 2 {
                    return Some(n);
                }
            }
        }
    }
    // cgroup v1: reports a huge sentinel (~PAGE_COUNTER_MAX) when unlimited.
    if let Ok(s) = std::fs::read_to_string("/sys/fs/cgroup/memory/memory.limit_in_bytes") {
        if let Ok(n) = s.trim().parse::<u64>() {
            if n > 0 && n < (1u64 << 62) {
                return Some(n);
            }
        }
    }
    // No cgroup limit (or unconstrained) → total system RAM.
    if let Ok(s) = std::fs::read_to_string("/proc/meminfo") {
        for line in s.lines() {
            if let Some(rest) = line.strip_prefix("MemTotal:") {
                if let Ok(kb) = rest.trim().trim_end_matches("kB").trim().parse::<u64>() {
                    return Some(kb.saturating_mul(1024));
                }
            }
        }
    }
    None
}

/// Derive the mirror's default triple cap from the detected memory budget so a
/// large persistent store never tries to build two RAM copies that would OOM the
/// container — the historical cause of the health-check flap during a big seed —
/// while still enabling the accelerator on a roomy host. The explicit
/// `OTS_PARALLEL_QUERY_MAX_TRIPLES` always wins over this; the result is clamped
/// to `[DEFAULT_MAX_TRIPLES, ABSOLUTE_MAX_TRIPLES]`, and falls back to the floor
/// when the budget can't be detected (e.g. non-Linux).
fn ram_aware_default_max_triples() -> usize {
    match detect_memory_limit_bytes() {
        Some(bytes) => {
            let budget = bytes / MIRROR_BUDGET_DIVISOR;
            let cap = (budget / BYTES_PER_TRIPLE_BOTH_COPIES) as usize;
            cap.clamp(DEFAULT_MAX_TRIPLES, ABSOLUTE_MAX_TRIPLES)
        }
        None => DEFAULT_MAX_TRIPLES,
    }
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
    // oxigraph 0.5: the bulk loader stages batches and persists them only on commit().
    let mut loader = full.bulk_loader();
    loader
        .load_quads(store.iter().filter_map(Result::ok))
        .ok()?;
    loader.commit().ok()?;
    Some(full)
}

/// Convert a merged parallel answer back into Oxigraph `QueryResults`, so the
/// caller cannot tell the answer was produced across shards.
fn par_answer_to_results(ans: ParAnswer) -> QueryResults<'static> {
    match ans {
        ParAnswer::Boolean(b) => QueryResults::Boolean(b),
        ParAnswer::Solutions { variables, rows } => {
            let vars: Arc<[Variable]> = Arc::from(variables);
            // oxigraph 0.5: `QuerySolutionIter` yields `QuerySolution`s built from the
            // (variables, values) pair; the iterator owns its data, so it is `'static`.
            let row_vars = vars.clone();
            let iter = rows
                .into_iter()
                .map(move |row| Ok(QuerySolution::from((row_vars.clone(), row))));
            QueryResults::Solutions(QuerySolutionIter::new(vars, iter))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxigraph::model::{GraphName, NamedNode, Quad};
    use std::time::Duration;

    /// A small in-memory store — enough to exercise a real mirror build without the
    /// cost mattering; the regression guard asserts the *number* of builds, not time.
    fn store_with_quads(n: usize) -> Store {
        let store = Store::new().unwrap();
        let p = NamedNode::new("http://example.org/p").unwrap();
        for i in 0..n {
            let s = NamedNode::new(format!("http://example.org/s{i}")).unwrap();
            let o = NamedNode::new(format!("http://example.org/o{i}")).unwrap();
            store
                .insert(&Quad::new(s, p.clone(), o, GraphName::DefaultGraph))
                .unwrap();
        }
        store
    }

    /// The regression guard for the post-seed CPU peg: a write-heavy burst (the boot
    /// seed interleaves registry/metadata writes with existence-check queries) must
    /// NOT rebuild the full-store mirror on every query. Before the debounce this
    /// loop rebuilt two full RAM copies 50 times; now it builds zero times until
    /// writes go quiet, then exactly once, then reuses the warm copy.
    #[test]
    fn write_burst_does_not_thrash_rebuilds() {
        let mirror = ParallelMirror::new(true, 2, 10_000_000);
        mirror.set_rebuild_quiet_ms(120);
        let store = store_with_quads(200);
        let q = "SELECT * WHERE { ?s ?p ?o } LIMIT 1";

        // Interleaved writes + queries with no quiet gap → every query declines and
        // falls back to the persistent store; the mirror is never (re)built.
        for _ in 0..50 {
            mirror.mark_dirty();
            assert!(
                mirror
                    .try_full_query(&store, q, SparqlEvaluator::new)
                    .is_none(),
                "a query during an active write burst must decline (fall back), not rebuild",
            );
        }
        assert_eq!(
            mirror.build_count(),
            0,
            "writes still churning → no full-store rebuild (was 50 before the debounce)",
        );

        // Writes go quiet → the next query builds the mirror exactly once.
        std::thread::sleep(Duration::from_millis(200));
        assert!(mirror
            .try_full_query(&store, q, SparqlEvaluator::new)
            .is_some());
        assert_eq!(mirror.build_count(), 1, "one build after writes quiesce");

        // Steady read-only traffic reuses the warm copy — no further rebuilds.
        for _ in 0..50 {
            assert!(mirror
                .try_full_query(&store, q, SparqlEvaluator::new)
                .is_some());
        }
        assert_eq!(
            mirror.build_count(),
            1,
            "clean mirror reused on every read, not rebuilt",
        );
    }

    /// The env escape hatch (`OTS_PARALLEL_QUERY_REBUILD_QUIET_MS=0`) restores the
    /// eager behaviour: the first query after a write rebuilds immediately.
    #[test]
    fn quiet_zero_rebuilds_eagerly() {
        let mirror = ParallelMirror::new(true, 2, 10_000_000);
        mirror.set_rebuild_quiet_ms(0);
        let store = store_with_quads(50);
        let q = "SELECT * WHERE { ?s ?p ?o } LIMIT 1";

        mirror.mark_dirty();
        assert!(mirror
            .try_full_query(&store, q, SparqlEvaluator::new)
            .is_some());
        assert_eq!(mirror.build_count(), 1);
    }

    /// With no write since construction the initial build is never debounced, so a
    /// cold first query (e.g. the first viewer-feed after boot settles) builds at
    /// once rather than waiting out a quiet period.
    #[test]
    fn first_build_is_not_debounced() {
        let mirror = ParallelMirror::new(true, 2, 10_000_000);
        mirror.set_rebuild_quiet_ms(60_000); // huge window; must not delay first build
        let store = store_with_quads(50);
        let q = "SELECT * WHERE { ?s ?p ?o } LIMIT 1";

        assert!(mirror
            .try_full_query(&store, q, SparqlEvaluator::new)
            .is_some());
        assert_eq!(mirror.build_count(), 1);
    }
}
