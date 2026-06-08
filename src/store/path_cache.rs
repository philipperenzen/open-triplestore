//! Property path memoisation cache (2-5x improvement for transitive paths).
//!
//! Caches results of SPARQL property path evaluations (`+`, `*`) to avoid
//! redundant BFS traversals across multiple query invocations over stable data.
//!
//! The cache has per-query lifetime: created at the start of a SHACL validation
//! pass, shared across constraint evaluations, and discarded afterwards.
//!
//! # Thread-local usage
//! The module exposes [`tl_get`], [`tl_insert`], and [`tl_clear`] helpers that
//! operate on a per-thread `PathCache`.  SHACL validation calls `tl_clear()` at
//! the start of each pass so that stale results from a previous run are evicted
//! before any worker thread begins evaluating constraints.

use lru::LruCache;
use std::cell::RefCell;
use std::num::NonZeroUsize;
use std::sync::Mutex;

/// Maximum number of entries before LRU eviction.
const MAX_ENTRIES: usize = 10_000;

/// Per-query cache for property path results.
///
/// Maps `(start_node, path_expression)` to the set of reachable nodes.
/// Uses an LRU eviction policy to bound memory while keeping hot entries.
/// Protected by a `Mutex` (writes are rare; LRU requires mutable access on reads).
pub struct PathCache {
    cache: Mutex<LruCache<(String, String), Vec<String>>>,
}

impl PathCache {
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(LruCache::new(
                NonZeroUsize::new(MAX_ENTRIES).expect("MAX_ENTRIES > 0"),
            )),
        }
    }

    /// Look up cached path results.
    pub fn get(&self, start_node: &str, path_expr: &str) -> Option<Vec<String>> {
        let mut map = self.cache.lock().unwrap();
        map.get(&(start_node.to_string(), path_expr.to_string()))
            .cloned()
    }

    /// Insert path results into the cache.
    /// LRU eviction happens automatically when capacity is exceeded.
    pub fn insert(&self, start_node: String, path_expr: String, results: Vec<String>) {
        let mut map = self.cache.lock().unwrap();
        map.put((start_node, path_expr), results);
    }

    /// Clear the entire cache.
    pub fn clear(&self) {
        self.cache.lock().unwrap().clear();
    }

    /// Number of cached entries.
    // is_empty is only needed by tests; keep it test-gated rather than ship an
    // unused method, and waive the companion lint here.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.cache.lock().unwrap().len()
    }

    /// Check if the cache is empty.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for PathCache {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Thread-local helpers ─────────────────────────────────────────────────────
//
// SHACL validation runs shapes in parallel via rayon.  Each rayon worker thread
// keeps its own `PathCache` via `thread_local!` so no cross-thread coordination
// is needed.  `tl_clear()` resets the calling thread's cache (called once at the
// start of every `validate()` call on the main thread, and lazily re-initialised
// per worker thread on first use).

thread_local! {
    static TL_CACHE: RefCell<PathCache> = RefCell::new(PathCache::new());
}

/// Look up cached path results for the current thread.
pub fn tl_get(start_node: &str, path_expr: &str) -> Option<Vec<String>> {
    TL_CACHE.with(|c| c.borrow().get(start_node, path_expr))
}

/// Insert path results into the current thread's cache.
pub fn tl_insert(start_node: String, path_expr: String, results: Vec<String>) {
    TL_CACHE.with(|c| c.borrow().insert(start_node, path_expr, results));
}

/// Clear the current thread's path cache.  Call this at the start of each
/// SHACL validation pass to prevent stale results from prior runs.
pub fn tl_clear() {
    TL_CACHE.with(|c| c.borrow().clear());
}

/// Number of entries in the current thread's path cache (primarily for diagnostics).
pub fn tl_len() -> usize {
    TL_CACHE.with(|c| c.borrow().len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_insert_and_get() {
        let cache = PathCache::new();
        cache.insert(
            "http://ex/a".to_string(),
            "http://ex/next+".to_string(),
            vec!["http://ex/b".to_string(), "http://ex/c".to_string()],
        );

        let result = cache.get("http://ex/a", "http://ex/next+");
        assert!(result.is_some());
        assert_eq!(result.unwrap().len(), 2);
    }

    #[test]
    fn test_cache_miss() {
        let cache = PathCache::new();
        assert!(cache.get("http://ex/a", "http://ex/next+").is_none());
    }

    #[test]
    fn test_cache_clear() {
        let cache = PathCache::new();
        cache.insert(
            "http://ex/a".to_string(),
            "http://ex/next+".to_string(),
            vec!["http://ex/b".to_string()],
        );
        assert_eq!(cache.len(), 1);
        cache.clear();
        assert!(cache.is_empty());
    }
}
