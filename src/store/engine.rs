use dashmap::DashMap;
use opengraph::spargebra::algebra::{AggregateExpression, Expression, GraphPattern};
use opengraph::spargebra::term::{NamedNodePattern, TermPattern, TriplePattern};
use opengraph::spargebra::{Query as SpargebraQuery, SparqlParser, Update as SpargebraUpdate};
use oxigraph::io::{RdfFormat, RdfParser, RdfSerializer};
use oxigraph::model::*;
use oxigraph::sparql::{QueryResults, QuerySolution, QuerySolutionIter, SparqlEvaluator};
use oxigraph::store::Store;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::sync::Arc;
use thiserror::Error;
use tracing::{debug, info};

use crate::geo::functions as geo_fns;
#[cfg(feature = "geometry3d")]
use crate::geo::index3d::SpatialIndex3D;
use crate::geo::spatial_index::SpatialIndex;
use crate::store::parallel_mirror::ParallelMirror;
use crate::store::query_cache::QueryCache;

#[derive(Error, Debug)]
pub enum StoreError {
    #[error("Storage error: {0}")]
    Storage(#[from] oxigraph::store::StorageError),
    #[error("Serializer error: {0}")]
    Serializer(#[from] oxigraph::store::SerializerError),
    #[error("Loader error: {0}")]
    Loader(#[from] oxigraph::store::LoaderError),
    #[error("SPARQL evaluation error: {0}")]
    Evaluation(#[from] oxigraph::sparql::QueryEvaluationError),
    #[error("SPARQL update error: {0}")]
    Update(#[from] oxigraph::sparql::UpdateEvaluationError),
    #[error("SPARQL syntax error: {0}")]
    SparqlSyntax(#[from] oxigraph::sparql::SparqlSyntaxError),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Unsupported format: {0}")]
    UnsupportedFormat(String),
    #[error("Graph not found: {0}")]
    GraphNotFound(String),
}

/// How blank nodes are treated when data is imported.
///
/// Plain RDF blank nodes are not durable: each parse mints fresh labels, so
/// re-importing or reloading the same data renames every anonymous node (and
/// re-importing *duplicates* it). OpenGraph fixes this on the import path.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BlankNodeMode {
    /// Keep the parser-assigned labels. Streams directly into the store (fastest,
    /// lowest memory) but blank nodes are **not** durable across re-imports.
    ///
    /// Current default: import behaviour is unchanged unless durability is
    /// explicitly enabled via [`TripleStore::with_blank_node_mode`]. Flipping the
    /// default to `Canonical` is the intended end-state once exercised against the
    /// full backend test suite.
    #[default]
    Preserve,
    /// Relabel blank nodes to deterministic, content-derived labels
    /// (`opengraph::canonical::stable_relabel`). The same structure always gets
    /// the same label, so re-importing is idempotent and reloads are stable.
    /// **Recommended** for durable blank-node identity.
    Canonical,
    /// Replace blank nodes with durable Skolem IRIs in the `/.well-known/genid/`
    /// space. Maximally durable and directly query-able, at the cost of turning
    /// anonymous nodes into IRIs.
    Skolem,
}

/// In-memory index of named-graph IRIs and their triple counts.
/// Provides O(1) graph enumeration instead of full index scans.
///
/// Uses `DashMap` for lock-free concurrent reads. Previously this was an
/// `Arc<RwLock<HashMap>>`, which serialized every browse_graphs request even
/// for read-only access — a major source of triple-browser slowness under
/// concurrent panel loads.
#[derive(Clone)]
struct GraphIndex {
    /// `None` key = default graph, `Some(iri)` = named graph.
    counts: Arc<DashMap<Option<String>, usize>>,
    /// Observability/test hook: total number of per-graph triple-count scans
    /// performed (one `quads_for_pattern(..).count()` per graph counted, in either
    /// a full [`Self::rebuild`] or a targeted [`Self::recount_specific_graphs`]).
    /// A targeted write must touch only its own graph; this counter lets a
    /// regression test prove an unrelated multi-million-triple graph (e.g. a
    /// dataset's `…/ifcowl` lift) is *not* rescanned on every `store.update()`.
    scans: Arc<std::sync::atomic::AtomicU64>,
}

impl GraphIndex {
    fn new() -> Self {
        Self {
            counts: Arc::new(DashMap::new()),
            scans: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    /// Count the quads in one graph, recording the scan for observability.
    fn scan_graph(&self, store: &Store, graph: GraphNameRef<'_>) -> usize {
        self.scans
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        store
            .quads_for_pattern(None, None, None, Some(graph))
            .count()
    }

    /// Total per-graph count scans performed since construction (shared across
    /// store clones). Test-only: lets a regression test assert that a targeted
    /// write recounts just its own graph instead of rescanning every graph.
    #[cfg(test)]
    fn scan_count(&self) -> u64 {
        self.scans.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Rebuild the entire index from the store.
    fn rebuild(&self, store: &Store) {
        // Compute new values first, then swap atomically per-key.
        // Default graph count
        let default_count = self.scan_graph(store, GraphNameRef::DefaultGraph);

        let mut fresh: Vec<(Option<String>, usize)> = Vec::new();
        fresh.push((None, default_count));

        for g in store.named_graphs() {
            if let Ok(NamedOrBlankNode::NamedNode(nn)) = g {
                let count = self.scan_graph(store, GraphNameRef::NamedNode(nn.as_ref()));
                fresh.push((Some(nn.as_str().to_string()), count));
            }
        }

        self.counts.clear();
        for (k, v) in fresh {
            self.counts.insert(k, v);
        }
    }

    /// Get count for a specific graph.
    fn get_count(&self, graph_iri: Option<&str>) -> Option<usize> {
        match graph_iri {
            Some(iri) => self.counts.get(&Some(iri.to_string())).map(|v| *v),
            None => self.counts.get(&None).map(|v| *v),
        }
    }

    /// Get all entries: (Option<iri>, count).
    fn all_entries(&self) -> Vec<(Option<String>, usize)> {
        self.counts
            .iter()
            .map(|kv| (kv.key().clone(), *kv.value()))
            .collect()
    }

    /// Remove a specific graph entry.
    fn remove(&self, graph_iri: Option<&str>) {
        match graph_iri {
            Some(iri) => {
                self.counts.remove(&Some(iri.to_string()));
            }
            None => {
                self.counts.remove(&None);
            }
        }
    }

    /// M-5: Re-count only the specified graphs instead of doing a full rebuild.
    /// Pass `None` in the slice to re-count the default graph.
    ///
    /// Stays faithful to [`Self::rebuild`]: a named graph that recounts to 0 and
    /// is no longer present in the store's named-graph set is *removed* from the
    /// index (a full rebuild only enumerates graphs in `named_graphs()`, so an
    /// emptied implicit graph drops out). Otherwise the per-graph counts and
    /// `cached_named_graph_count` would drift after a `DELETE` empties a graph.
    /// Add `delta` new quads to `graph`'s count (a graph-targeted load whose
    /// new-quad count is known exactly), instead of rescanning the graph.
    fn add(&self, graph_iri: Option<&str>, delta: usize) {
        let key = graph_iri.map(str::to_string);
        *self.counts.entry(key).or_insert(0) += delta;
    }

    /// Adjust `graph`'s count by a signed delta (a ground update whose effect
    /// on the graph is known exactly). Never goes below zero.
    fn adjust(&self, graph_iri: Option<&str>, delta: i64) {
        let key = graph_iri.map(str::to_string);
        let mut e = self.counts.entry(key).or_insert(0);
        *e = (*e as i64 + delta).max(0) as usize;
    }

    fn recount_specific_graphs(&self, store: &Store, graph_iris: &[Option<String>]) {
        for graph_iri in graph_iris {
            match graph_iri {
                None => {
                    // The default graph always conceptually exists (rebuild always
                    // records it), so keep its entry even at 0.
                    let count = self.scan_graph(store, GraphNameRef::DefaultGraph);
                    self.counts.insert(None, count);
                }
                Some(iri) => {
                    if let Ok(nn) = NamedNode::new(iri.as_str()) {
                        let count = self.scan_graph(store, GraphNameRef::NamedNode(nn.as_ref()));
                        // Mirror rebuild: a named graph appears in the index iff it
                        // is in `named_graphs()`. An emptied implicit graph is gone
                        // from that set, so drop its entry rather than leaving a
                        // ghost 0-count. An explicitly-created empty graph stays
                        // (`contains_named_graph` is true), matching rebuild.
                        let keep =
                            count > 0 || store.contains_named_graph(nn.as_ref()).unwrap_or(true);
                        if keep {
                            self.counts.insert(Some(iri.clone()), count);
                        } else {
                            self.counts.remove(&Some(iri.clone()));
                        }
                    }
                }
            }
        }
    }
}

/// The core triple store engine wrapping Oxigraph with GeoSPARQL extensions.
/// Does the query text mention `SERVICE` (case-insensitively)? A false positive
/// only costs a cache miss.
fn sparql_uses_service(sparql: &str) -> bool {
    sparql
        .as_bytes()
        .windows(7)
        .any(|w| w.eq_ignore_ascii_case(b"SERVICE"))
}

/// The exact effect of a ground write: `(inserted, deleted)` quads.
pub type QuadDelta = (Vec<Quad>, Vec<Quad>);

/// Whole-store VoID statistics (see [`TripleStore::void_stats`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VoidStats {
    pub triples: usize,
    pub distinct_subjects: usize,
    pub distinct_predicates: usize,
    pub distinct_objects: usize,
    pub named_graphs: usize,
}

#[derive(Clone)]
pub struct TripleStore {
    store: Arc<Store>,
    graph_index: GraphIndex,
    spatial_index: SpatialIndex,
    /// 3D R*-tree over volumetric-geometry AABBs — the broad phase for `ots-geof:`
    /// 3D relations and the 3D-Tiles/OGC bbox pre-filter. Lazily rebuilt on dirty,
    /// mirroring [`spatial_index`](Self::spatial_index).
    #[cfg(feature = "geometry3d")]
    spatial_index_3d: SpatialIndex3D,
    /// In-memory subject-sharded read accelerator: a decomposable aggregate/ASK is
    /// answered across cores instead of on one. Derived from `store`, rebuilt
    /// lazily after writes, and bounded by a triple-count cap (see
    /// [`ParallelMirror`]).
    parallel_mirror: ParallelMirror,
    /// Memoises small query results (invalidated on every write); a repeated query
    /// is answered without re-evaluation. See [`QueryCache`].
    query_cache: QueryCache,
    /// VoID statistics for the whole store, keyed by the write generation
    /// they were computed at (see [`TripleStore::void_stats`]).
    void_stats_cache: std::sync::Arc<std::sync::Mutex<Option<(u64, VoidStats)>>>,
    /// Blank-node durability policy applied on import. Defaults to
    /// [`BlankNodeMode::Preserve`] (opt into durability via
    /// [`TripleStore::with_blank_node_mode`]).
    blank_node_mode: BlankNodeMode,
    /// Process-unique identity for cache keying (shared by clones, distinct per
    /// underlying store). Prevents the per-thread SHACL path cache from serving
    /// one store's results to another (same focus IRI + path, different data).
    cache_id: u64,
}

/// Monotonic source for [`TripleStore::cache_id`].
static NEXT_CACHE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl TripleStore {
    /// Open or create a persistent store at the given path.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let store = Store::open(path)?;
        info!("Opened store at {}", path.display());
        let graph_index = GraphIndex::new();
        graph_index.rebuild(&store);
        let spatial_index = SpatialIndex::new();
        spatial_index.rebuild(&store);
        #[cfg(feature = "geometry3d")]
        let spatial_index_3d = {
            // Defer the (potentially large) WKT-Z scan: a persistent store may
            // already hold geometry, but the 3D R*-tree is consumed only by the
            // opt-in 3D-Tiles broad phase. Mark it dirty so the `spatial_index_3d()`
            // accessor builds it lazily on the first 3D request — matching the 2D
            // index's lazy `mark_dirty` write path — instead of paying a full scan
            // + parse on every boot for deployments that never serve a 3D tile.
            let idx = SpatialIndex3D::new();
            idx.mark_dirty();
            idx
        };
        Ok(Self {
            store: Arc::new(store),
            graph_index,
            spatial_index,
            #[cfg(feature = "geometry3d")]
            spatial_index_3d,
            parallel_mirror: ParallelMirror::from_env(),
            query_cache: QueryCache::from_env(),
            void_stats_cache: std::sync::Arc::new(std::sync::Mutex::new(None)),
            blank_node_mode: BlankNodeMode::default(),
            cache_id: NEXT_CACHE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        })
    }

    /// Create an in-memory store (useful for testing).
    pub fn in_memory() -> Result<Self, StoreError> {
        let store = Store::new()?;
        let graph_index = GraphIndex::new();
        let spatial_index = SpatialIndex::new();
        #[cfg(feature = "geometry3d")]
        let spatial_index_3d = SpatialIndex3D::new();
        Ok(Self {
            store: Arc::new(store),
            graph_index,
            spatial_index,
            #[cfg(feature = "geometry3d")]
            spatial_index_3d,
            parallel_mirror: ParallelMirror::from_env(),
            query_cache: QueryCache::from_env(),
            void_stats_cache: std::sync::Arc::new(std::sync::Mutex::new(None)),
            blank_node_mode: BlankNodeMode::default(),
            cache_id: NEXT_CACHE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        })
    }

    /// Set the blank-node durability policy applied on import (builder style).
    pub fn with_blank_node_mode(mut self, mode: BlankNodeMode) -> Self {
        self.blank_node_mode = mode;
        self
    }

    /// Override the parallel-query mirror configuration (builder style) instead of
    /// reading it from the environment. `enabled` toggles the accelerator,
    /// `shards` is the subject-hash shard count (clamped to 1..=16), and
    /// `max_triples` is the memory cap above which it stays disabled.
    pub fn with_parallel_query(mut self, enabled: bool, shards: usize, max_triples: usize) -> Self {
        self.parallel_mirror = ParallelMirror::new(enabled, shards, max_triples);
        self
    }

    /// Override the query-result cache configuration (builder style; tests).
    pub fn with_query_cache(mut self, enabled: bool, max_entries: usize, max_rows: usize) -> Self {
        self.query_cache = QueryCache::new(enabled, max_entries, max_rows);
        self
    }

    /// Override the parallel mirror's rebuild quiet period (builder style;
    /// tests). `0` rebuilds eagerly on the first query after a write.
    ///
    /// The parity suite needs this: every test loads data and queries within
    /// microseconds, well inside the default 500 ms quiet window, so the mirror
    /// declined to build and BOTH sides of every "parity" assertion were the
    /// persistent store — the suite compared the engine against itself.
    pub fn with_parallel_rebuild_quiet_ms(self, ms: u64) -> Self {
        self.parallel_mirror.set_rebuild_quiet_ms(ms);
        self
    }

    /// How many times the parallel mirror has been (re)built. Tests use it to
    /// prove the mirror was consulted rather than silently bypassed.
    pub fn parallel_build_count(&self) -> usize {
        self.parallel_mirror.build_count()
    }

    /// Record a write: invalidate the in-memory mirror (mark for rebuild) and the
    /// result cache (bump its generation). Called by every mutating path so reads
    /// never see stale derived state.
    fn note_write(&self) {
        self.parallel_mirror.mark_dirty();
        self.query_cache.invalidate();
    }

    /// The blank-node durability policy currently in effect.
    pub fn blank_node_mode(&self) -> BlankNodeMode {
        self.blank_node_mode
    }

    /// Apply the configured blank-node policy to a freshly-parsed quad batch.
    ///
    /// Canonicalization/Skolemization are whole-batch operations, so callers
    /// must materialise the quads first (see [`Self::load_reader`]).
    fn apply_blank_node_mode(&self, quads: Vec<Quad>) -> Vec<Quad> {
        match self.blank_node_mode {
            BlankNodeMode::Preserve => quads,
            BlankNodeMode::Canonical => opengraph::canonical::stable_relabel(&quads),
            BlankNodeMode::Skolem => {
                opengraph::skolem::skolemize(&quads, opengraph::DEFAULT_SKOLEM_BASE).0
            }
        }
    }

    /// Access the underlying Oxigraph store for direct index queries.
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Process-unique store identity for cache keying (stable across clones).
    pub fn cache_id(&self) -> u64 {
        self.cache_id
    }

    /// Build query options with all registered custom functions (GeoSPARQL, RDF 1.2, etc.).
    /// The store's write generation: bumped on every mutation. Lets read-side
    /// caches key on "has anything changed" instead of rescanning per request.
    pub fn write_generation(&self) -> u64 {
        self.query_cache.generation()
    }

    /// Whole-store VoID statistics over default *and* named graphs, cached
    /// until the next write. The DCAT catalogue used to run three DISTINCT
    /// scans over the default graph only on every anonymous request — and
    /// report 0 distinct subjects for a store whose data sits in named graphs.
    pub fn void_stats(&self) -> VoidStats {
        let generation = self.write_generation();
        if let Ok(guard) = self.void_stats_cache.lock() {
            if let Some((g, stats)) = guard.as_ref() {
                if *g == generation {
                    return *stats;
                }
            }
        }
        let count = |q: &str| -> usize {
            match self.query(q) {
                Ok(oxigraph::sparql::QueryResults::Solutions(mut sols)) => sols
                    .next()
                    .and_then(|r| r.ok())
                    .and_then(|r| match r.get(0) {
                        Some(oxigraph::model::Term::Literal(l)) => l.value().parse().ok(),
                        _ => None,
                    })
                    .unwrap_or(0),
                _ => 0,
            }
        };
        let stats = VoidStats {
            triples: self.len().unwrap_or(0),
            distinct_subjects: count(
                "SELECT (COUNT(DISTINCT ?s) AS ?c) WHERE { { ?s ?p ?o } UNION { GRAPH ?g { ?s ?p ?o } } }",
            ),
            distinct_predicates: count(
                "SELECT (COUNT(DISTINCT ?p) AS ?c) WHERE { { ?s ?p ?o } UNION { GRAPH ?g { ?s ?p ?o } } }",
            ),
            distinct_objects: count(
                "SELECT (COUNT(DISTINCT ?o) AS ?c) WHERE { { ?s ?p ?o } UNION { GRAPH ?g { ?s ?p ?o } } }",
            ),
            named_graphs: self.named_graphs().map(|g| g.len()).unwrap_or(0),
        };
        if let Ok(mut guard) = self.void_stats_cache.lock() {
            *guard = Some((generation, stats));
        }
        stats
    }

    pub(crate) fn query_options(&self) -> SparqlEvaluator {
        // SPARQL federation (`SERVICE`) stays disabled: oxigraph is built without the
        // `http-client` feature, so there is no HTTP service handler and `SERVICE`/`LOAD`
        // error rather than fetch — SERVICE-based SSRF/exfiltration stays off. (SSRF-1)
        // (oxigraph 0.5 moved the explicit `without_service_handler` toggle behind the
        // `http-client` feature, so there is nothing to call when it is disabled.)
        let mut opts = SparqlEvaluator::new();
        // SPARQL federation: every `SERVICE` goes through the allowlisted
        // handler (crate::sparql::federation) — no allowlist, no network.
        opts = opts.with_default_service_handler(
            crate::sparql::federation::AllowlistedServiceHandler {
                identity: crate::federation::current_identity(),
            },
        );

        // Register all GeoSPARQL functions
        for (iri, handler) in geo_fns::all_functions() {
            opts = opts.with_custom_function(iri, move |args| handler(args));
        }

        // Register the additive ots-geof: 3D functions (spec §3.4). Separate
        // namespace, so GeoSPARQL 1.1 results are unchanged.
        #[cfg(feature = "geometry3d")]
        for (iri, handler) in crate::geo::functions3d::all_functions_3d() {
            opts = opts.with_custom_function(iri, move |args| handler(args));
        }

        // Register RDF 1.2 SPARQL built-in functions (rdf-12 feature)
        #[cfg(feature = "rdf-12")]
        for (iri, handler) in crate::sparql::rdf12_functions::all_functions() {
            opts = opts.with_custom_function(iri, move |args| handler(args));
        }

        // Register SPARQL 1.2 ADJUST function (always available)
        {
            let (iri, handler) = crate::sparql::rdf12_functions::adjust_function();
            opts = opts.with_custom_function(iri, move |args| handler(args));
        }

        // Register SHACL-AF user-defined functions (sh:SPARQLFunction) discovered in the
        // store. Discovery uses the raw quad index (never store.query), so this does not
        // re-enter query_options; each function evaluates against a fresh in-memory store.
        for (iri, handler) in crate::shacl::sparql_functions::all_functions(self) {
            opts = opts.with_custom_function(iri, move |args| handler(args));
        }

        opts
    }

    /// Execute a SPARQL query (SELECT, CONSTRUCT, ASK, DESCRIBE).
    pub fn query(&self, sparql: &str) -> Result<QueryResults<'static>, StoreError> {
        // A federated query is never cached: its SERVICE part reads a remote
        // whose data and whose view of *this caller's identity* the local write
        // generation knows nothing about.
        if sparql_uses_service(sparql) {
            return self.query_uncached(sparql);
        }
        // Result cache: a repeated, *deterministic* query is answered from a small
        // LRU keyed by the (already ACL-scoped) query string and invalidated on
        // every write — so a hit is the exact result the engine would compute.
        if let Some(cached) = self.query_cache.get(sparql) {
            return Ok(cached);
        }
        // Snapshot the generation BEFORE evaluating: a write that commits while
        // this query runs must invalidate the result, not be stamped onto it.
        let gen = self.query_cache.generation();
        let results = self.query_uncached(sparql)?;
        Ok(self.query_cache.put(sparql, gen, results))
    }

    /// The evaluation pipeline behind [`Self::query`], without the result cache.
    fn query_uncached(&self, sparql: &str) -> Result<QueryResults<'static>, StoreError> {
        // Use char-boundary-safe slicing to avoid panics on multi-byte UTF-8 input.
        let prefix_end = (0..=sparql.len().min(200))
            .rfind(|&i| sparql.is_char_boundary(i))
            .unwrap_or(0);
        debug!("Executing query: {}", &sparql[..prefix_end]);
        // Fast path: a global `COUNT(*)` over a full triple scan is answered from
        // the maintained per-graph count index instead of materialising and then
        // discarding every solution tuple. Callgrind shows ~30% of COUNT(*) cost is
        // tuple build/copy (`InternalTuple::set`, `EncodedTerm::clone`, memcpy) —
        // pure waste when the projection is only a count.
        if let Some(fast) = self.try_fast_count(sparql) {
            return Ok(fast);
        }
        // Multi-core path: a decomposable aggregate / `ASK` is evaluated across
        // subject-hash shards (the in-memory mirror) and merged, using every core
        // instead of one. Returns `None` — falling through to the single-store
        // evaluator below — for anything not provably safe, an over-cap store, or a
        // shard error, so results are identical to single-store evaluation.
        if let Some(parallel) = self
            .parallel_mirror
            .try_query(&self.store, sparql, || self.query_options())
        {
            return Ok(parallel);
        }
        // In-memory full mirror: everything the shards can't decompose (joins,
        // grouped non-COUNT aggregates, large SELECTs) is served from an unsharded
        // RAM copy within the cap. RocksDB answers a multi-pattern join with one
        // point lookup per row — ~40x slower than the same join in memory — so this
        // is the biggest win for non-aggregate reads. Identical engine + data, so
        // results match; `None` (over cap / error) falls through to RocksDB.
        if let Some(full) = self
            .parallel_mirror
            .try_full_query(&self.store, sparql, || self.query_options())
        {
            return Ok(full);
        }
        let results = self
            .query_options()
            .parse_query(sparql)?
            .on_store(&self.store)
            .execute()?;
        Ok(results)
    }

    /// Recognise `SELECT (COUNT(*) AS ?v) WHERE { ?s ?p ?o }` (optionally with a
    /// single default-graph `FROM <g>`) and answer it from the O(1) per-graph count
    /// index. Returns `None` for anything else, so the normal evaluator runs and
    /// results never change — this only short-circuits one exact, provably-safe
    /// shape (a single full-scan triple pattern over one graph; the count of a
    /// graph equals its triple count, no RDF-merge dedup involved).
    fn try_fast_count(&self, sparql: &str) -> Option<QueryResults<'static>> {
        // Cheap reject: must mention COUNT(*) (whitespace-insensitive) before parsing.
        if !sparql
            .chars()
            .filter(|c| !c.is_whitespace())
            .map(|c| c.to_ascii_lowercase())
            .collect::<String>()
            .contains("count(*)")
        {
            return None;
        }
        let parsed = SparqlParser::new().parse_query(sparql).ok()?;
        let (pattern, dataset) = match &parsed {
            SpargebraQuery::Select {
                pattern, dataset, ..
            } => (pattern, dataset),
            _ => return None,
        };
        let (var_name, scope) = count_star_var(pattern)?;
        let named_in_dataset = |g: &str| -> bool {
            match dataset.as_ref().and_then(|ds| ds.named.as_ref()) {
                Some(named) => named.iter().any(|n| n.as_str() == g),
                None => true,
            }
        };
        let count = match scope {
            // `{ ?s ?p ?o }` — the query's default graph.
            CountScope::Default => {
                let graph: Option<String> = match dataset {
                    None => None,
                    Some(ds) => match ds.default.as_slice() {
                        [] if ds.named.is_some() => return None,
                        [] => None,
                        [g] => Some(g.as_str().to_string()),
                        _ => return None, // multiple FROM → RDF-merge dedup, can't sum counts
                    },
                };
                self.graph_count_cached(graph.as_deref())
                    .or_else(|| self.count_graph(graph.as_deref()).ok())?
            }
            // `GRAPH <g> { ?s ?p ?o }` — one named graph, if it is in the dataset.
            CountScope::Named(g) => {
                if named_in_dataset(&g) {
                    self.graph_count_cached(Some(&g))
                        .or_else(|| self.count_graph(Some(&g)).ok())?
                } else {
                    0
                }
            }
            // `GRAPH ?g { ?s ?p ?o }` — every named graph in the dataset.
            CountScope::AnyNamed => self
                .graph_index
                .all_entries()
                .into_iter()
                .filter(|(k, _)| k.as_deref().is_some_and(&named_in_dataset))
                .map(|(_, c)| c)
                .sum(),
        };
        let var = oxigraph::sparql::Variable::new(var_name).ok()?;
        let lit = Literal::new_typed_literal(
            count.to_string(),
            NamedNode::new_unchecked("http://www.w3.org/2001/XMLSchema#integer"),
        );
        let vars: Arc<[oxigraph::sparql::Variable]> = Arc::from(vec![var]);
        let sol = QuerySolution::from((vars.clone(), vec![Some(Term::Literal(lit))]));
        let iter = QuerySolutionIter::new(vars, std::iter::once(Ok(sol)));
        Some(QueryResults::Solutions(iter))
    }

    /// Execute a SPARQL UPDATE operation.
    pub fn update(&self, sparql: &str) -> Result<(), StoreError> {
        // Use char-boundary-safe slicing to avoid panics on multi-byte UTF-8 input.
        let prefix_end = (0..=sparql.len().min(200))
            .rfind(|&i| sparql.is_char_boundary(i))
            .unwrap_or(0);
        debug!("Executing update: {}", &sparql[..prefix_end]);
        // Statically determine which graphs this UPDATE writes to *before*
        // mutating the store, so the per-graph count index can be surgically
        // recounted instead of fully rebuilt. A full rebuild rescans every named
        // graph — including a dataset's multi-million-triple `…/ifcowl` lift — so
        // doing it after each write turned the ~100 single-graph registry/audit/
        // migration `update()`s on the post-seed boot path into ~100 full-store
        // scans. `None` ⇒ targets can't be bounded (variable graph, CLEAR/DROP/
        // CREATE/LOAD, or a parse miss) ⇒ conservative full rebuild.
        let targets = Self::static_update_targets(sparql);
        self.query_options()
            .parse_update(sparql)?
            .on_store(&self.store)
            .execute()?;
        match targets {
            Some(targets) => self
                .graph_index
                .recount_specific_graphs(&self.store, &targets),
            None => self.graph_index.rebuild(&self.store),
        }
        self.note_write();
        Ok(())
    }

    /// Statically determine the set of graphs a SPARQL UPDATE writes to, so the
    /// per-graph count index can be surgically recounted instead of fully rebuilt.
    ///
    /// Returns `Some(targets)` only when *every* operation writes to graphs whose
    /// IRIs are known at parse time: the ground `GRAPH <iri>` blocks of
    /// `INSERT DATA` / `DELETE DATA`, and the literal `GRAPH <iri>` templates of
    /// `DELETE/INSERT … WHERE`. Each target is `Option<String>` (`None` = default
    /// graph); a write's count delta is confined to the graphs named in its
    /// delete/insert *templates* — the `WHERE`/`USING` clauses only match, they
    /// never write — so recounting exactly those keeps every count correct.
    ///
    /// Returns `None` — "do a full rebuild" — whenever the effect can't be bounded
    /// to statically-known graphs: a variable graph target (`GRAPH ?g`), any
    /// `LOAD`/`CLEAR`/`CREATE`/`DROP` (which add or remove whole graph entries),
    /// or a parse failure. Conservative by construction: when in doubt, rebuild.
    fn static_update_targets(sparql: &str) -> Option<Vec<Option<String>>> {
        use opengraph::spargebra::term::{GraphName, GraphNamePattern};
        use opengraph::spargebra::GraphUpdateOperation;

        let parsed = SparqlParser::new().parse_update(sparql).ok()?;
        // Ground `GraphName` (DATA blocks): NamedNode or DefaultGraph, never a var.
        let ground = |g: &GraphName| match g {
            GraphName::NamedNode(nn) => Some(nn.as_str().to_string()),
            GraphName::DefaultGraph => None,
        };
        let mut targets: Vec<Option<String>> = Vec::new();
        for op in &parsed.operations {
            match op {
                GraphUpdateOperation::InsertData { data } => {
                    targets.extend(data.iter().map(|q| ground(&q.graph_name)));
                }
                GraphUpdateOperation::DeleteData { data } => {
                    targets.extend(data.iter().map(|q| ground(&q.graph_name)));
                }
                GraphUpdateOperation::DeleteInsert { delete, insert, .. } => {
                    // Both delete and insert templates can write; a variable graph
                    // target can resolve to graphs we can't enumerate → full rebuild.
                    for g in delete
                        .iter()
                        .map(|q| &q.graph_name)
                        .chain(insert.iter().map(|q| &q.graph_name))
                    {
                        match g {
                            GraphNamePattern::NamedNode(nn) => {
                                targets.push(Some(nn.as_str().to_string()))
                            }
                            GraphNamePattern::DefaultGraph => targets.push(None),
                            GraphNamePattern::Variable(_) => return None,
                        }
                    }
                }
                // LOAD / CLEAR / CREATE / DROP add or remove whole graph entries
                // (and LOAD's source is opaque); their effect isn't bounded to a
                // known count delta, so fall back to a full rebuild.
                _ => return None,
            }
        }
        // Many quads usually share one target graph (e.g. a registry INSERT DATA of
        // a dozen triples all in REGISTRY_GRAPH); dedup so that graph is recounted
        // once, not once per quad.
        targets.sort();
        targets.dedup();
        Some(targets)
    }

    /// M-5: Execute a SPARQL UPDATE using a surgical graph index update.
    ///
    /// Instead of rebuilding the entire graph count index after every write,
    /// only the graphs listed in `affected_iris` are re-counted.  When
    /// `full_rebuild` is true (e.g. CLEAR ALL / DROP ALL) a full rebuild is
    /// still performed.
    /// Run an update whose `WHERE` clauses read ONLY the union of `scope`: a
    /// SPARQL `USING` dataset is set on every DELETE/INSERT operation that does
    /// not declare one (`GRAPH <g>` patterns are unaffected). Without a scope
    /// the rules of the reasoners read the *unnamed default graph* only, which
    /// made every named graph — every dataset — invisible to
    /// `POST /api/reasoning/materialize`; this is how a reasoner materialises
    /// over a selected layer (a dataset's own graphs plus the model version it
    /// conforms to) instead of the whole store.
    pub fn update_scoped(&self, sparql: &str, scope: &[String]) -> Result<(), StoreError> {
        let mut parsed = spargebra::SparqlParser::new()
            .parse_update(sparql)
            .map_err(|e| StoreError::Parse(format!("scoped update: {e}")))?;
        let default = Self::scope_graphs(scope)?;
        for op in &mut parsed.operations {
            if let spargebra::GraphUpdateOperation::DeleteInsert { using, .. } = op {
                if using.is_none() {
                    *using = Some(spargebra::algebra::QueryDataset {
                        default: default.clone(),
                        named: None,
                    });
                }
            }
        }
        let targets = Self::static_update_targets(sparql);
        self.query_options()
            .for_update(parsed)
            .on_store(&self.store)
            .execute()?;
        match targets {
            Some(targets) => self
                .graph_index
                .recount_specific_graphs(&self.store, &targets),
            None => self.graph_index.rebuild(&self.store),
        }
        self.note_write();
        Ok(())
    }

    /// The read-only counterpart of [`update_scoped`](Self::update_scoped): a
    /// query without its own `FROM` clauses evaluated against the union of
    /// `scope` as its default graph. Uncached.
    pub fn query_scoped(
        &self,
        sparql: &str,
        scope: &[String],
    ) -> Result<QueryResults<'static>, StoreError> {
        let mut parsed = spargebra::SparqlParser::new()
            .parse_query(sparql)
            .map_err(|e| StoreError::Parse(format!("scoped query: {e}")))?;
        let ds = spargebra::algebra::QueryDataset {
            default: Self::scope_graphs(scope)?,
            named: None,
        };
        match &mut parsed {
            spargebra::Query::Select { dataset, .. }
            | spargebra::Query::Construct { dataset, .. }
            | spargebra::Query::Describe { dataset, .. }
            | spargebra::Query::Ask { dataset, .. } => {
                if dataset.is_none() {
                    *dataset = Some(ds);
                }
            }
        }
        Ok(self
            .query_options()
            .for_query(parsed)
            .on_store(&self.store)
            .execute()?)
    }

    fn scope_graphs(scope: &[String]) -> Result<Vec<oxigraph::model::NamedNode>, StoreError> {
        scope
            .iter()
            .map(|g| {
                oxigraph::model::NamedNode::new(g.as_str())
                    .map_err(|e| StoreError::Parse(format!("scope graph <{g}>: {e}")))
            })
            .collect()
    }

    pub fn update_targeted(
        &self,
        sparql: &str,
        affected_iris: &[String],
        full_rebuild: bool,
    ) -> Result<(), StoreError> {
        self.update_targeted_delta(sparql, affected_iris, full_rebuild)
            .map(|_| ())
    }

    /// As [`Self::update_targeted`], returning the exact `(inserted, deleted)`
    /// quads when the update is ground (`INSERT DATA` / `DELETE DATA` only),
    /// so derived indexes can be maintained incrementally; `None` otherwise.
    pub fn update_targeted_delta(
        &self,
        sparql: &str,
        affected_iris: &[String],
        full_rebuild: bool,
    ) -> Result<Option<QuadDelta>, StoreError> {
        let prefix_end = (0..=sparql.len().min(200))
            .rfind(|&i| sparql.is_char_boundary(i))
            .unwrap_or(0);
        debug!("Executing targeted update: {}", &sparql[..prefix_end]);
        // A ground update (INSERT DATA / DELETE DATA only) has an exactly
        // knowable effect per graph: count it before executing and adjust the
        // index afterwards, instead of rescanning every affected graph — that
        // recount made a 500-quad insert into a 900k-quad graph cost a
        // 900k-quad scan (the HTTP write stall the scale benchmark measured).
        let exact = if full_rebuild {
            None
        } else {
            self.ground_update_delta(sparql)
        };
        self.query_options()
            .parse_update(sparql)?
            .on_store(&self.store)
            .execute()?;
        let mut result = None;
        match exact {
            Some((deltas, inserted, deleted)) if !affected_iris.is_empty() => {
                result = Some((inserted, deleted));
                for (graph, delta) in deltas {
                    let known = self.graph_index.get_count(graph.as_deref()).is_some();
                    if known {
                        self.graph_index.adjust(graph.as_deref(), delta);
                    } else {
                        self.graph_index
                            .recount_specific_graphs(&self.store, std::slice::from_ref(&graph));
                    }
                }
            }
            _ => {
                if full_rebuild || affected_iris.is_empty() {
                    self.graph_index.rebuild(&self.store);
                } else {
                    let graphs: Vec<Option<String>> =
                        affected_iris.iter().map(|s| Some(s.clone())).collect();
                    self.graph_index
                        .recount_specific_graphs(&self.store, &graphs);
                }
            }
        }
        self.note_write();
        Ok(result)
    }

    /// For an update made only of `INSERT DATA` / `DELETE DATA`, the exact
    /// per-graph change it will make (duplicates and no-ops excluded, ground
    /// quads checked against the store). `None` for anything else.
    /// spargebra's graph name → oxrdf's (None for a blank-node graph name,
    /// which a ground update cannot name deterministically).
    fn oxrdf_graph_name(g: &spargebra::term::GraphName) -> Option<GraphName> {
        match g {
            spargebra::term::GraphName::NamedNode(n) => {
                Some(GraphName::NamedNode(NamedNode::new_unchecked(n.as_str())))
            }
            spargebra::term::GraphName::DefaultGraph => Some(GraphName::DefaultGraph),
        }
    }

    #[allow(clippy::type_complexity)]
    fn ground_update_delta(
        &self,
        sparql: &str,
    ) -> Option<(Vec<(Option<String>, i64)>, Vec<Quad>, Vec<Quad>)> {
        use spargebra::GraphUpdateOperation;
        let parsed = spargebra::SparqlParser::new().parse_update(sparql).ok()?;
        let mut deltas: std::collections::HashMap<Option<String>, i64> =
            std::collections::HashMap::new();
        let mut seen: std::collections::HashSet<Quad> = std::collections::HashSet::new();
        let mut inserted: Vec<Quad> = Vec::new();
        let mut deleted: Vec<Quad> = Vec::new();
        for op in &parsed.operations {
            match op {
                GraphUpdateOperation::InsertData { data } => {
                    for q in data {
                        let quad = Quad::new(
                            q.subject.clone(),
                            q.predicate.clone(),
                            q.object.clone(),
                            Self::oxrdf_graph_name(&q.graph_name)?,
                        );
                        let key = match &quad.graph_name {
                            GraphName::NamedNode(n) => Some(n.as_str().to_string()),
                            GraphName::DefaultGraph => None,
                            GraphName::BlankNode(_) => return None,
                        };
                        let fresh_bnode = matches!(quad.subject, NamedOrBlankNode::BlankNode(_))
                            || matches!(quad.object, Term::BlankNode(_));
                        if fresh_bnode
                            || (seen.insert(quad.clone())
                                && !self.store.contains(quad.as_ref()).ok()?)
                        {
                            *deltas.entry(key).or_insert(0) += 1;
                            inserted.push(quad);
                        }
                    }
                }
                GraphUpdateOperation::DeleteData { data } => {
                    for q in data {
                        let quad = Quad::new(
                            NamedOrBlankNode::NamedNode(q.subject.clone()),
                            q.predicate.clone(),
                            Term::from(q.object.clone()),
                            Self::oxrdf_graph_name(&q.graph_name)?,
                        );
                        let key = match &quad.graph_name {
                            GraphName::NamedNode(n) => Some(n.as_str().to_string()),
                            GraphName::DefaultGraph => None,
                            GraphName::BlankNode(_) => return None,
                        };
                        if seen.insert(quad.clone()) && self.store.contains(quad.as_ref()).ok()? {
                            *deltas.entry(key).or_insert(0) -= 1;
                            deleted.push(quad);
                        }
                    }
                }
                _ => return None,
            }
        }
        Some((deltas.into_iter().collect(), inserted, deleted))
    }

    /// Execute multiple SPARQL UPDATE statements in a single batch.
    ///
    /// All updates are parsed upfront (fail-fast on syntax errors), then
    /// executed sequentially. The graph index is rebuilt once at the end,
    /// amortising overhead across the entire batch (3-7x faster than
    /// individual `update()` calls).
    pub fn batch_update(
        &self,
        statements: &[String],
    ) -> Result<Vec<Result<(), String>>, StoreError> {
        // Parse all upfront
        let parsed: Vec<Result<SpargebraUpdate, String>> = statements
            .iter()
            .map(|s| {
                SparqlParser::new()
                    .parse_update(s)
                    .map_err(|e| e.to_string())
            })
            .collect();

        let opts = self.query_options();
        let mut results = Vec::with_capacity(statements.len());

        for update in parsed {
            match update {
                Ok(u) => match opts.clone().for_update(u).on_store(&self.store).execute() {
                    Ok(()) => results.push(Ok(())),
                    Err(e) => results.push(Err(e.to_string())),
                },
                Err(e) => results.push(Err(e)),
            }
        }

        // Rebuild graph index once for the entire batch
        self.graph_index.rebuild(&self.store);
        self.note_write();
        Ok(results)
    }

    /// Load RDF data from a reader with the given format.
    ///
    /// Blank nodes are handled per the store's [`BlankNodeMode`]. In `Preserve`
    /// mode a default-graph load streams straight into the store; otherwise the
    /// batch is materialised so canonical labeling / Skolemization can run before
    /// insertion (those are whole-dataset transforms and need the full set).
    pub fn load_reader(
        &self,
        reader: impl BufRead,
        format: RdfFormat,
        to_graph: Option<&str>,
    ) -> Result<(), StoreError> {
        self.load_reader_with_base(reader, format, None, to_graph)
    }

    /// Like [`Self::load_reader`], but resolves relative IRIs in the input against
    /// `base_iri`. Used by the LDP layer so an idiomatic `<>`-rooted request body
    /// attaches to the target resource IRI instead of being rejected as schemeless.
    pub fn load_reader_with_base(
        &self,
        reader: impl BufRead,
        format: RdfFormat,
        base_iri: Option<&str>,
        to_graph: Option<&str>,
    ) -> Result<(), StoreError> {
        // Fast path: nothing to rewrite and no forced graph → stream directly.
        if self.blank_node_mode == BlankNodeMode::Preserve && to_graph.is_none() {
            // oxigraph 0.5: the bulk loader stages batches and only persists them on
            // an explicit `commit()` — dropping it without committing loses the data.
            let mut loader = self.store.bulk_loader();
            loader.load_from_reader(Self::parser_for(format, base_iri)?, reader)?;
            loader.commit()?;
            info!("Data loaded successfully (streamed)");
            self.graph_index.rebuild(&self.store);
            self.spatial_index.mark_dirty();
            #[cfg(feature = "geometry3d")]
            self.spatial_index_3d.mark_dirty();
            self.note_write();
            return Ok(());
        }

        let quads = self.parse_quads(reader, format, base_iri, to_graph)?;
        self.insert_quads_and_reindex(quads, to_graph).map(|_| ())
    }

    /// As [`Self::load_str`] into a named graph, returning the quads that were
    /// actually new (so callers can update derived indexes incrementally).
    pub fn load_str_delta(
        &self,
        data: &str,
        format: RdfFormat,
        to_graph: &str,
    ) -> Result<Vec<Quad>, StoreError> {
        let reader = BufReader::new(data.as_bytes());
        let quads = self.parse_quads(reader, format, None, Some(to_graph))?;
        self.insert_quads_and_reindex(quads, Some(to_graph))
    }

    /// Parse `reader` into quads, retargeting them into `to_graph` when one is
    /// given and applying the durable blank-node policy.
    ///
    /// Split out of [`Self::load_reader_with_base`] so a caller that must not
    /// mutate the store until the input is known-good — Graph Store PUT, which
    /// replaces a graph — can parse first and only then clear.
    fn parse_quads(
        &self,
        reader: impl BufRead,
        format: RdfFormat,
        base_iri: Option<&str>,
        to_graph: Option<&str>,
    ) -> Result<Vec<Quad>, StoreError> {
        // Embedded graph names from NQuads/TriG are preserved; triple formats
        // land in the default graph. Parse errors are propagated.
        let mut quads: Vec<Quad> = Self::parser_for(format, base_iri)?
            .for_reader(reader)
            .map(|r| r.map_err(|e| StoreError::Parse(e.to_string())))
            .collect::<Result<Vec<_>, _>>()?;

        // Force everything into the target graph if one was requested.
        if let Some(graph_iri) = to_graph {
            let target = GraphName::NamedNode(
                NamedNode::new(graph_iri)
                    .map_err(|e| StoreError::Parse(format!("Invalid graph IRI: {}", e)))?,
            );
            quads = quads
                .into_iter()
                .map(|q| Quad::new(q.subject, q.predicate, q.object, target.clone()))
                .collect();
        }

        Ok(self.apply_blank_node_mode(quads))
    }

    /// Bulk-insert already-parsed quads and run the post-write index bookkeeping.
    fn insert_quads_and_reindex(
        &self,
        quads: Vec<Quad>,
        to_graph: Option<&str>,
    ) -> Result<Vec<Quad>, StoreError> {
        // A graph-targeted load knows exactly how many quads are new (duplicates
        // within the batch and quads already stored do not count), so the graph
        // index is bumped by that number. Recounting the graph after every load
        // made each write O(graph): under the scale benchmark a 500-quad insert
        // into a 900k-quad graph took seconds, and concurrent writers stalled.
        let new_quads: Option<Vec<Quad>> = if to_graph.is_some() {
            let mut seen: std::collections::HashSet<&Quad> =
                std::collections::HashSet::with_capacity(quads.len());
            let mut fresh = Vec::new();
            for q in &quads {
                if seen.insert(q) && !self.store.contains(q.as_ref())? {
                    fresh.push(q.clone());
                }
            }
            Some(fresh)
        } else {
            None
        };
        let mut loader = self.store.bulk_loader();
        loader.load_quads(quads)?;
        loader.commit()?; // oxigraph 0.5: stage-then-commit (see load_reader_with_base).

        info!("Data loaded successfully");
        let delta = match (to_graph, new_quads) {
            (Some(g), Some(fresh)) => {
                if self.graph_index.get_count(Some(g)).is_some() {
                    self.graph_index.add(Some(g), fresh.len());
                } else {
                    self.graph_index
                        .recount_specific_graphs(&self.store, &[Some(g.to_string())]);
                }
                fresh
            }
            _ => {
                self.graph_index.rebuild(&self.store);
                Vec::new()
            }
        };
        self.spatial_index.mark_dirty();
        #[cfg(feature = "geometry3d")]
        self.spatial_index_3d.mark_dirty();
        self.note_write();
        Ok(delta)
    }

    /// Load RDF data from a file, auto-detecting format from extension.
    pub fn load_file(&self, path: &Path) -> Result<(), StoreError> {
        let format = detect_format_from_path(path)?;
        let file = std::fs::File::open(path)?;
        let reader = BufReader::new(file);
        self.load_reader(reader, format, None)
    }

    /// Load RDF data from a string with the given format.
    pub fn load_str(
        &self,
        data: &str,
        format: RdfFormat,
        to_graph: Option<&str>,
    ) -> Result<(), StoreError> {
        let reader = BufReader::new(data.as_bytes());
        self.load_reader(reader, format, to_graph)
    }

    /// Load RDF data from a string, resolving relative IRIs against `base_iri`.
    pub fn load_str_with_base(
        &self,
        data: &str,
        format: RdfFormat,
        base_iri: &str,
        to_graph: Option<&str>,
    ) -> Result<(), StoreError> {
        let reader = BufReader::new(data.as_bytes());
        self.load_reader_with_base(reader, format, Some(base_iri), to_graph)
    }

    /// Build an [`RdfParser`] for `format`, optionally resolving relative IRIs
    /// against `base_iri`.
    fn parser_for(format: RdfFormat, base_iri: Option<&str>) -> Result<RdfParser, StoreError> {
        let parser = RdfParser::from_format(format);
        match base_iri {
            Some(base) => parser
                .with_base_iri(base)
                .map_err(|e| StoreError::Parse(format!("Invalid base IRI '{base}': {e}"))),
            None => Ok(parser),
        }
    }

    /// Stream all triples from a graph in the specified format into `writer`.
    ///
    /// This is the primitive used by both the buffered [`Self::dump`] helper and
    /// the streaming HTTP response path — callers that want to avoid buffering
    /// multi-MB results in memory can pass an `axum`-backed writer directly.
    pub fn dump_to_writer<W: Write>(
        &self,
        mut writer: W,
        format: RdfFormat,
        from_graph: Option<&str>,
    ) -> Result<(), StoreError> {
        let graph = match from_graph {
            Some(g) => GraphNameRef::NamedNode(
                NamedNodeRef::new(g)
                    .map_err(|e| StoreError::Parse(format!("Invalid IRI: {}", e)))?,
            ),
            None => GraphNameRef::DefaultGraph,
        };

        let serializer = RdfSerializer::from_format(format);
        let mut ser = serializer.for_writer(&mut writer);
        for quad in self.store.quads_for_pattern(None, None, None, Some(graph)) {
            let quad = quad?;
            ser.serialize_triple(quad.as_ref())?;
        }
        ser.finish()?;
        Ok(())
    }

    /// Stream every quad in the store (default graph + all named graphs)
    /// into `writer` as N-Quads. Used by the backup subsystem; preserves
    /// graph names so a fresh store can be reconstructed by re-importing.
    pub fn dump_all_nquads<W: Write>(&self, mut writer: W) -> Result<usize, StoreError> {
        let serializer = RdfSerializer::from_format(RdfFormat::NQuads);
        let mut ser = serializer.for_writer(&mut writer);
        let mut count = 0usize;
        for quad in self.store.iter() {
            let quad = quad?;
            ser.serialize_quad(quad.as_ref())?;
            count += 1;
        }
        ser.finish()?;
        Ok(count)
    }

    /// Dump all triples from a graph in the specified format into a `Vec<u8>`.
    ///
    /// Convenience wrapper over [`Self::dump_to_writer`] for callers that need
    /// the bytes in memory (e.g. label-filtering re-serialization, in-process
    /// validation). Streaming HTTP paths should call `dump_to_writer` directly.
    pub fn dump(&self, format: RdfFormat, from_graph: Option<&str>) -> Result<Vec<u8>, StoreError> {
        let approx = self.graph_index.get_count(from_graph).unwrap_or(0);
        let mut buffer: Vec<u8> =
            Vec::with_capacity(approx.saturating_mul(80).min(8 * 1024 * 1024));
        self.dump_to_writer(&mut buffer, format, from_graph)?;
        Ok(buffer)
    }

    /// Graph Store Protocol: GET a named graph.
    pub fn graph_store_get(
        &self,
        graph_iri: Option<&str>,
        format: RdfFormat,
    ) -> Result<Vec<u8>, StoreError> {
        self.dump(format, graph_iri)
    }

    /// Graph Store Protocol: PUT (replace) a named graph.
    pub fn graph_store_put(
        &self,
        graph_iri: Option<&str>,
        data: &str,
        format: RdfFormat,
    ) -> Result<(), StoreError> {
        let graph_name = match graph_iri {
            Some(iri) => GraphNameRef::NamedNode(
                NamedNodeRef::new(iri)
                    .map_err(|e| StoreError::Parse(format!("Invalid IRI: {}", e)))?,
            ),
            None => GraphNameRef::DefaultGraph,
        };

        // Parse BEFORE clearing. Clearing first meant a malformed body destroyed
        // the graph and then failed to replace it: the caller got a 4xx while the
        // graph was left empty, with no way to recover it. PUT is a replace, so
        // the body is already fully in memory — materialising the quads costs
        // nothing extra and makes the replace all-or-nothing.
        let quads = self.parse_quads(BufReader::new(data.as_bytes()), format, None, graph_iri)?;

        self.clear_graph_chunked(graph_name)?;
        self.insert_quads_and_reindex(quads, graph_iri).map(|_| ())
    }

    /// Graph Store Protocol: POST (merge into) a named graph.
    pub fn graph_store_post(
        &self,
        graph_iri: Option<&str>,
        data: &str,
        format: RdfFormat,
    ) -> Result<(), StoreError> {
        self.load_str(data, format, graph_iri)
    }

    /// As [`Self::graph_store_post`], returning the quads that were new when
    /// the target is a named graph (empty for the default graph).
    pub fn graph_store_post_delta(
        &self,
        graph_iri: Option<&str>,
        data: &str,
        format: RdfFormat,
    ) -> Result<Vec<Quad>, StoreError> {
        match graph_iri {
            Some(g) => self.load_str_delta(data, format, g),
            None => self.load_str(data, format, None).map(|_| Vec::new()),
        }
    }

    /// Graph Store Protocol: DELETE a named graph.
    /// Empty a graph in chunks of transactions. `Store::clear_graph` walks the
    /// graph and deletes quad by quad inside one transaction; on a 900k-quad
    /// graph that took ~33 s, and collecting the quads first then deleting
    /// them in 50k-quad transactions takes ~22 s — the same work in RocksDB,
    /// but without one write batch the size of the graph.
    fn clear_graph_chunked(&self, graph_name: GraphNameRef<'_>) -> Result<(), StoreError> {
        let quads: Vec<Quad> = self
            .store
            .quads_for_pattern(None, None, None, Some(graph_name))
            .collect::<Result<_, _>>()?;
        if quads.len() < 100_000 {
            self.store.clear_graph(graph_name)?;
            return Ok(());
        }
        for chunk in quads.chunks(50_000) {
            let mut tx = self.store.start_transaction()?;
            for q in chunk {
                tx.remove(q.as_ref());
            }
            tx.commit()?;
        }
        Ok(())
    }

    pub fn graph_store_delete(&self, graph_iri: Option<&str>) -> Result<(), StoreError> {
        let graph_name = match graph_iri {
            Some(iri) => GraphNameRef::NamedNode(
                NamedNodeRef::new(iri)
                    .map_err(|e| StoreError::Parse(format!("Invalid IRI: {}", e)))?,
            ),
            None => GraphNameRef::DefaultGraph,
        };
        self.clear_graph_chunked(graph_name)?;
        if let Some(iri) = graph_iri {
            let nn = NamedNode::new(iri)
                .map_err(|e| StoreError::Parse(format!("Invalid IRI: {}", e)))?;
            self.store.remove_named_graph(&nn)?;
        }
        self.graph_index.remove(graph_iri);
        self.note_write();
        Ok(())
    }

    /// Delete multiple named graphs in a single SPARQL UPDATE transaction.
    ///
    /// Batches all `DROP SILENT GRAPH` operations into one `Update::parse()` +
    /// `update_opt()` call, avoiding N separate Oxigraph write transactions.
    /// Graph index entries are removed in one pass after the update.
    pub fn bulk_delete_graphs(&self, graph_iris: &[&str]) -> Result<(), StoreError> {
        if graph_iris.is_empty() {
            return Ok(());
        }

        // Validate all IRIs upfront before touching the store.
        for iri in graph_iris {
            NamedNodeRef::new(iri)
                .map_err(|e| StoreError::Parse(format!("Invalid IRI '{}': {}", iri, e)))?;
        }

        // Build a single SPARQL UPDATE with all DROP SILENT GRAPH statements.
        let sparql: String = graph_iris
            .iter()
            .map(|iri| format!("DROP SILENT GRAPH <{iri}>"))
            .collect::<Vec<_>>()
            .join(" ; ");

        self.query_options()
            .parse_update(&sparql)?
            .on_store(&self.store)
            .execute()?;

        // Remove from in-memory graph index.
        for iri in graph_iris {
            self.graph_index.remove(Some(iri));
        }
        self.note_write();
        Ok(())
    }

    /// Insert multiple quads using Oxigraph's bulk loader.
    ///
    /// Significantly faster than individual `store_quad()` calls for large
    /// batches. After insertion, only `affected_graphs` are re-counted in the
    /// graph index (O(affected_graphs) instead of O(all_graphs)).
    pub fn bulk_insert_quads(
        &self,
        quads: Vec<Quad>,
        affected_graphs: &[String],
    ) -> Result<(), StoreError> {
        if !quads.is_empty() {
            let mut loader = self.store.bulk_loader();
            loader.load_quads(quads)?;
            loader.commit()?; // oxigraph 0.5: stage-then-commit.
            self.spatial_index.mark_dirty();
            #[cfg(feature = "geometry3d")]
            self.spatial_index_3d.mark_dirty();
        }

        // Register (or recount) only the affected graphs in the index.
        let iris: Vec<Option<String>> = affected_graphs.iter().map(|s| Some(s.clone())).collect();
        self.graph_index.recount_specific_graphs(&self.store, &iris);
        self.note_write();
        Ok(())
    }

    /// Get the number of quads in the store.
    pub fn len(&self) -> Result<usize, StoreError> {
        Ok(self.store.len()?)
    }

    /// Check if the store is empty.
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        Ok(self.store.is_empty()?)
    }

    /// List all named graphs.
    pub fn named_graphs(&self) -> Result<Vec<NamedNode>, StoreError> {
        let mut graphs = Vec::new();
        for g in self.store.named_graphs() {
            let g = g?;
            if let NamedOrBlankNode::NamedNode(nn) = g {
                graphs.push(nn);
            }
        }
        Ok(graphs)
    }

    /// O(1) approximate total triple count read from the maintained per-graph
    /// count index (lock-free `DashMap`), summing every graph including the
    /// default graph. Unlike [`Self::len`] this never scans RocksDB, so it is
    /// safe to call from the `/health` probe even while a large import holds the
    /// store under write pressure — the historical cause of the health-check
    /// timing out and flapping the container during a heavy seed. The count may
    /// lag a single-quad write that bypassed index maintenance, which is
    /// acceptable for a health signal.
    pub fn cached_total_triples(&self) -> usize {
        self.graph_index.all_entries().iter().map(|(_, n)| *n).sum()
    }

    /// O(1) count of *named* graphs from the maintained index — the `None`
    /// default-graph entry is excluded so this matches [`Self::named_graphs`]
    /// length semantics without an index scan.
    pub fn cached_named_graph_count(&self) -> usize {
        self.graph_index
            .all_entries()
            .iter()
            .filter(|(g, _)| g.is_some())
            .count()
    }

    /// Insert a single quad into the store.
    pub fn store_quad(&self, quad: Quad) -> Result<(), StoreError> {
        self.store.insert(&quad)?;
        self.note_write();
        Ok(())
    }

    /// Return all quads from a specific named graph.
    pub fn quads_for_graph(&self, graph: GraphNameRef<'_>) -> Result<Vec<Quad>, StoreError> {
        let quads = self
            .store
            .quads_for_pattern(None, None, None, Some(graph))
            .collect::<Result<Vec<Quad>, _>>()?;
        Ok(quads)
    }

    /// Count quads `(?s <predicate> <object>)` inside a named graph via a direct
    /// index scan — no SPARQL parse, no query-accelerator involvement. Used by
    /// hot probes (shape detection after an import) that must stay cheap even
    /// while the in-memory mirror is stale and rebuilding.
    pub fn count_pattern_in_graph(
        &self,
        graph_iri: &str,
        predicate: &str,
        object: &str,
    ) -> Result<usize, StoreError> {
        use oxigraph::model::TermRef;
        let g = NamedNodeRef::new(graph_iri)
            .map_err(|e| StoreError::Parse(format!("Invalid IRI: {}", e)))?;
        let p = NamedNodeRef::new(predicate)
            .map_err(|e| StoreError::Parse(format!("Invalid IRI: {}", e)))?;
        let o = NamedNodeRef::new(object)
            .map_err(|e| StoreError::Parse(format!("Invalid IRI: {}", e)))?;
        Ok(self
            .store
            .quads_for_pattern(
                None,
                Some(p),
                Some(TermRef::NamedNode(o)),
                Some(GraphNameRef::NamedNode(g)),
            )
            .count())
    }

    /// Return at most `limit` quads from a named graph.
    ///
    /// For prevalence-based work (content-kind classification) a bounded sample
    /// is as good as the whole graph, and materialising a multi-million-quad
    /// graph just to count type signals held the import request for seconds.
    pub fn quads_for_graph_sample(
        &self,
        graph: GraphNameRef<'_>,
        limit: usize,
    ) -> Result<Vec<Quad>, StoreError> {
        let quads = self
            .store
            .quads_for_pattern(None, None, None, Some(graph))
            .take(limit)
            .collect::<Result<Vec<Quad>, _>>()?;
        Ok(quads)
    }

    // ── Performance optimisations ────────────────────────────────────────

    /// Fast count of quads in a specific graph without SPARQL overhead.
    ///
    /// Uses Oxigraph's `quads_for_pattern()` iterator directly, avoiding
    /// SPARQL parsing and solution materialisation (3-5x faster than
    /// `SELECT (COUNT(*) AS ?c) WHERE { … }`).
    pub fn count_graph(&self, graph_iri: Option<&str>) -> Result<usize, StoreError> {
        let graph = match graph_iri {
            Some(iri) => GraphNameRef::NamedNode(
                NamedNodeRef::new(iri)
                    .map_err(|e| StoreError::Parse(format!("Invalid IRI: {}", e)))?,
            ),
            None => GraphNameRef::DefaultGraph,
        };
        Ok(self
            .store
            .quads_for_pattern(None, None, None, Some(graph))
            .count())
    }

    /// Return cached graph counts from the in-memory graph index.
    ///
    /// Returns `(Option<iri>, count)` pairs. `None` represents the default graph.
    /// O(1) — no index scans required.
    pub fn graph_counts(&self) -> Vec<(Option<String>, usize)> {
        self.graph_index.all_entries()
    }

    /// Get the cached count for a specific graph.
    pub fn graph_count_cached(&self, graph_iri: Option<&str>) -> Option<usize> {
        self.graph_index.get_count(graph_iri)
    }

    /// Rebuild the graph index (e.g. after external writes).
    pub fn rebuild_graph_index(&self) {
        self.graph_index.rebuild(&self.store);
        self.note_write();
    }

    /// Access the spatial R-tree index for GeoSPARQL pre-filtering.
    ///
    /// Lazily rebuilds the index if marked dirty (after writes to `geo:asWKT` triples).
    pub fn spatial_index(&self) -> &SpatialIndex {
        if self.spatial_index.is_dirty() {
            self.spatial_index.rebuild(&self.store);
        }
        &self.spatial_index
    }

    /// Rebuild the spatial index explicitly.
    pub fn rebuild_spatial_index(&self) {
        self.spatial_index.rebuild(&self.store);
    }

    /// Access the 3D R*-tree index (volumetric broad phase for `ots-geof:` 3D
    /// relations and the 3D-Tiles/OGC bbox pre-filter).
    ///
    /// Lazily rebuilds the index if marked dirty (after writes to `geo:asWKT`
    /// triples), exactly mirroring [`spatial_index`](Self::spatial_index).
    #[cfg(feature = "geometry3d")]
    pub fn spatial_index_3d(&self) -> &SpatialIndex3D {
        if self.spatial_index_3d.is_dirty() {
            self.spatial_index_3d.rebuild(&self.store);
        }
        &self.spatial_index_3d
    }

    /// Rebuild the 3D spatial index explicitly.
    #[cfg(feature = "geometry3d")]
    pub fn rebuild_spatial_index_3d(&self) {
        self.spatial_index_3d.rebuild(&self.store);
    }

    /// Iterate quads matching a specific predicate (P-S-O index).
    ///
    /// Leverages Oxigraph's POS index directly without SPARQL overhead.
    /// Useful for SHACL target resolution and reasoning rules.
    pub fn quads_for_predicate(&self, predicate: &str) -> Result<Vec<Quad>, StoreError> {
        let pred = NamedNodeRef::new(predicate)
            .map_err(|e| StoreError::Parse(format!("Invalid predicate IRI: {}", e)))?;
        let quads = self
            .store
            .quads_for_pattern(None, Some(pred), None, None)
            .collect::<Result<Vec<Quad>, _>>()?;
        Ok(quads)
    }

    /// Objects of `<blank> <predicate> ?o`, where the subject is a *stored* blank
    /// node addressed by its label (the value returned by a prior query, without
    /// the leading `_:`). SPARQL surface syntax cannot reference a specific
    /// stored blank node — `_:x` in a query is a fresh existential — so this goes
    /// straight to the raw quad index. Scoped to `data_graphs` (empty = the
    /// default graph), matching the SHACL engine's `graph_scoped` semantics.
    pub fn blank_subject_objects(
        &self,
        blank_label: &str,
        predicate: &str,
        data_graphs: &[String],
    ) -> Vec<Term> {
        let subject = BlankNode::new_unchecked(blank_label);
        let pred = match NamedNodeRef::new(predicate) {
            Ok(p) => p,
            Err(_) => return Vec::new(),
        };
        let subj_ref = NamedOrBlankNodeRef::BlankNode(subject.as_ref());
        let collect = |graph: GraphNameRef<'_>| -> Vec<Term> {
            self.store
                .quads_for_pattern(Some(subj_ref), Some(pred), None, Some(graph))
                .filter_map(|q| q.ok().map(|q| q.object))
                .collect()
        };
        if data_graphs.is_empty() {
            collect(GraphNameRef::DefaultGraph)
        } else {
            let mut out = Vec::new();
            for g in data_graphs {
                if let Ok(gn) = NamedNodeRef::new(g) {
                    out.extend(collect(GraphNameRef::NamedNode(gn)));
                }
            }
            out
        }
    }

    /// Objects of `subject predicate ?o` within a single graph, where `subject`
    /// may be an IRI or a *stored* blank node (a `_:label` string). Goes through
    /// the raw quad index so blank-node subjects — which SPARQL surface syntax
    /// cannot name — are addressable. Used by the SHACL loader to walk RDF lists
    /// (`( … )` cells are blank nodes) and resolve blank-node shape attributes.
    pub fn objects_for_subject_in_graph(
        &self,
        subject: &str,
        predicate: &str,
        graph: Option<&str>,
    ) -> Vec<Term> {
        let subj: NamedOrBlankNode = match subject.strip_prefix("_:") {
            Some(label) => NamedOrBlankNode::BlankNode(BlankNode::new_unchecked(label)),
            None => match NamedNode::new(subject) {
                Ok(nn) => NamedOrBlankNode::NamedNode(nn),
                Err(_) => return Vec::new(),
            },
        };
        let pred = match NamedNodeRef::new(predicate) {
            Ok(p) => p,
            Err(_) => return Vec::new(),
        };
        let graph_name = match graph {
            Some(g) => match NamedNodeRef::new(g) {
                Ok(gn) => GraphNameRef::NamedNode(gn),
                Err(_) => return Vec::new(),
            },
            None => GraphNameRef::DefaultGraph,
        };
        self.store
            .quads_for_pattern(Some(subj.as_ref()), Some(pred), None, Some(graph_name))
            .filter_map(|q| q.ok().map(|q| q.object))
            .collect()
    }
}

/// Detect RDF format from file extension.
pub fn detect_format_from_path(path: &Path) -> Result<RdfFormat, StoreError> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("ttl") | Some("turtle") => Ok(RdfFormat::Turtle),
        Some("nt") | Some("ntriples") => Ok(RdfFormat::NTriples),
        Some("rdf") | Some("xml") | Some("rdfxml") => Ok(RdfFormat::RdfXml),
        Some("nq") | Some("nquads") => Ok(RdfFormat::NQuads),
        Some("trig") => Ok(RdfFormat::TriG),
        Some(ext) => Err(StoreError::UnsupportedFormat(ext.to_string())),
        None => Err(StoreError::UnsupportedFormat("no extension".to_string())),
    }
}

// ── Fast-COUNT(*) shape recognition (see TripleStore::try_fast_count) ─────────────

/// If `pattern` is `SELECT (COUNT(*) AS ?v)` over a single full-scan triple
/// pattern, return the projected variable name `v`; otherwise `None`.
/// What a `COUNT(*)` full scan ranges over.
enum CountScope {
    Default,
    Named(String),
    AnyNamed,
}

fn count_star_var(pattern: &GraphPattern) -> Option<(String, CountScope)> {
    if let GraphPattern::Project { inner, variables } = pattern {
        if variables.len() == 1 {
            if let Some(scope) = count_star_scope(inner) {
                return Some((variables[0].as_str().to_string(), scope));
            }
        }
    }
    None
}

fn count_star_scope(p: &GraphPattern) -> Option<CountScope> {
    match p {
        GraphPattern::Extend {
            inner,
            expression: Expression::Variable(_),
            ..
        } => count_star_scope(inner),
        GraphPattern::Group {
            inner,
            variables,
            aggregates,
        } => {
            if !variables.is_empty()
                || aggregates.len() != 1
                || !matches!(
                    &aggregates[0].1,
                    AggregateExpression::CountSolutions { distinct: false }
                )
            {
                return None;
            }
            full_scan_scope(inner)
        }
        _ => None,
    }
}

/// `{ ?s ?p ?o }`, `GRAPH <g> { ?s ?p ?o }` or `GRAPH ?g { ?s ?p ?o }` — and
/// nothing else in the pattern.
fn full_scan_scope(p: &GraphPattern) -> Option<CountScope> {
    match p {
        GraphPattern::Bgp { .. } if is_full_scan_bgp(p) => Some(CountScope::Default),
        GraphPattern::Graph { name, inner } if is_full_scan_bgp(inner) => match name {
            NamedNodePattern::NamedNode(n) => Some(CountScope::Named(n.as_str().to_string())),
            NamedNodePattern::Variable(v) => {
                // Only when ?g is not one of the triple's own variables.
                if let GraphPattern::Bgp { patterns } = inner.as_ref() {
                    let tp = &patterns[0];
                    let used = |t: &TermPattern| matches!(t, TermPattern::Variable(x) if x == v);
                    if used(&tp.subject)
                        || used(&tp.object)
                        || matches!(&tp.predicate, NamedNodePattern::Variable(x) if x == v)
                    {
                        return None;
                    }
                }
                Some(CountScope::AnyNamed)
            }
        },
        _ => None,
    }
}

fn is_full_scan_bgp(p: &GraphPattern) -> bool {
    matches!(p, GraphPattern::Bgp { patterns } if patterns.len() == 1 && all_distinct_vars(&patterns[0]))
}

fn all_distinct_vars(tp: &TriplePattern) -> bool {
    let s = match &tp.subject {
        TermPattern::Variable(v) => v.as_str(),
        _ => return false,
    };
    let p = match &tp.predicate {
        NamedNodePattern::Variable(v) => v.as_str(),
        _ => return false,
    };
    let o = match &tp.object {
        TermPattern::Variable(v) => v.as_str(),
        _ => return false,
    };
    s != p && p != o && s != o
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn fast_count_matches_normal_eval() {
        let store = TripleStore::in_memory().unwrap();
        // default graph: 3 triples
        store
            .load_str(
                "<http://e/a> <http://e/p> \"1\" . <http://e/b> <http://e/p> \"2\" . <http://e/c> <http://e/p> \"3\" .",
                RdfFormat::Turtle,
                None,
            )
            .unwrap();
        // named graph <http://g>: 2 triples
        store
            .load_str(
                "<http://e/x> <http://e/p> \"1\" . <http://e/y> <http://e/p> \"2\" .",
                RdfFormat::Turtle,
                Some("http://g"),
            )
            .unwrap();

        let count = |q: &str| -> String {
            match store.query(q).unwrap() {
                QueryResults::Solutions(s) => {
                    let sol = s.into_iter().next().unwrap().unwrap();
                    sol.get("c").map(|t| t.to_string()).unwrap_or_default()
                }
                _ => panic!("expected solutions"),
            }
        };

        // Fast path: default-graph count = 3.
        assert!(count("SELECT (COUNT(*) AS ?c) WHERE { ?s ?p ?o }").contains("\"3\""));
        // Fast path: FROM <g> counts only that graph = 2.
        assert!(
            count("SELECT (COUNT(*) AS ?c) FROM <http://g> WHERE { ?s ?p ?o }").contains("\"2\"")
        );
        // A COUNT with a FILTER must NOT be short-circuited — it must reflect the
        // filter (one default-graph triple has object "1"), proving the fast path
        // is skipped for anything but a bare full scan.
        assert!(
            count("SELECT (COUNT(*) AS ?c) WHERE { ?s ?p ?o FILTER(STR(?o) = \"1\") }")
                .contains("\"1\"")
        );
        // A two-pattern BGP COUNT is also not the bare-scan shape → normal eval.
        assert!(count(
            "SELECT (COUNT(*) AS ?c) WHERE { ?s <http://e/p> ?o . ?s <http://e/p> ?o2 }"
        )
        .contains("\"3\""));
    }

    fn detect_format_from_mime(mime: &str) -> Result<RdfFormat, StoreError> {
        let mime = mime.split(';').next().unwrap_or(mime).trim();
        match mime {
            "text/turtle" | "application/x-turtle" => Ok(RdfFormat::Turtle),
            "application/n-triples" | "text/plain" => Ok(RdfFormat::NTriples),
            "application/rdf+xml" | "application/xml" => Ok(RdfFormat::RdfXml),
            "application/n-quads" | "text/x-nquads" => Ok(RdfFormat::NQuads),
            "application/trig" => Ok(RdfFormat::TriG),
            _ => Err(StoreError::UnsupportedFormat(mime.to_string())),
        }
    }

    #[test]
    fn test_in_memory_store() {
        let store = TripleStore::in_memory().unwrap();
        assert!(store.is_empty().unwrap());
    }

    // ── Durable blank nodes (opengraph integration) ─────────────────────────

    /// Distinct blank-node labels currently in the store (subject or object).
    fn distinct_bnodes(store: &TripleStore) -> std::collections::BTreeSet<String> {
        let mut set = std::collections::BTreeSet::new();
        for quad in store.store().iter() {
            let quad = quad.unwrap();
            if let NamedOrBlankNode::BlankNode(b) = &quad.subject {
                set.insert(b.as_str().to_string());
            }
            if let Term::BlankNode(b) = &quad.object {
                set.insert(b.as_str().to_string());
            }
        }
        set
    }

    /// The single anonymous node here is the durability test subject.
    const BNODE_TTL: &str = "@prefix ex: <http://example.org/> .\nex:a ex:p [ ex:v \"1\" ] .";

    #[test]
    fn preserve_mode_duplicates_blank_nodes_on_reimport() {
        // Legacy behaviour: each parse mints a fresh label, so re-importing the
        // same anonymous node creates a second, differently-labelled one.
        let store = TripleStore::in_memory()
            .unwrap()
            .with_blank_node_mode(BlankNodeMode::Preserve);
        store.load_str(BNODE_TTL, RdfFormat::Turtle, None).unwrap();
        store.load_str(BNODE_TTL, RdfFormat::Turtle, None).unwrap();
        assert_eq!(
            distinct_bnodes(&store).len(),
            2,
            "preserve mode duplicates the bnode"
        );
    }

    #[test]
    fn canonical_mode_makes_blank_nodes_durable_and_idempotent() {
        // Durable behaviour: the same anonymous structure always gets the same
        // content-derived label, so re-importing is idempotent (no duplicate) and
        // the label is stable across reloads.
        let store = TripleStore::in_memory()
            .unwrap()
            .with_blank_node_mode(BlankNodeMode::Canonical);
        store.load_str(BNODE_TTL, RdfFormat::Turtle, None).unwrap();
        let first = distinct_bnodes(&store);
        store.load_str(BNODE_TTL, RdfFormat::Turtle, None).unwrap();
        let second = distinct_bnodes(&store);
        assert_eq!(first.len(), 1, "one anonymous node");
        assert_eq!(
            first, second,
            "re-import must reproduce the same durable label"
        );
        assert!(
            first
                .iter()
                .next()
                .unwrap()
                .starts_with(opengraph::canonical::STABLE_PREFIX),
            "label should be a stable content hash"
        );
    }

    #[test]
    fn skolem_mode_replaces_blank_nodes_with_genid_iris() {
        let store = TripleStore::in_memory()
            .unwrap()
            .with_blank_node_mode(BlankNodeMode::Skolem);
        store.load_str(BNODE_TTL, RdfFormat::Turtle, None).unwrap();
        // No blank nodes remain; the anonymous node is now a durable genid IRI.
        assert!(
            distinct_bnodes(&store).is_empty(),
            "no blank nodes after Skolemization"
        );
        let has_genid = store.store().iter().any(|q| {
            let q = q.unwrap();
            matches!(&q.object, Term::NamedNode(n) if n.as_str().contains("/.well-known/genid/"))
        });
        assert!(has_genid, "blank node should become a genid IRI");
    }

    #[test]
    fn test_load_and_query() {
        let store = TripleStore::in_memory().unwrap();
        let ttl = r#"
            @prefix ex: <http://example.org/> .
            ex:alice ex:name "Alice" .
            ex:alice ex:age 30 .
            ex:bob ex:name "Bob" .
        "#;
        store.load_str(ttl, RdfFormat::Turtle, None).unwrap();
        assert_eq!(store.len().unwrap(), 3);

        let results = store
            .query("SELECT ?name WHERE { ?s <http://example.org/name> ?name } ORDER BY ?name")
            .unwrap();
        if let QueryResults::Solutions(solutions) = results {
            let names: Vec<String> = solutions
                .map(|s| s.unwrap().get("name").unwrap().to_string())
                .collect();
            assert_eq!(names.len(), 2);
            assert!(names[0].contains("Alice"));
            assert!(names[1].contains("Bob"));
        } else {
            panic!("Expected SELECT results");
        }
    }

    #[test]
    #[cfg_attr(
        all(target_os = "macos", target_arch = "aarch64"),
        ignore = "Oxigraph RocksDB TryFromIntError on macOS arm64 in test context — runs on Linux/CI"
    )]
    fn test_persistent_store() {
        // Use a unique subdirectory to avoid RocksDB lock conflicts
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("persistent_test");
        std::fs::create_dir_all(&db_path).unwrap();
        {
            let store = TripleStore::open(&db_path).unwrap();
            store
                .update("INSERT DATA { <http://example.org/s> <http://example.org/p> \"hello\" }")
                .unwrap();
            assert_eq!(store.len().unwrap(), 1);
            // Explicitly drop before re-opening
            drop(store);
        }
        // Reopen and verify data persisted
        {
            let store = TripleStore::open(&db_path).unwrap();
            assert_eq!(store.len().unwrap(), 1);
        }
    }

    #[test]
    fn test_sparql_update() {
        let store = TripleStore::in_memory().unwrap();
        store
            .update("INSERT DATA { <http://example.org/s> <http://example.org/p> \"value\" }")
            .unwrap();
        assert_eq!(store.len().unwrap(), 1);

        store
            .update("DELETE DATA { <http://example.org/s> <http://example.org/p> \"value\" }")
            .unwrap();
        assert_eq!(store.len().unwrap(), 0);
    }

    #[test]
    fn test_named_graphs() {
        let store = TripleStore::in_memory().unwrap();
        store
            .load_str(
                "<http://example.org/s> <http://example.org/p> \"v\" .",
                RdfFormat::NTriples,
                Some("http://example.org/graph1"),
            )
            .unwrap();
        let graphs = store.named_graphs().unwrap();
        assert_eq!(graphs.len(), 1);
        assert_eq!(graphs[0].as_str(), "http://example.org/graph1");
    }

    #[test]
    fn test_format_detection() {
        assert!(matches!(
            detect_format_from_path(Path::new("test.ttl")),
            Ok(RdfFormat::Turtle)
        ));
        assert!(matches!(
            detect_format_from_path(Path::new("test.nt")),
            Ok(RdfFormat::NTriples)
        ));
        assert!(matches!(
            detect_format_from_mime("text/turtle"),
            Ok(RdfFormat::Turtle)
        ));
        assert!(matches!(
            detect_format_from_mime("application/n-triples"),
            Ok(RdfFormat::NTriples)
        ));
    }

    // ── Surgical graph-index maintenance on store.update() ──────────────────

    /// Regression guard for the post-seed boot CPU peg: a single-graph
    /// `store.update()` must recount only the graph it writes to, never rescan
    /// every named graph (which on the viewer-3d-demo includes a 2.64M-triple
    /// `…/ifcowl` lift). Asserted via the index's per-graph scan counter.
    #[test]
    fn update_recounts_only_written_graph_not_unrelated_ones() {
        let store = TripleStore::in_memory().unwrap();

        // A "big" graph standing in for a dataset's multi-million-triple ifcOWL
        // lift — a full rebuild would rescan it on every write.
        let big = "http://example.org/big";
        let mut ttl = String::new();
        for i in 0..200 {
            ttl.push_str(&format!(
                "<http://example.org/s{i}> <http://example.org/p> \"{i}\" .\n"
            ));
        }
        store
            .load_str(&ttl, RdfFormat::NTriples, Some(big))
            .unwrap();
        // Two more unrelated named graphs, so a full rebuild scans several graphs.
        store
            .load_str(
                "<http://e/a> <http://e/p> \"1\" .",
                RdfFormat::NTriples,
                Some("http://example.org/other1"),
            )
            .unwrap();
        store
            .load_str(
                "<http://e/b> <http://e/p> \"2\" .",
                RdfFormat::NTriples,
                Some("http://example.org/other2"),
            )
            .unwrap();

        assert_eq!(store.graph_count_cached(Some(big)), Some(200));
        assert!(store.named_graphs().unwrap().len() >= 3);

        // Snapshot the scan counter, then run an UNRELATED targeted write into a
        // brand-new small graph.
        let before = store.graph_index.scan_count();
        store
            .update(
                "INSERT DATA { GRAPH <http://example.org/small> { <http://e/x> <http://e/p> \"v\" } }",
            )
            .unwrap();
        let targeted_scans = store.graph_index.scan_count() - before;

        // Exactly one graph (the small target) was recounted — NOT a full rebuild
        // that would have rescanned the big graph and the two others.
        assert_eq!(
            targeted_scans, 1,
            "a single-graph update must recount only its own graph"
        );

        // Counts stay correct: big graph untouched, small graph registered.
        assert_eq!(store.graph_count_cached(Some(big)), Some(200));
        assert_eq!(
            store.graph_count_cached(Some("http://example.org/small")),
            Some(1)
        );

        // Demonstrate the win explicitly: a full rebuild scans every graph.
        let before_rebuild = store.graph_index.scan_count();
        store.rebuild_graph_index();
        let rebuild_scans = store.graph_index.scan_count() - before_rebuild;
        assert!(
            rebuild_scans >= 4 && rebuild_scans > targeted_scans,
            "full rebuild ({rebuild_scans}) scans far more graphs than the targeted recount ({targeted_scans})"
        );
    }

    /// The targeted recount must leave the graph-count index byte-for-byte
    /// identical to an authoritative full rebuild, across the full mix of update
    /// shapes the fast path handles (named/default INSERT & DELETE DATA,
    /// DELETE/INSERT…WHERE, and a DELETE that empties a graph).
    #[test]
    fn targeted_update_index_matches_full_rebuild() {
        let store = TripleStore::in_memory().unwrap();
        let stmts = [
            "INSERT DATA { GRAPH <http://ex/a> { <http://ex/s> <http://ex/p> \"1\" } }",
            "INSERT DATA { GRAPH <http://ex/a> { <http://ex/s> <http://ex/p> \"2\" } }",
            "INSERT DATA { GRAPH <http://ex/b> { <http://ex/s> <http://ex/p> \"1\" } }",
            "INSERT DATA { <http://ex/s> <http://ex/p> \"default\" }",
            // Empties graph b (the graph stays registered in oxigraph at count 0).
            "DELETE DATA { GRAPH <http://ex/b> { <http://ex/s> <http://ex/p> \"1\" } }",
            // DELETE/INSERT…WHERE rewriting one triple in graph a.
            "DELETE { GRAPH <http://ex/a> { <http://ex/s> <http://ex/p> \"1\" } } \
             INSERT { GRAPH <http://ex/a> { <http://ex/s> <http://ex/p> \"3\" } } \
             WHERE  { GRAPH <http://ex/a> { <http://ex/s> <http://ex/p> \"1\" } }",
            // DELETE DATA on a graph that never existed → must NOT add a ghost entry.
            "DELETE DATA { GRAPH <http://ex/never> { <http://ex/s> <http://ex/p> \"x\" } }",
        ];
        for s in stmts {
            store.update(s).unwrap();
        }

        let mut targeted = store.graph_counts();
        targeted.sort();

        store.rebuild_graph_index();
        let mut rebuilt = store.graph_counts();
        rebuilt.sort();

        assert_eq!(
            targeted, rebuilt,
            "targeted recount must match a full rebuild exactly"
        );
        // Sanity: the never-existed graph is absent in both.
        assert_eq!(store.graph_count_cached(Some("http://ex/never")), None);
    }

    /// Static target analysis: only statically-known write graphs yield a
    /// targeted recount; anything unbounded falls back to a full rebuild (`None`).
    #[test]
    fn static_update_targets_bounds_known_graphs() {
        let t = TripleStore::static_update_targets;

        // INSERT/DELETE DATA into a named graph ⇒ just that graph.
        assert_eq!(
            t("INSERT DATA { GRAPH <http://ex/g> { <http://ex/s> <http://ex/p> <http://ex/o> } }"),
            Some(vec![Some("http://ex/g".to_string())])
        );
        // INSERT DATA into the default graph ⇒ the default-graph target (None).
        assert_eq!(
            t("INSERT DATA { <http://ex/s> <http://ex/p> <http://ex/o> }"),
            Some(vec![None])
        );
        // Several distinct graphs ⇒ all of them, deduped (many quads, one graph each).
        let many = t("INSERT DATA { \
                GRAPH <http://ex/a> { <http://ex/s> <http://ex/p> <http://ex/o> } \
                GRAPH <http://ex/b> { <http://ex/s> <http://ex/p> <http://ex/o> } \
                GRAPH <http://ex/a> { <http://ex/s> <http://ex/p> <http://ex/o2> } }")
        .unwrap();
        assert_eq!(many.len(), 2);
        assert!(many.contains(&Some("http://ex/a".to_string())));
        assert!(many.contains(&Some("http://ex/b".to_string())));
        // DELETE/INSERT…WHERE with literal graph templates ⇒ those graphs.
        assert_eq!(
            t(
                "DELETE { GRAPH <http://ex/g> { <http://ex/s> <http://ex/p> ?o } } \
               INSERT { GRAPH <http://ex/g> { <http://ex/s> <http://ex/p> \"x\" } } \
               WHERE  { GRAPH <http://ex/g> { OPTIONAL { <http://ex/s> <http://ex/p> ?o } } }"
            ),
            Some(vec![Some("http://ex/g".to_string())])
        );

        // Unbounded / whole-graph effects ⇒ full rebuild (None).
        assert_eq!(
            t("DELETE { GRAPH ?g { ?s ?p ?o } } WHERE { GRAPH ?g { ?s ?p ?o } }"),
            None,
            "variable graph target can't be bounded"
        );
        assert_eq!(t("DROP GRAPH <http://ex/g>"), None);
        assert_eq!(t("CLEAR ALL"), None);
        assert_eq!(t("CREATE GRAPH <http://ex/g>"), None);
        assert_eq!(
            t("this is not a valid update"),
            None,
            "parse miss → rebuild"
        );
    }
}

#[cfg(test)]
mod fast_count_scope_tests {
    use super::*;
    use oxigraph::sparql::QueryResults;

    fn count(store: &TripleStore, q: &str) -> Option<i64> {
        match store.query(q).ok()? {
            QueryResults::Solutions(mut s) => s.next()?.ok()?.get(0).and_then(|t| match t {
                Term::Literal(l) => l.value().parse().ok(),
                _ => None,
            }),
            _ => None,
        }
    }

    /// `COUNT(*)` over `GRAPH <g>` / `GRAPH ?g` is answered from the count
    /// index — exact, and without a scan — and respects `FROM NAMED`.
    #[test]
    fn graph_scoped_counts_come_from_the_index() {
        let store = TripleStore::in_memory().unwrap();
        store
            .update("INSERT DATA { GRAPH <urn:a> { <urn:s> <urn:p> 1, 2, 3 } GRAPH <urn:b> { <urn:s> <urn:p> 4 } <urn:d> <urn:p> 5 }")
            .unwrap();
        let scans = store.graph_index.scan_count();
        assert_eq!(
            count(
                &store,
                "SELECT (COUNT(*) AS ?c) WHERE { GRAPH <urn:a> { ?s ?p ?o } }"
            ),
            Some(3)
        );
        assert_eq!(
            count(
                &store,
                "SELECT (COUNT(*) AS ?c) WHERE { GRAPH <urn:b> { ?s ?p ?o } }"
            ),
            Some(1)
        );
        assert_eq!(
            count(
                &store,
                "SELECT (COUNT(*) AS ?c) WHERE { GRAPH ?g { ?s ?p ?o } }"
            ),
            Some(4)
        );
        assert_eq!(
            count(
                &store,
                "SELECT (COUNT(*) AS ?c) FROM NAMED <urn:b> WHERE { GRAPH ?g { ?s ?p ?o } }"
            ),
            Some(1)
        );
        assert_eq!(
            count(
                &store,
                "SELECT (COUNT(*) AS ?c) FROM NAMED <urn:b> WHERE { GRAPH <urn:a> { ?s ?p ?o } }"
            ),
            Some(0)
        );
        assert_eq!(
            count(
                &store,
                "SELECT (COUNT(*) AS ?c) WHERE { GRAPH <urn:none> { ?s ?p ?o } }"
            ),
            Some(0)
        );
        assert_eq!(
            store.graph_index.scan_count(),
            scans,
            "no graph scan for any of them"
        );
        // A count that binds the graph variable inside the triple is not a full scan.
        assert_eq!(
            count(
                &store,
                "SELECT (COUNT(*) AS ?c) WHERE { GRAPH ?g { ?g ?p ?o } }"
            ),
            Some(0)
        );
    }
}

#[cfg(test)]
mod graph_index_increment_tests {
    use super::*;

    /// Two loads into one graph — with duplicates inside a batch and quads
    /// already stored — leave the index equal to a real recount, without a
    /// graph scan per load.
    #[test]
    fn graph_targeted_loads_bump_the_index_exactly() {
        let store = TripleStore::in_memory().unwrap();
        let g = "urn:inc";
        store
            .load_str(
                "<urn:a> <urn:p> 1 . <urn:b> <urn:p> 2 . <urn:a> <urn:p> 1 .",
                RdfFormat::Turtle,
                Some(g),
            )
            .unwrap();
        assert_eq!(store.graph_count_cached(Some(g)), Some(2));
        let scans_before = store.graph_index.scan_count();
        store
            .load_str(
                "<urn:b> <urn:p> 2 . <urn:c> <urn:p> 3 .",
                RdfFormat::Turtle,
                Some(g),
            )
            .unwrap();
        assert_eq!(
            store.graph_count_cached(Some(g)),
            Some(3),
            "one new quad, one duplicate"
        );
        assert_eq!(
            store.graph_index.scan_count(),
            scans_before,
            "no graph scan for a known graph"
        );
        assert_eq!(
            store.count_graph(Some(g)).unwrap(),
            3,
            "the index agrees with a real count"
        );
    }
}

#[cfg(test)]
mod scoped_tests {
    use super::TripleStore;
    use oxigraph::io::RdfFormat;
    use oxigraph::sparql::QueryResults;

    fn ask(store: &TripleStore, q: &str) -> bool {
        matches!(store.query(q), Ok(QueryResults::Boolean(true)))
    }

    /// The rules of the reasoners are `INSERT … WHERE` updates whose WHERE
    /// reads the default graph; scoped, that default graph is the union of the
    /// given graphs and nothing else.
    #[test]
    fn scoped_update_reads_only_the_scope() {
        let store = TripleStore::in_memory().unwrap();
        store
            .load_str(
                "<urn:a> <urn:p> <urn:b> .",
                RdfFormat::Turtle,
                Some("urn:g1"),
            )
            .unwrap();
        store
            .load_str(
                "<urn:c> <urn:p> <urn:d> .",
                RdfFormat::Turtle,
                Some("urn:g2"),
            )
            .unwrap();
        store
            .load_str("<urn:e> <urn:p> <urn:f> .", RdfFormat::Turtle, None)
            .unwrap();
        let rule = "INSERT { GRAPH <urn:t> { ?s <urn:q> ?o } } WHERE { ?s <urn:p> ?o }";
        store.update_scoped(rule, &["urn:g1".to_string()]).unwrap();
        assert!(ask(
            &store,
            "ASK { GRAPH <urn:t> { <urn:a> <urn:q> <urn:b> } }"
        ));
        assert!(
            !ask(&store, "ASK { GRAPH <urn:t> { <urn:c> <urn:q> <urn:d> } }"),
            "a graph outside the scope must not be read"
        );
        assert!(
            !ask(&store, "ASK { GRAPH <urn:t> { <urn:e> <urn:q> <urn:f> } }"),
            "the unnamed default graph is outside an explicit scope"
        );
        // Unscoped: the historical behaviour — the default graph only.
        store.update(rule).unwrap();
        assert!(ask(
            &store,
            "ASK { GRAPH <urn:t> { <urn:e> <urn:q> <urn:f> } }"
        ));
        assert!(!ask(
            &store,
            "ASK { GRAPH <urn:t> { <urn:c> <urn:q> <urn:d> } }"
        ));
    }

    #[test]
    fn scoped_query_sees_the_scope_as_its_default_graph() {
        let store = TripleStore::in_memory().unwrap();
        store
            .load_str(
                "<urn:a> <urn:p> <urn:b> .",
                RdfFormat::Turtle,
                Some("urn:g1"),
            )
            .unwrap();
        store
            .load_str(
                "<urn:c> <urn:p> <urn:d> .",
                RdfFormat::Turtle,
                Some("urn:g2"),
            )
            .unwrap();
        let q = "ASK { <urn:c> <urn:p> <urn:d> }";
        assert!(matches!(
            store.query_scoped(q, &["urn:g2".to_string()]),
            Ok(QueryResults::Boolean(true))
        ));
        assert!(matches!(
            store.query_scoped(q, &["urn:g1".to_string()]),
            Ok(QueryResults::Boolean(false))
        ));
    }
}
