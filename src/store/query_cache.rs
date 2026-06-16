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
use oxigraph::sparql::{EvaluationError, QueryResults, QuerySolution, QuerySolutionIter, Variable};

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
                // oxigraph 0.5: `QuerySolutionIter` yields `QuerySolution` (not raw
                // `Vec<Option<Term>>`), built from the (variables, values) pair. The
                // iterator owns its cloned data, so the result is `'static`.
                let vars = vars.clone();
                let rows = rows.clone();
                let row_vars = vars.clone();
                let iter = (0..rows.len())
                    .map(move |i| Ok(QuerySolution::from((row_vars.clone(), rows[i].clone()))));
                QueryResults::Solutions(QuerySolutionIter::new(vars, iter))
            }
        }
    }
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

    /// Materialise `results`, caching it if it is cacheable and small enough, and
    /// return the (reconstructed) full results either way — so the caller streams
    /// the same data whether or not it was cached.
    pub fn put(&self, sparql: &str, results: QueryResults<'static>) -> QueryResults<'static> {
        if !self.inner.enabled || !is_cacheable(sparql) {
            return results;
        }
        let gen = self.inner.generation.load(Ordering::Acquire);
        match results {
            QueryResults::Boolean(b) => {
                self.store(sparql, gen, Cached::Boolean(b));
                QueryResults::Boolean(b)
            }
            QueryResults::Solutions(mut sols) => {
                let vars: Arc<[Variable]> = Arc::from(sols.variables().to_vec());
                // Pull up to max_rows+1 rows (so overflow is detectable) while
                // preserving any mid-stream error.
                let mut buf: Vec<Result<Vec<Option<Term>>, EvaluationError>> = Vec::new();
                let mut exhausted = false;
                let mut errored = false;
                loop {
                    match sols.next() {
                        None => {
                            exhausted = true;
                            break;
                        }
                        Some(Ok(sol)) => {
                            buf.push(Ok(vars.iter().map(|v| sol.get(v).cloned()).collect()));
                            if buf.len() > self.inner.max_rows {
                                break; // over the cap
                            }
                        }
                        Some(Err(e)) => {
                            buf.push(Err(e));
                            errored = true;
                            break;
                        }
                    }
                }

                if exhausted && !errored && buf.len() <= self.inner.max_rows {
                    // Small, complete, error-free → cache it and reconstruct.
                    let rows: Arc<[Vec<Option<Term>>]> =
                        buf.into_iter().map(|r| r.unwrap_or_default()).collect();
                    self.store(
                        sparql,
                        gen,
                        Cached::Solutions {
                            vars: vars.clone(),
                            rows: rows.clone(),
                        },
                    );
                    let row_vars = vars.clone();
                    let iter = (0..rows.len())
                        .map(move |i| Ok(QuerySolution::from((row_vars.clone(), rows[i].clone()))));
                    QueryResults::Solutions(QuerySolutionIter::new(vars, iter))
                } else {
                    // Over the cap or errored → don't cache; stream the buffered prefix
                    // chained with the rest of the live iterator. oxigraph 0.5's live
                    // `sols` already yield `QuerySolution`s, so only the buffered raw
                    // value rows need rebuilding into solutions.
                    let head_vars = vars.clone();
                    let head = buf
                        .into_iter()
                        .map(move |r| r.map(|row| QuerySolution::from((head_vars.clone(), row))));
                    let iter = head.chain(sols);
                    QueryResults::Solutions(QuerySolutionIter::new(vars, iter))
                }
            }
            // CONSTRUCT/DESCRIBE graphs are not cached.
            other => other,
        }
    }

    fn store(&self, sparql: &str, gen: u64, value: Cached) {
        if let Ok(mut cache) = self.inner.cache.lock() {
            cache.put(sparql.to_string(), (gen, value));
        }
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
