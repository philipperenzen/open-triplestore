//! MVCC (Multi-Version Concurrency Control) read snapshot utilities.
//!
//! RocksDB — the storage engine underlying Oxigraph — natively supports
//! MVCC via snapshots: a `Snapshot` captures the database state at a point in
//! time, allowing reads to proceed without blocking on concurrent writes.
//!
//! # Current state
//!
//! Oxigraph 0.5 binds **one storage snapshot per query**:
//! `PreparedSparqlQuery::on_store` takes `storage().snapshot()` once and every
//! iterator of the evaluation reads through that snapshot, so a `SELECT`
//! joining several triple patterns sees one committed state for its whole
//! duration — on RocksDB and on the in-memory backend alike. Writers are never
//! blocked by readers (RocksDB MVCC is copy-on-write).
//!
//! An earlier version of this comment claimed a snapshot per *iterator*, i.e.
//! that a multi-pattern `SELECT` could observe a partially applied write.
//! `tests/read_isolation.rs` races a writer that flips two properties of
//! every subject in one transaction against a reader joining the two: no
//! torn read is observable on either backend, so the claim was wrong for the
//! engine in use and has been removed.
//!
//! # Utilities in this module
//!
//! - [`ReadIsolationLevel`] — names the isolation guarantee a backend gives.
//! - [`ConcurrencyStats`] — tracks concurrent readers and writers (useful for
//!   monitoring dashboards and adaptive rate limiting).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Read isolation guarantees for SPARQL queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadIsolationLevel {
    /// Each index scan sees its own snapshot; a query with several patterns
    /// could observe a partially applied write. Not what oxigraph 0.5 does —
    /// kept as the name of the weaker guarantee.
    PerIterator,
    /// A single snapshot covers the entire query (oxigraph 0.5 behaviour, see
    /// the module docs): full read-committed isolation with no reader-writer
    /// blocking.
    PerQuery,
}

/// Lightweight concurrency monitor.
///
/// Tracks active readers and writers to help detect contention and tune
/// the read/write ratio for capacity planning.
#[derive(Clone, Default)]
pub struct ConcurrencyStats {
    active_readers: Arc<AtomicU64>,
    active_writers: Arc<AtomicU64>,
    total_reads: Arc<AtomicU64>,
    total_writes: Arc<AtomicU64>,
}

impl ConcurrencyStats {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the start of a read operation. Returns a guard that decrements
    /// on drop.
    pub fn read_start(&self) -> ReadGuard {
        self.active_readers.fetch_add(1, Ordering::Relaxed);
        self.total_reads.fetch_add(1, Ordering::Relaxed);
        ReadGuard {
            counter: Arc::clone(&self.active_readers),
        }
    }

    /// Record the start of a write operation. Returns a guard that decrements
    /// on drop.
    pub fn write_start(&self) -> WriteGuard {
        self.active_writers.fetch_add(1, Ordering::Relaxed);
        self.total_writes.fetch_add(1, Ordering::Relaxed);
        WriteGuard {
            counter: Arc::clone(&self.active_writers),
        }
    }

    pub fn active_readers(&self) -> u64 {
        self.active_readers.load(Ordering::Relaxed)
    }

    pub fn active_writers(&self) -> u64 {
        self.active_writers.load(Ordering::Relaxed)
    }

    pub fn total_reads(&self) -> u64 {
        self.total_reads.load(Ordering::Relaxed)
    }

    pub fn total_writes(&self) -> u64 {
        self.total_writes.load(Ordering::Relaxed)
    }

    /// Compute the read/write ratio. Returns `f64::INFINITY` when there are no
    /// writes.
    pub fn read_write_ratio(&self) -> f64 {
        let w = self.total_writes() as f64;
        if w == 0.0 {
            f64::INFINITY
        } else {
            self.total_reads() as f64 / w
        }
    }
}

/// RAII guard that decrements the active-reader counter on drop.
pub struct ReadGuard {
    counter: Arc<AtomicU64>,
}

impl Drop for ReadGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

/// RAII guard that decrements the active-writer counter on drop.
pub struct WriteGuard {
    counter: Arc<AtomicU64>,
}

impl Drop for WriteGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_guard_increments_and_decrements() {
        let stats = ConcurrencyStats::new();
        assert_eq!(stats.active_readers(), 0);
        {
            let _g = stats.read_start();
            assert_eq!(stats.active_readers(), 1);
        }
        assert_eq!(stats.active_readers(), 0);
    }

    #[test]
    fn test_write_guard() {
        let stats = ConcurrencyStats::new();
        let _g = stats.write_start();
        assert_eq!(stats.active_writers(), 1);
        drop(_g);
        assert_eq!(stats.active_writers(), 0);
    }

    #[test]
    fn test_totals_accumulate() {
        let stats = ConcurrencyStats::new();
        let _r1 = stats.read_start();
        let _r2 = stats.read_start();
        drop(_r1);
        drop(_r2);
        assert_eq!(stats.total_reads(), 2);
    }

    #[test]
    fn test_read_write_ratio() {
        let stats = ConcurrencyStats::new();
        // No writes yet — ratio should be infinity
        assert_eq!(stats.read_write_ratio(), f64::INFINITY);

        let _w = stats.write_start();
        drop(_w);
        let _r = stats.read_start();
        drop(_r);
        // 1 read, 1 write → ratio = 1.0
        assert!((stats.read_write_ratio() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_isolation_level_enum() {
        assert_ne!(
            ReadIsolationLevel::PerIterator,
            ReadIsolationLevel::PerQuery
        );
    }
}
