use dashmap::DashMap;
use opengraph::spargebra::algebra::{AggregateExpression, Expression, GraphPattern};
use opengraph::spargebra::term::{NamedNodePattern, TermPattern, TriplePattern};
use opengraph::spargebra::Query as SpargebraQuery;
use oxigraph::io::{RdfFormat, RdfParser, RdfSerializer};
use oxigraph::model::*;
use oxigraph::sparql::{QueryOptions, QueryResults, QuerySolutionIter, Update};
use oxigraph::store::Store;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::sync::Arc;
use thiserror::Error;
use tracing::{debug, info};

use crate::geo::functions as geo_fns;
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
    Evaluation(#[from] oxigraph::sparql::EvaluationError),
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
}

impl GraphIndex {
    fn new() -> Self {
        Self {
            counts: Arc::new(DashMap::new()),
        }
    }

    /// Rebuild the entire index from the store.
    fn rebuild(&self, store: &Store) {
        // Compute new values first, then swap atomically per-key.
        // Default graph count
        let default_count = store
            .quads_for_pattern(None, None, None, Some(GraphNameRef::DefaultGraph))
            .count();

        let mut fresh: Vec<(Option<String>, usize)> = Vec::new();
        fresh.push((None, default_count));

        for g in store.named_graphs() {
            if let Ok(NamedOrBlankNode::NamedNode(nn)) = g {
                let count = store
                    .quads_for_pattern(None, None, None, Some(GraphNameRef::NamedNode(nn.as_ref())))
                    .count();
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
    fn recount_specific_graphs(&self, store: &Store, graph_iris: &[Option<String>]) {
        for graph_iri in graph_iris {
            match graph_iri {
                None => {
                    let count = store
                        .quads_for_pattern(None, None, None, Some(GraphNameRef::DefaultGraph))
                        .count();
                    self.counts.insert(None, count);
                }
                Some(iri) => {
                    if let Ok(nn) = NamedNode::new(iri.as_str()) {
                        let count = store
                            .quads_for_pattern(
                                None,
                                None,
                                None,
                                Some(GraphNameRef::NamedNode(nn.as_ref())),
                            )
                            .count();
                        self.counts.insert(Some(iri.clone()), count);
                    }
                }
            }
        }
    }
}

/// The core triple store engine wrapping Oxigraph with GeoSPARQL extensions.
#[derive(Clone)]
pub struct TripleStore {
    store: Arc<Store>,
    graph_index: GraphIndex,
    spatial_index: SpatialIndex,
    /// In-memory subject-sharded read accelerator: a decomposable aggregate/ASK is
    /// answered across cores instead of on one. Derived from `store`, rebuilt
    /// lazily after writes, and bounded by a triple-count cap (see
    /// [`ParallelMirror`]).
    parallel_mirror: ParallelMirror,
    /// Memoises small query results (invalidated on every write); a repeated query
    /// is answered without re-evaluation. See [`QueryCache`].
    query_cache: QueryCache,
    /// Blank-node durability policy applied on import. Defaults to
    /// [`BlankNodeMode::Preserve`] (opt into durability via
    /// [`TripleStore::with_blank_node_mode`]).
    blank_node_mode: BlankNodeMode,
}

impl TripleStore {
    /// Open or create a persistent store at the given path.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let store = Store::open(path)?;
        info!("Opened store at {}", path.display());
        let graph_index = GraphIndex::new();
        graph_index.rebuild(&store);
        let spatial_index = SpatialIndex::new();
        spatial_index.rebuild(&store);
        Ok(Self {
            store: Arc::new(store),
            graph_index,
            spatial_index,
            parallel_mirror: ParallelMirror::from_env(),
            query_cache: QueryCache::from_env(),
            blank_node_mode: BlankNodeMode::default(),
        })
    }

    /// Create an in-memory store (useful for testing).
    pub fn in_memory() -> Result<Self, StoreError> {
        let store = Store::new()?;
        let graph_index = GraphIndex::new();
        let spatial_index = SpatialIndex::new();
        Ok(Self {
            store: Arc::new(store),
            graph_index,
            spatial_index,
            parallel_mirror: ParallelMirror::from_env(),
            query_cache: QueryCache::from_env(),
            blank_node_mode: BlankNodeMode::default(),
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

    /// Build query options with all registered custom functions (GeoSPARQL, RDF 1.2, etc.).
    pub(crate) fn query_options(&self) -> QueryOptions {
        // Disable SPARQL federation (`SERVICE`) explicitly. Today oxigraph is built
        // without an HTTP client so `SERVICE`/`LOAD` already error rather than
        // fetch, but pinning it here keeps SERVICE-based SSRF/exfiltration off even
        // if an oxigraph HTTP feature is ever enabled transitively. (SSRF-1)
        let mut opts = QueryOptions::default().without_service_handler();

        // Register all GeoSPARQL functions
        for (iri, handler) in geo_fns::all_functions() {
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

        opts
    }

    /// Execute a SPARQL query (SELECT, CONSTRUCT, ASK, DESCRIBE).
    pub fn query(&self, sparql: &str) -> Result<QueryResults, StoreError> {
        // Result cache: a repeated, *deterministic* query is answered from a small
        // LRU keyed by the (already ACL-scoped) query string and invalidated on
        // every write — so a hit is the exact result the engine would compute.
        if let Some(cached) = self.query_cache.get(sparql) {
            return Ok(cached);
        }
        let results = self.query_uncached(sparql)?;
        Ok(self.query_cache.put(sparql, results))
    }

    /// The evaluation pipeline behind [`Self::query`], without the result cache.
    fn query_uncached(&self, sparql: &str) -> Result<QueryResults, StoreError> {
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
        let opts = self.query_options();
        let results = self.store.query_opt(sparql, opts)?;
        Ok(results)
    }

    /// Recognise `SELECT (COUNT(*) AS ?v) WHERE { ?s ?p ?o }` (optionally with a
    /// single default-graph `FROM <g>`) and answer it from the O(1) per-graph count
    /// index. Returns `None` for anything else, so the normal evaluator runs and
    /// results never change — this only short-circuits one exact, provably-safe
    /// shape (a single full-scan triple pattern over one graph; the count of a
    /// graph equals its triple count, no RDF-merge dedup involved).
    fn try_fast_count(&self, sparql: &str) -> Option<QueryResults> {
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
        let parsed = SpargebraQuery::parse(sparql, None).ok()?;
        let (pattern, dataset) = match &parsed {
            SpargebraQuery::Select {
                pattern, dataset, ..
            } => (pattern, dataset),
            _ => return None,
        };
        let var_name = count_star_var(pattern)?;
        // The single graph the count applies to (the query's default graph).
        let graph: Option<String> = match dataset {
            None => None,
            Some(ds) => match ds.default.as_slice() {
                // `FROM NAMED` with no `FROM` empties the default graph → not us.
                [] if ds.named.is_some() => return None,
                [] => None,
                [g] => Some(g.as_str().to_string()),
                _ => return None, // multiple FROM → RDF-merge dedup, can't sum counts
            },
        };
        let count = self
            .graph_count_cached(graph.as_deref())
            .or_else(|| self.count_graph(graph.as_deref()).ok())?;
        let var = oxigraph::sparql::Variable::new(var_name).ok()?;
        let lit = Literal::new_typed_literal(
            count.to_string(),
            NamedNode::new_unchecked("http://www.w3.org/2001/XMLSchema#integer"),
        );
        let vars: Arc<[oxigraph::sparql::Variable]> = Arc::from(vec![var]);
        let iter =
            QuerySolutionIter::new(vars, std::iter::once(Ok(vec![Some(Term::Literal(lit))])));
        Some(QueryResults::Solutions(iter))
    }

    /// Execute a SPARQL UPDATE operation.
    pub fn update(&self, sparql: &str) -> Result<(), StoreError> {
        // Use char-boundary-safe slicing to avoid panics on multi-byte UTF-8 input.
        let prefix_end = (0..=sparql.len().min(200))
            .rfind(|&i| sparql.is_char_boundary(i))
            .unwrap_or(0);
        debug!("Executing update: {}", &sparql[..prefix_end]);
        let update = Update::parse(sparql, None)?;
        self.store.update_opt(update, self.query_options())?;
        self.graph_index.rebuild(&self.store);
        self.note_write();
        Ok(())
    }

    /// M-5: Execute a SPARQL UPDATE using a surgical graph index update.
    ///
    /// Instead of rebuilding the entire graph count index after every write,
    /// only the graphs listed in `affected_iris` are re-counted.  When
    /// `full_rebuild` is true (e.g. CLEAR ALL / DROP ALL) a full rebuild is
    /// still performed.
    pub fn update_targeted(
        &self,
        sparql: &str,
        affected_iris: &[String],
        full_rebuild: bool,
    ) -> Result<(), StoreError> {
        let prefix_end = (0..=sparql.len().min(200))
            .rfind(|&i| sparql.is_char_boundary(i))
            .unwrap_or(0);
        debug!("Executing targeted update: {}", &sparql[..prefix_end]);
        let update = Update::parse(sparql, None)?;
        self.store.update_opt(update, self.query_options())?;
        if full_rebuild || affected_iris.is_empty() {
            self.graph_index.rebuild(&self.store);
        } else {
            let graphs: Vec<Option<String>> =
                affected_iris.iter().map(|s| Some(s.clone())).collect();
            self.graph_index
                .recount_specific_graphs(&self.store, &graphs);
        }
        self.note_write();
        Ok(())
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
        let parsed: Vec<Result<Update, String>> = statements
            .iter()
            .map(|s| Update::parse(s, None).map_err(|e| e.to_string()))
            .collect();

        let opts = self.query_options();
        let mut results = Vec::with_capacity(statements.len());

        for update in parsed {
            match update {
                Ok(u) => match self.store.update_opt(u, opts.clone()) {
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
            self.store
                .bulk_loader()
                .load_from_reader(Self::parser_for(format, base_iri)?, reader)?;
            info!("Data loaded successfully (streamed)");
            self.graph_index.rebuild(&self.store);
            self.spatial_index.mark_dirty();
            self.note_write();
            return Ok(());
        }

        // Materialise quads (embedded graph names from NQuads/TriG are preserved;
        // triple formats land in the default graph). Parse errors are propagated.
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

        // Apply the durable blank-node policy, then bulk-load.
        let quads = self.apply_blank_node_mode(quads);
        self.store.bulk_loader().load_quads(quads)?;

        info!("Data loaded successfully");
        self.graph_index.rebuild(&self.store);
        self.spatial_index.mark_dirty();
        self.note_write();
        Ok(())
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
        // Clear existing graph first
        let graph_name = match graph_iri {
            Some(iri) => GraphNameRef::NamedNode(
                NamedNodeRef::new(iri)
                    .map_err(|e| StoreError::Parse(format!("Invalid IRI: {}", e)))?,
            ),
            None => GraphNameRef::DefaultGraph,
        };
        self.store.clear_graph(graph_name)?;

        // Load new data (load_reader rebuilds graph index)
        self.load_str(data, format, graph_iri)
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

    /// Graph Store Protocol: DELETE a named graph.
    pub fn graph_store_delete(&self, graph_iri: Option<&str>) -> Result<(), StoreError> {
        let graph_name = match graph_iri {
            Some(iri) => GraphNameRef::NamedNode(
                NamedNodeRef::new(iri)
                    .map_err(|e| StoreError::Parse(format!("Invalid IRI: {}", e)))?,
            ),
            None => GraphNameRef::DefaultGraph,
        };
        self.store.clear_graph(graph_name)?;
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

        let update = Update::parse(&sparql, None)?;
        self.store.update_opt(update, self.query_options())?;

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
            self.store.bulk_loader().load_quads(quads)?;
            self.spatial_index.mark_dirty();
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
        let subj_ref = SubjectRef::BlankNode(subject.as_ref());
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
        let subj: Subject = match subject.strip_prefix("_:") {
            Some(label) => Subject::BlankNode(BlankNode::new_unchecked(label)),
            None => match NamedNode::new(subject) {
                Ok(nn) => Subject::NamedNode(nn),
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
fn count_star_var(pattern: &GraphPattern) -> Option<String> {
    if let GraphPattern::Project { inner, variables } = pattern {
        if variables.len() == 1 && is_count_star_full_scan(inner) {
            return Some(variables[0].as_str().to_string());
        }
    }
    None
}

/// `Extend(Group([], [COUNT(*)]), full-scan BGP)` — the algebra a global
/// `(COUNT(*) AS ?v)` parses to (the Extend aliases the aggregate result).
fn is_count_star_full_scan(p: &GraphPattern) -> bool {
    match p {
        GraphPattern::Extend {
            inner, expression, ..
        } => matches!(expression, Expression::Variable(_)) && is_count_star_full_scan(inner),
        GraphPattern::Group {
            inner,
            variables,
            aggregates,
        } => {
            variables.is_empty()
                && aggregates.len() == 1
                && matches!(
                    &aggregates[0].1,
                    AggregateExpression::CountSolutions { distinct: false }
                )
                && is_full_scan_bgp(inner)
        }
        _ => false,
    }
}

/// A single triple pattern whose subject, predicate and object are three
/// *distinct* variables (`{ ?s ?p ?o }`) — a true full scan.
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
            if let Subject::BlankNode(b) = &quad.subject {
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
    #[ignore = "Oxigraph RocksDB TryFromIntError on macOS arm64 in test context - works in Docker/Linux"]
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
}
