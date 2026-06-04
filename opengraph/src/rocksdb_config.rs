//! RocksDB tuning configuration for the Oxigraph store.
//!
//! Oxigraph 0.4 uses RocksDB internally.  While the storage engine is not
//! currently exposed for direct configuration, this module documents the
//! optimal settings and provides a configuration builder that can be applied
//! when Oxigraph exposes store options (planned in opengraph fork).
//!
//! # Effect of each setting
//!
//! | Setting | Default | Recommended | Effect |
//! |---------|---------|-------------|--------|
//! | `block_cache_mb` | 8 MB | 512–4096 MB | LRU block cache reduces I/O on repeated reads |
//! | `bloom_bits_per_key` | 10 | 10–15 | Bloom filter reduces negative lookups by ~95% |
//! | `compression` | Snappy | LZ4 | LZ4 is ~30% faster decompression with similar ratio |
//! | `write_buffer_mb` | 64 MB | 128–512 MB | Larger buffer reduces L0→L1 compaction frequency |
//! | `max_write_buffer_number` | 2 | 3–4 | Allow more concurrent memtables |
//! | `level0_slowdown` | 20 | 40 | Delay writes less aggressively |
//! | `level0_stop` | 36 | 48 | Stop writes only at higher file count |
//!
//! # Workload-specific recommendations
//!
//! ## Bulk ingest (write-heavy)
//! ```text
//! write_buffer_mb: 512
//! max_write_buffer_number: 4
//! bloom_bits_per_key: 0  // disable during bulk load; re-enable after
//! ```
//!
//! ## SPARQL query (read-heavy)
//! ```text
//! block_cache_mb: 2048
//! bloom_bits_per_key: 15
//! compression: LZ4
//! ```
//!
//! ## Mixed workload
//! ```text
//! block_cache_mb: 1024
//! bloom_bits_per_key: 10
//! write_buffer_mb: 128
//! ```

/// Compression algorithm for RocksDB SST files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    /// No compression (fastest CPU, most disk).
    None,
    /// Snappy (Oxigraph default) — moderate speed and ratio.
    Snappy,
    /// LZ4 — 30% faster decompression than Snappy at similar ratio (recommended).
    Lz4,
    /// Zstd — best ratio for archival; 2–3× slower decompression than LZ4.
    Zstd,
}

/// Workload profile for auto-selecting tuning parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkloadProfile {
    /// Maximise bulk ingest throughput (SPARQL load, ETL pipelines).
    BulkIngest,
    /// Maximise SPARQL query performance (read-heavy production).
    QueryHeavy,
    /// Balance read and write performance.
    Mixed,
}

/// RocksDB tuning configuration.
///
/// Build with [`RocksDbConfig::for_workload`] or configure individual fields.
#[derive(Debug, Clone)]
pub struct RocksDbConfig {
    /// LRU block cache size in megabytes.  Set to ~50% of available RAM for
    /// read-heavy workloads, 10–20% for write-heavy.
    pub block_cache_mb: usize,
    /// Bloom filter bits per key.  Higher values reduce false positives at the
    /// cost of memory.  10–12 is optimal for most workloads.  Set to 0 to
    /// disable (useful during bulk ingest to reduce memory pressure).
    pub bloom_bits_per_key: u8,
    /// Compression algorithm for SST files.
    pub compression: Compression,
    /// Memtable (write buffer) size in megabytes per column family.
    pub write_buffer_mb: usize,
    /// Maximum number of concurrent memtables before a flush is forced.
    pub max_write_buffer_number: u8,
    /// Number of background compaction threads.
    pub compaction_threads: u8,
    /// L0 file count at which writes are slowed down.
    pub level0_slowdown_writes_trigger: u32,
    /// L0 file count at which writes are stopped.
    pub level0_stop_writes_trigger: u32,
}

impl RocksDbConfig {
    /// Build a configuration optimised for the given workload profile.
    pub fn for_workload(profile: WorkloadProfile) -> Self {
        match profile {
            WorkloadProfile::BulkIngest => Self {
                block_cache_mb: 256,
                bloom_bits_per_key: 0,
                compression: Compression::Lz4,
                write_buffer_mb: 512,
                max_write_buffer_number: 4,
                compaction_threads: 4,
                level0_slowdown_writes_trigger: 40,
                level0_stop_writes_trigger: 56,
            },
            WorkloadProfile::QueryHeavy => Self {
                block_cache_mb: 2048,
                bloom_bits_per_key: 12,
                compression: Compression::Lz4,
                write_buffer_mb: 128,
                max_write_buffer_number: 2,
                compaction_threads: 2,
                level0_slowdown_writes_trigger: 20,
                level0_stop_writes_trigger: 36,
            },
            WorkloadProfile::Mixed => Self::default(),
        }
    }

    /// Return estimated memory usage in megabytes.
    pub fn estimated_memory_mb(&self) -> usize {
        self.block_cache_mb + (self.write_buffer_mb * self.max_write_buffer_number as usize)
    }

    /// Generate a human-readable tuning summary.
    pub fn summary(&self) -> String {
        format!(
            "RocksDB config: block_cache={}MB bloom={} bits/key compression={:?} \
             write_buffer={}MB×{} compaction_threads={} estimated_mem={}MB",
            self.block_cache_mb,
            self.bloom_bits_per_key,
            self.compression,
            self.write_buffer_mb,
            self.max_write_buffer_number,
            self.compaction_threads,
            self.estimated_memory_mb(),
        )
    }
}

impl Default for RocksDbConfig {
    fn default() -> Self {
        Self {
            block_cache_mb: 1024,
            bloom_bits_per_key: 10,
            compression: Compression::Lz4,
            write_buffer_mb: 128,
            max_write_buffer_number: 3,
            compaction_threads: 2,
            level0_slowdown_writes_trigger: 20,
            level0_stop_writes_trigger: 36,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bulk_ingest_disables_bloom() {
        let cfg = RocksDbConfig::for_workload(WorkloadProfile::BulkIngest);
        assert_eq!(
            cfg.bloom_bits_per_key, 0,
            "Bloom filter should be off during bulk ingest"
        );
        assert!(
            cfg.write_buffer_mb >= 256,
            "Write buffer should be large for bulk ingest"
        );
    }

    #[test]
    fn test_query_heavy_has_large_cache() {
        let cfg = RocksDbConfig::for_workload(WorkloadProfile::QueryHeavy);
        assert!(
            cfg.block_cache_mb >= 1024,
            "Block cache should be large for query-heavy"
        );
        assert!(
            cfg.bloom_bits_per_key >= 10,
            "Bloom filter should be enabled for queries"
        );
    }

    #[test]
    fn test_estimated_memory() {
        let cfg = RocksDbConfig {
            block_cache_mb: 1000,
            write_buffer_mb: 100,
            max_write_buffer_number: 3,
            ..Default::default()
        };
        assert_eq!(cfg.estimated_memory_mb(), 1300);
    }

    #[test]
    fn test_summary_contains_key_fields() {
        let cfg = RocksDbConfig::default();
        let s = cfg.summary();
        assert!(s.contains("block_cache"));
        assert!(s.contains("bloom"));
        assert!(s.contains("compression"));
    }

    #[test]
    fn test_default_is_mixed_workload() {
        let default = RocksDbConfig::default();
        let mixed = RocksDbConfig::for_workload(WorkloadProfile::Mixed);
        assert_eq!(default.block_cache_mb, mixed.block_cache_mb);
    }
}
