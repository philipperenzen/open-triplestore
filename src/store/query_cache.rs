//! Query-result cache — memoises the *result* of a SPARQL read so a repeated
//! query (dashboards, APIs, paged browsing) is answered from a small LRU instead
//! of being re-evaluated. It is a pure performance layer: a hit returns the exact
//! same result the engine would compute, so fidelity and standards-compliance are
//! never traded.
//!
//! ## Three correctness invariants
//!
//! 1. **Cross-tenant safe.** The key is the SPARQL string *as it reaches
//!    `TripleStore::query`* — i.e. already ACL-scoped (the HTTP layer injects
//!    `FROM <readable graphs>` before calling). Two principals with different
//!    readable graphs therefore produce different strings → different cache
//!    entries; two principals who legitimately see the same graphs share an entry
//!    and the same correct result. No scope is ever crossed.
//! 2. **Never stale.** A monotonic generation counter is bumped on *every* write
//!    (wired into `TripleStore`'s write paths alongside the mirror invalidation).
//!    A cached entry records the generation it was computed at; a read at a newer
//!    generation is a miss and recomputes. So any write invalidates everything —
//!    coarse but always correct.
//! 3. **Deterministic queries only.** Queries calling `RAND`/`NOW`/`UUID`/
//!    `STRUUID`/`BNODE` are *never* cached (their value changes between calls);
//!    caching them would freeze a timestamp or random value. The check errs toward
//!    not caching, never toward caching something unsafe.
//!
//! Results larger than the row cap are not cached (and streamed through without
//! buffering the whole thing); `CONSTRUCT`/`DESCRIBE` graphs are not cached (the
//! cache targets the small, expensive aggregate/`ASK`/lookup results that dominate
//! real traffic).

use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use lru::LruCache;
use oxigraph::model::Term;
use oxigraph::sparql::{
    QueryEvaluationError, QueryResults, QuerySolution, QuerySolutionIter, Variable,
};

const DEFAULT_MAX_ENTRIES: usize = 1024;
const DEFAULT_MAX_ROWS: usize = 10_000;

/// A materialised, cheaply-cloneable query result.
#[derive(Clone)]
enum Cached {
    Boolean(bool),
    Solutions {
        vars: Arc<[Variable]>,
        rows: Arc<[Vec<Option<Term>>]>,
    },
}

impl Cached {
    fn to_results(&self) -> QueryResults<'static> {
        match self {
            Cached::Boolean(b) => QueryResults::Boolean(*b),
            Cached::Solutions { vars, rows } => {
                QueryResults::Solutions(replay(vars.clone(), rows.clone()))
            }
        }
    }
}

/// Re-emit stored value rows as an owned, `'static` result stream.
///
/// This is the cache-*hit* path — the hot one, since a hit is the whole point — so
/// it does the minimum: clone the row and pair it with the shared variable list.
/// oxigraph 0.5's `QuerySolutionIter` yields `QuerySolution` (not raw value rows),
/// hence the rebuild; owning its data is what makes the result `'static` and lets
/// the cache keep its copy. Rows are stored already in `vars` order (`put` does that
/// once, on the way in), so nothing here needs to inspect variables.
fn replay(vars: Arc<[Variable]>, rows: Arc<[Vec<Option<Term>>]>) -> QuerySolutionIter<'static> {
    let row_vars = Arc::clone(&vars);
    let iter = (0..rows.len()).map(move |i| {
        Ok(QuerySolution::from((
            Arc::clone(&row_vars),
            rows[i].clone(),
        )))
    });
    QuerySolutionIter::new(vars, iter)
}

/// The cache, shared (`Arc`) inside `TripleStore`.
#[derive(Clone)]
pub struct QueryCache {
    inner: Arc<Inner>,
}

struct Inner {
    enabled: bool,
    generation: AtomicU64,
    max_rows: usize,
    cache: Mutex<LruCache<String, (u64, Cached)>>,
}

impl QueryCache {
    /// Build from environment configuration:
    ///   * `OTS_QUERY_CACHE`           — `0`/`false`/`off`/`no` disables it (default on)
    ///   * `OTS_QUERY_CACHE_ENTRIES`   — max cached queries (default 1024)
    ///   * `OTS_QUERY_CACHE_MAX_ROWS`  — max rows per cached result (default 10000)
    pub fn from_env() -> Self {
        let enabled = std::env::var("OTS_QUERY_CACHE")
            .map(|v| {
                !matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "0" | "false" | "off" | "no"
                )
            })
            .unwrap_or(true);
        let entries = std::env::var("OTS_QUERY_CACHE_ENTRIES")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(DEFAULT_MAX_ENTRIES);
        let max_rows = std::env::var("OTS_QUERY_CACHE_MAX_ROWS")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(DEFAULT_MAX_ROWS);
        Self::new(enabled, entries, max_rows)
    }

    pub fn new(enabled: bool, max_entries: usize, max_rows: usize) -> Self {
        let cap = NonZeroUsize::new(max_entries.max(1)).unwrap();
        Self {
            inner: Arc::new(Inner {
                enabled,
                generation: AtomicU64::new(0),
                max_rows,
                cache: Mutex::new(LruCache::new(cap)),
            }),
        }
    }

    /// Bump the generation so every existing entry is treated as stale. Called on
    /// every write to the store. O(1).
    pub fn invalidate(&self) {
        if self.inner.enabled {
            self.inner.generation.fetch_add(1, Ordering::Release);
        }
    }

    /// Return a *fresh* (current-generation) cached result, or `None`.
    pub fn get(&self, sparql: &str) -> Option<QueryResults<'static>> {
        if !self.inner.enabled {
            return None;
        }
        let gen = self.inner.generation.load(Ordering::Acquire);
        let mut cache = self.inner.cache.lock().ok()?;
        match cache.get(sparql) {
            Some((g, cached)) if *g == gen => Some(cached.to_results()),
            _ => None,
        }
    }

    /// The current generation. Callers snapshot this *before* evaluating a
    /// query and hand it back to [`Self::put`], so a write that lands during
    /// evaluation invalidates the result instead of being stamped onto it.
    pub fn generation(&self) -> u64 {
        self.inner.generation.load(Ordering::Acquire)
    }

    /// Materialise `results`, caching it if it is cacheable and small enough, and
    /// return the (reconstructed) full results either way — so the caller streams
    /// the same data whether or not it was cached.
    ///
    /// `gen_at_start` must be the generation observed before evaluation began.
    /// Reading the generation here instead was a lost-update race against the
    /// module's own "never stale" invariant: thread A computes a result from the
    /// pre-write index, thread B commits a write and bumps the generation, then
    /// A's `put` reads the NEW generation and stores (new_gen, old_value) — a
    /// stale answer indistinguishable from a fresh one, served until the next
    /// write. `try_fast_count` and the parallel-mirror paths materialise eagerly,
    /// which makes the window easy to hit.
    pub fn put(
        &self,
        sparql: &str,
        gen_at_start: u64,
        results: QueryResults<'static>,
    ) -> QueryResults<'static> {
        if !self.inner.enabled || !is_cacheable(sparql) {
            return results;
        }
        let gen = gen_at_start;
        match results {
            QueryResults::Boolean(b) => {
                self.store(sparql, gen, Cached::Boolean(b));
                QueryResults::Boolean(b)
            }
            QueryResults::Solutions(mut sols) => {
                let vars: Arc<[Variable]> = Arc::from(sols.variables().to_vec());
                // Pull up to max_rows+1 solutions (so overflow is detectable) while
                // preserving any mid-stream error.
                //
                // Buffer the `QuerySolution`s the engine hands us, untouched, and
                // decompose them into value rows only if it turns out we can cache
                // them. This used to decompose on the way in, before knowing — so a
                // result over the cap paid a deep copy of every term in its first
                // max_rows+1 rows, only to rebuild the very solutions it had just
                // taken apart. A 100k-row scan did that 10001 times for nothing.
                let mut buf: Vec<QuerySolution> = Vec::new();
                let mut exhausted = false;
                let mut error: Option<QueryEvaluationError> = None;
                loop {
                    match sols.next() {
                        None => {
                            exhausted = true;
                            break;
                        }
                        Some(Ok(sol)) => {
                            buf.push(sol);
                            if buf.len() > self.inner.max_rows {
                                break; // over the cap
                            }
                        }
                        Some(Err(e)) => {
                            error = Some(e);
                            break;
                        }
                    }
                }

                if exhausted && error.is_none() && buf.len() <= self.inner.max_rows {
                    // Small, complete, error-free → decompose once, cache, replay.
                    let rows: Arc<[Vec<Option<Term>>]> =
                        buf.iter().map(|sol| row_values(sol, &vars)).collect();
                    self.store(
                        sparql,
                        gen,
                        Cached::Solutions {
                            vars: vars.clone(),
                            rows: rows.clone(),
                        },
                    );
                    QueryResults::Solutions(replay(vars, rows))
                } else {
                    // Over the cap or errored → don't cache; stream the buffered
                    // solutions unchanged, then the error (if any), then the rest of
                    // the live iterator. Nothing is rebuilt: oxigraph 0.5's `sols`
                    // already yields owned `QuerySolution`s, so the buffered prefix
                    // passes straight through.
                    let iter = buf
                        .into_iter()
                        .map(Ok)
                        .chain(error.into_iter().map(Err))
                        .chain(sols);
                    QueryResults::Solutions(QuerySolutionIter::new(vars, iter))
                }
            }
            // CONSTRUCT/DESCRIBE graphs are not cached.
            other => other,
        }
    }

    fn store(&self, sparql: &str, gen: u64, value: Cached) {
        // A write landed while this query was being evaluated, so the value is
        // already out of date. Storing it under its start generation would be
        // harmless (`get` compares against the current one and would miss), but
        // it would still evict a live entry from the LRU for nothing.
        if self.inner.generation.load(Ordering::Acquire) != gen {
            return;
        }
        if let Ok(mut cache) = self.inner.cache.lock() {
            cache.put(sparql.to_string(), (gen, value));
        }
    }
}

/// A solution's values in `vars` order — the once-per-result decomposition `put`
/// does before caching, never the per-hit path (see `replay`).
///
/// A `QuerySolution` carries its own ordered variable list, and the evaluator builds
/// every solution in a result against the same list `QuerySolutionIter::variables()`
/// reports — so `sol.values()` is normally already in `vars` order and copying the
/// slice is all that is needed. `QuerySolutionIter::new` does not *enforce* that, so
/// the orders are compared rather than assumed, and anything else falls back to a
/// per-variable lookup.
///
/// The comparison costs less than the lookup it replaces: `QuerySolution::get`
/// resolves a `&Variable` by scanning the solution's variable list and comparing
/// variable names, so the lookup form is `vars.len()` scans of up to `vars.len()`
/// name comparisons per row — quadratic in the projection width — against one linear
/// slice comparison here.
fn row_values(sol: &QuerySolution, vars: &Arc<[Variable]>) -> Vec<Option<Term>> {
    if sol.variables() == &vars[..] {
        sol.values().to_vec()
    } else {
        vars.iter().map(|v| sol.get(v).cloned()).collect()
    }
}

/// True unless the query calls a non-deterministic SPARQL function whose value
/// changes between executions (`RAND`/`NOW`/`UUID`/`STRUUID`/`BNODE`). A
/// conservative token scan: it matches a keyword only at a word boundary followed
/// by `(`, so it errs toward *not* caching (a false positive is a missed cache, a
/// false negative — caching something non-deterministic — never happens for these).
fn is_cacheable(sparql: &str) -> bool {
    const NONDET: &[&str] = &["rand", "now", "uuid", "struuid", "bnode"];
    let lower = sparql.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    for kw in NONDET {
        let mut from = 0;
        while let Some(off) = lower[from..].find(kw) {
            let i = from + off;
            let end = i + kw.len();
            let before_ok = i == 0 || !is_word(bytes[i - 1]);
            let after_boundary = bytes.get(end).is_none_or(|&b| !is_word(b));
            if before_ok && after_boundary {
                let mut j = end;
                while bytes.get(j).is_some_and(|b| b.is_ascii_whitespace()) {
                    j += 1;
                }
                if bytes.get(j) == Some(&b'(') {
                    return false;
                }
            }
            from = end;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A result computed before a write must never be cached as if it were
    /// computed after it.
    ///
    /// `put` used to read the generation counter itself, so this exact interleave
    /// — evaluate, write commits, store — stamped the post-write generation onto
    /// the pre-write value, and `get` then happily served it as fresh. The race
    /// is too narrow to reproduce reliably by running threads, so it is pinned
    /// here directly: take the snapshot, bump the generation, then put.
    #[test]
    fn a_result_computed_before_a_write_is_not_cached_as_fresh() {
        let cache = QueryCache::new(true, 16, 1000);
        let q = "ASK { ?s ?p ?o }";

        let gen_at_start = cache.generation();
        // A write commits while the query is being evaluated.
        cache.invalidate();
        let _ = cache.put(q, gen_at_start, QueryResults::Boolean(true));

        assert!(
            cache.get(q).is_none(),
            "a value computed before an interleaved write must not be served as fresh"
        );
    }

    /// The ordinary path still caches: no write, so the snapshot is current.
    #[test]
    fn a_result_with_no_interleaved_write_is_cached() {
        let cache = QueryCache::new(true, 16, 1000);
        let q = "ASK { ?s ?p ?o }";

        let gen_at_start = cache.generation();
        let _ = cache.put(q, gen_at_start, QueryResults::Boolean(true));

        assert!(
            matches!(cache.get(q), Some(QueryResults::Boolean(true))),
            "an uncontended result must still be cached"
        );
    }
}
