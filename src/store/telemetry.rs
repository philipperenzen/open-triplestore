//! Workload telemetry: which exit of the query path answers and how fast,
//! what shape the queries have, what validation runs read and how long they
//! take, and how writes are spaced. Fixed-size rings behind one lock, nothing
//! persisted, nothing parsed on the cache-hit path — the numbers are the
//! inputs to the go/no-go thresholds of
//! `docs/notes/analytical-mirror-design.md` §1.4 and are read through
//! `GET /api/admin/telemetry`.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Which exit of [`crate::store::TripleStore::query`] answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Served {
    /// The result cache.
    CacheHit,
    /// The columnar copy's own evaluator (opengraph::columnar).
    Columnar,
    /// The O(1) per-graph count index (`SELECT (COUNT(*) …) { ?s ?p ?o }`).
    FastCount,
    /// The subject-hash shards of the in-memory mirror.
    Shards,
    /// The unsharded in-memory copy.
    FullCopy,
    /// The store itself: RocksDB when persistent, the memory backend otherwise.
    Engine,
}

impl Served {
    pub const ALL: [Served; 6] = [
        Served::CacheHit,
        Served::Columnar,
        Served::FastCount,
        Served::Shards,
        Served::FullCopy,
        Served::Engine,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Served::CacheHit => "cache_hit",
            Served::Columnar => "columnar",
            Served::FastCount => "fast_count",
            Served::Shards => "shards",
            Served::FullCopy => "full_copy",
            Served::Engine => "engine",
        }
    }
}

/// The two shape bits of a query, computed once per uncached evaluation and
/// stamped on the cache entry, so a hit inherits them without a parse — the
/// repeated aggregates a dashboard fires are exactly the hits, and a bit
/// computed only on misses would under-count them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QueryShape {
    /// The parallel classifier calls it an `Aggregate`: a global or grouped
    /// aggregate, or an `ASK` — the "analytical" bit.
    pub analytical: bool,
    /// The text mentions `COUNT(` or `GROUP BY`, whitespace-insensitively —
    /// the cheaper, wider net, kept beside the classifier's verdict.
    pub aggregate_text: bool,
}

impl QueryShape {
    /// The text bit. The classifier bit is the caller's: it already parses.
    pub fn mentions_aggregate(sparql: &str) -> bool {
        let compact: String = sparql
            .chars()
            .filter(|c| !c.is_whitespace())
            .map(|c| c.to_ascii_lowercase())
            .collect();
        compact.contains("count(") || compact.contains("groupby")
    }
}

#[derive(Clone, Copy, Debug)]
pub struct QuerySample {
    pub served: Served,
    pub elapsed_us: u32,
    pub shape: QueryShape,
}

/// One SHACL validation run, as the engine saw it.
#[derive(Clone, Debug, serde::Serialize)]
pub struct ValidationSample {
    /// Who asked: `dataset` (the validate route), `gate` (a write gate),
    /// `pipeline` (a Studio run), `engine` (a direct call).
    pub path: &'static str,
    pub duration_ms: u32,
    /// Quads in the data graphs, from the count index.
    pub quads: u64,
    pub graphs: u32,
    /// The run's data source: the mirror copy, a RocksDB snapshot, or live.
    pub source: &'static str,
    pub run_index: bool,
    pub results: u32,
}

/// Upper bounds (exclusive, ms) of the inter-write gap histogram.
pub const GAP_BUCKETS: [(&str, u64); 8] = [
    ("lt_100ms", 100),
    ("lt_500ms", 500),
    ("lt_1s", 1_000),
    ("lt_5s", 5_000),
    ("lt_30s", 30_000),
    ("lt_5min", 300_000),
    ("lt_1h", 3_600_000),
    ("ge_1h", u64::MAX),
];

/// A fixed-capacity ring: the newest `capacity` samples, order irrelevant.
struct Ring<T> {
    buf: Vec<T>,
    next: usize,
    capacity: usize,
}

impl<T> Ring<T> {
    fn new(capacity: usize) -> Self {
        Self {
            buf: Vec::with_capacity(capacity.min(1024)),
            next: 0,
            capacity: capacity.max(1),
        }
    }

    fn push(&mut self, v: T) {
        if self.buf.len() < self.capacity {
            self.buf.push(v);
        } else {
            self.buf[self.next] = v;
        }
        self.next = (self.next + 1) % self.capacity;
    }

    fn iter(&self) -> impl Iterator<Item = &T> {
        self.buf.iter()
    }
}

/// Default ring sizes; `OTS_TELEMETRY_QUERY_RING` and
/// `OTS_TELEMETRY_VALIDATION_RING` override them.
pub const DEFAULT_QUERY_RING: usize = 8192;
pub const DEFAULT_VALIDATION_RING: usize = 1024;

pub struct Telemetry {
    started: Instant,
    queries: Mutex<Ring<QuerySample>>,
    validations: Mutex<Ring<ValidationSample>>,
    query_total: AtomicU64,
    validation_total: AtomicU64,
    writes: AtomicU64,
    /// Milliseconds since `started` of the last write, 0 before the first.
    last_write_ms: AtomicU64,
    gaps: [AtomicU64; GAP_BUCKETS.len()],
}

impl Default for Telemetry {
    fn default() -> Self {
        Self::new()
    }
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(default)
}

impl Telemetry {
    pub fn new() -> Self {
        Self::with_capacity(
            env_usize("OTS_TELEMETRY_QUERY_RING", DEFAULT_QUERY_RING),
            env_usize("OTS_TELEMETRY_VALIDATION_RING", DEFAULT_VALIDATION_RING),
        )
    }

    pub fn with_capacity(queries: usize, validations: usize) -> Self {
        Self {
            started: Instant::now(),
            queries: Mutex::new(Ring::new(queries)),
            validations: Mutex::new(Ring::new(validations)),
            query_total: AtomicU64::new(0),
            validation_total: AtomicU64::new(0),
            writes: AtomicU64::new(0),
            last_write_ms: AtomicU64::new(0),
            gaps: Default::default(),
        }
    }

    fn now_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// One query answered. The hot path (a cache hit) pays one lock and a
    /// 16-byte write.
    pub fn record_query(&self, served: Served, elapsed: Duration, shape: QueryShape) {
        self.query_total.fetch_add(1, Ordering::Relaxed);
        let sample = QuerySample {
            served,
            elapsed_us: elapsed.as_micros().min(u32::MAX as u128) as u32,
            shape,
        };
        if let Ok(mut ring) = self.queries.lock() {
            ring.push(sample);
        }
    }

    pub fn record_validation(&self, sample: ValidationSample) {
        self.validation_total.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut ring) = self.validations.lock() {
            ring.push(sample);
        }
    }

    /// One write started (the outermost guard only, so a nested primitive
    /// does not register a zero-length gap).
    pub fn record_write(&self) {
        let now = self.now_ms().max(1);
        let prev = self.last_write_ms.swap(now, Ordering::AcqRel);
        self.writes.fetch_add(1, Ordering::Relaxed);
        if prev > 0 {
            self.record_gap_ms(now.saturating_sub(prev));
        }
    }

    fn record_gap_ms(&self, gap_ms: u64) {
        let idx = GAP_BUCKETS
            .iter()
            .position(|(_, upper)| gap_ms < *upper)
            .unwrap_or(GAP_BUCKETS.len() - 1);
        self.gaps[idx].fetch_add(1, Ordering::Relaxed);
    }

    pub fn summary(&self) -> Summary {
        let queries: Vec<QuerySample> = self
            .queries
            .lock()
            .map(|r| r.iter().copied().collect())
            .unwrap_or_default();
        let validations: Vec<ValidationSample> = self
            .validations
            .lock()
            .map(|r| r.iter().cloned().collect())
            .unwrap_or_default();
        let window = queries.len();
        let (analytical, other): (Vec<&QuerySample>, Vec<&QuerySample>) =
            queries.iter().partition(|q| q.shape.analytical);
        let mut by_served: BTreeMap<String, u64> = BTreeMap::new();
        for s in Served::ALL {
            by_served.insert(s.label().to_string(), 0);
        }
        for q in &queries {
            *by_served.entry(q.served.label().to_string()).or_default() += 1;
        }
        let aggregate_text = queries.iter().filter(|q| q.shape.aggregate_text).count() as u64;
        Summary {
            uptime_secs: self.started.elapsed().as_secs(),
            queries: QuerySummary {
                total: self.query_total.load(Ordering::Relaxed),
                window,
                by_served,
                aggregate_text,
                analytical: LatencySummary::of(&analytical, window),
                other: LatencySummary::of(&other, window),
            },
            validations: ValidationSummary::of(
                &validations,
                self.validation_total.load(Ordering::Relaxed),
            ),
            writes: WriteSummary {
                total: self.writes.load(Ordering::Relaxed),
                gaps: GAP_BUCKETS
                    .iter()
                    .zip(self.gaps.iter())
                    .map(|((label, upper), n)| GapBucket {
                        label,
                        upper_ms: (*upper != u64::MAX).then_some(*upper),
                        count: n.load(Ordering::Relaxed),
                    })
                    .collect(),
            },
        }
    }
}

/// Nearest-rank percentile of a sorted slice; 0 for an empty one.
fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Summary {
    pub uptime_secs: u64,
    pub queries: QuerySummary,
    pub validations: ValidationSummary,
    pub writes: WriteSummary,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct QuerySummary {
    /// Every query since start.
    pub total: u64,
    /// Samples currently in the ring — the population the rest describes.
    pub window: usize,
    pub by_served: BTreeMap<String, u64>,
    /// Samples whose text mentions `COUNT(` or `GROUP BY`.
    pub aggregate_text: u64,
    /// The analytical subset (the classifier says aggregate or `ASK`).
    pub analytical: LatencySummary,
    /// Everything else.
    pub other: LatencySummary,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct LatencySummary {
    pub count: u64,
    /// `count / window`, 0 when the window is empty.
    pub share: f64,
    pub p50_us: u64,
    pub p95_us: u64,
    pub p99_us: u64,
    pub max_us: u64,
    pub by_served: BTreeMap<String, u64>,
}

impl LatencySummary {
    fn of(samples: &[&QuerySample], window: usize) -> Self {
        let mut sorted: Vec<u64> = samples.iter().map(|q| q.elapsed_us as u64).collect();
        sorted.sort_unstable();
        let mut by_served = BTreeMap::new();
        for q in samples {
            *by_served.entry(q.served.label().to_string()).or_default() += 1;
        }
        Self {
            count: samples.len() as u64,
            share: if window == 0 {
                0.0
            } else {
                samples.len() as f64 / window as f64
            },
            p50_us: percentile(&sorted, 50.0),
            p95_us: percentile(&sorted, 95.0),
            p99_us: percentile(&sorted, 99.0),
            max_us: sorted.last().copied().unwrap_or(0),
            by_served,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ValidationSummary {
    pub total: u64,
    pub window: usize,
    pub by_path: BTreeMap<String, u64>,
    pub by_source: BTreeMap<String, u64>,
    pub with_run_index: u64,
    pub p50_ms: u64,
    pub p95_ms: u64,
    pub max_ms: u64,
    pub max_quads: u64,
}

impl ValidationSummary {
    fn of(samples: &[ValidationSample], total: u64) -> Self {
        let mut sorted: Vec<u64> = samples.iter().map(|v| v.duration_ms as u64).collect();
        sorted.sort_unstable();
        let mut by_path = BTreeMap::new();
        let mut by_source = BTreeMap::new();
        for v in samples {
            *by_path.entry(v.path.to_string()).or_default() += 1;
            *by_source.entry(v.source.to_string()).or_default() += 1;
        }
        Self {
            total,
            window: samples.len(),
            by_path,
            by_source,
            with_run_index: samples.iter().filter(|v| v.run_index).count() as u64,
            p50_ms: percentile(&sorted, 50.0),
            p95_ms: percentile(&sorted, 95.0),
            max_ms: sorted.last().copied().unwrap_or(0),
            max_quads: samples.iter().map(|v| v.quads).max().unwrap_or(0),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WriteSummary {
    pub total: u64,
    pub gaps: Vec<GapBucket>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GapBucket {
    pub label: &'static str,
    /// Exclusive upper bound in ms; `None` for the open-ended last bucket.
    pub upper_ms: Option<u64>,
    pub count: u64,
}

// ─── Who is validating ──────────────────────────────────────────────────────

thread_local! {
    static VALIDATION_PATH: Cell<&'static str> = const { Cell::new("engine") };
}

/// Labels the validation runs made on this thread while it lives, for the
/// engine to stamp on its samples. Set it inside the code that calls the
/// engine and let it drop before any `.await`: a thread-local does not follow
/// a task across one.
pub struct ValidationPathGuard(&'static str);

impl ValidationPathGuard {
    pub fn set(path: &'static str) -> Self {
        Self(VALIDATION_PATH.with(|c| c.replace(path)))
    }
}

impl Drop for ValidationPathGuard {
    fn drop(&mut self) {
        VALIDATION_PATH.with(|c| c.set(self.0));
    }
}

/// The label in force on this thread (`engine` when nobody set one).
pub fn validation_path() -> &'static str {
    VALIDATION_PATH.with(|c| c.get())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(served: Served, us: u32, analytical: bool) -> (Served, Duration, QueryShape) {
        (
            served,
            Duration::from_micros(us as u64),
            QueryShape {
                analytical,
                aggregate_text: analytical,
            },
        )
    }

    #[test]
    fn the_ring_keeps_the_newest_capacity_samples() {
        let t = Telemetry::with_capacity(4, 2);
        for i in 0..10u32 {
            let (s, d, sh) = q(Served::Engine, i, false);
            t.record_query(s, d, sh);
        }
        let s = t.summary();
        assert_eq!(s.queries.total, 10);
        assert_eq!(s.queries.window, 4);
        // The survivors are the last four: 6..=9 µs.
        assert_eq!(s.queries.other.max_us, 9);
        assert_eq!(s.queries.other.p50_us, 7);
    }

    #[test]
    fn shape_bits_split_the_population_and_shares_add_up() {
        let t = Telemetry::with_capacity(16, 2);
        for (s, d, sh) in [
            q(Served::CacheHit, 5, true),
            q(Served::Shards, 900, true),
            q(Served::Engine, 40, false),
            q(Served::FastCount, 3, false),
        ] {
            t.record_query(s, d, sh);
        }
        let s = t.summary();
        assert_eq!(s.queries.analytical.count, 2);
        assert_eq!(s.queries.other.count, 2);
        assert!((s.queries.analytical.share - 0.5).abs() < 1e-9);
        assert_eq!(s.queries.analytical.p95_us, 900);
        assert_eq!(s.queries.by_served["cache_hit"], 1);
        assert_eq!(s.queries.by_served["full_copy"], 0, "every exit is listed");
        assert_eq!(s.queries.analytical.by_served["shards"], 1);
        assert_eq!(s.queries.aggregate_text, 2);
    }

    #[test]
    fn percentiles_are_nearest_rank() {
        let v: Vec<u64> = (1..=100).collect();
        assert_eq!(percentile(&v, 50.0), 50);
        assert_eq!(percentile(&v, 95.0), 95);
        assert_eq!(percentile(&v, 99.0), 99);
        assert_eq!(percentile(&[7], 99.0), 7);
        assert_eq!(percentile(&[], 50.0), 0);
    }

    #[test]
    fn write_gaps_land_in_their_buckets() {
        let t = Telemetry::with_capacity(2, 2);
        t.record_gap_ms(0);
        t.record_gap_ms(99);
        t.record_gap_ms(100);
        t.record_gap_ms(4_999);
        t.record_gap_ms(7_200_000);
        let s = t.summary();
        let count = |label: &str| {
            s.writes
                .gaps
                .iter()
                .find(|g| g.label == label)
                .unwrap()
                .count
        };
        assert_eq!(count("lt_100ms"), 2);
        assert_eq!(count("lt_500ms"), 1);
        assert_eq!(count("lt_5s"), 1);
        assert_eq!(count("ge_1h"), 1);
        // The first write has no predecessor and records no gap.
        t.record_write();
        assert_eq!(t.summary().writes.total, 1);
        assert_eq!(
            t.summary().writes.gaps.iter().map(|g| g.count).sum::<u64>(),
            5
        );
    }

    #[test]
    fn validation_samples_summarise_by_path_and_source() {
        let t = Telemetry::with_capacity(2, 8);
        for (path, ms, source, idx) in [
            ("dataset", 120, "snapshot", true),
            ("gate", 8, "live", false),
            ("gate", 12, "live", false),
        ] {
            t.record_validation(ValidationSample {
                path,
                duration_ms: ms,
                quads: 1_000 * ms as u64,
                graphs: 1,
                source,
                run_index: idx,
                results: 0,
            });
        }
        let s = t.summary().validations;
        assert_eq!(s.total, 3);
        assert_eq!(s.by_path["gate"], 2);
        assert_eq!(s.by_source["live"], 2);
        assert_eq!(s.with_run_index, 1);
        assert_eq!(s.max_ms, 120);
        assert_eq!(s.max_quads, 120_000);
        assert_eq!(s.p50_ms, 12);
    }

    #[test]
    fn the_path_guard_restores_the_previous_label() {
        assert_eq!(validation_path(), "engine");
        {
            let _outer = ValidationPathGuard::set("gate");
            assert_eq!(validation_path(), "gate");
            {
                let _inner = ValidationPathGuard::set("pipeline");
                assert_eq!(validation_path(), "pipeline");
            }
            assert_eq!(validation_path(), "gate");
        }
        assert_eq!(validation_path(), "engine");
    }

    #[test]
    fn the_text_bit_ignores_whitespace_and_case() {
        assert!(QueryShape::mentions_aggregate(
            "SELECT (COUNT (*) AS ?n) WHERE { ?s ?p ?o }"
        ));
        assert!(QueryShape::mentions_aggregate(
            "select ?t where { ?s a ?t } group\n by ?t"
        ));
        assert!(!QueryShape::mentions_aggregate(
            "SELECT ?s WHERE { ?s ?p ?o } LIMIT 10"
        ));
    }
}
