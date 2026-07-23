//! Tantivy-backed vocabulary term search engine (feature `vocab-search`).
//!
//! # Architecture
//!
//! Two physical indexes share one schema:
//!
//! * **LOV index** — built once per corpus snapshot from `lov.nq.gz` into
//!   `{data_dir}/vocab_index/lov-v{SCHEMA_VERSION}-{sha8}/` (reopened
//!   instantly on later boots).  Built in a background task at boot; term
//!   search serves platform results (with `lov_index_ready: false` in the
//!   envelope) until it finishes.
//! * **Platform index** — in-RAM, rebuilt from the latest published version
//!   graphs of public registry entries whenever the registry changes (dirty
//!   flag set by model mutations, checked before each search).
//!
//! # Ranking
//!
//! Tantivy provides BM25 candidate retrieval with LOV's field boosts
//! (`localName^12`, primary labels `^3`, secondary text `^1.5`, parent
//! vocabulary text `^1`); the final ordering re-scores the candidate set with
//! the LOV formula — a weighted mean of normalized text similarity and
//! sqrt-dampened, per-result-set-normalized popularity:
//!
//! ```text
//! score = (1.0·bm25/max + 0.3·√(occ/max) + 0.5·√(reuse/max) + 0.4·√(local/max))
//!         / Σ(active weights)
//! ```
//!
//! `occ`/`reuse` are LOD-corpus metrics (term-level where the LOV dump has
//! them, vocabulary-level dampened ×0.25 otherwise); `local` counts how often
//! the term is actually used in this instance's datasets — an OTS extension
//! so vocabularies already adopted on the platform rank first.  Weights of
//! the LOV terms match the original implementation (wHitScore=1.0,
//! wOccScore=0.3, wDatScore=0.5).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use serde::Serialize;
use tantivy::query::{AllQuery, BooleanQuery, BoostQuery, FuzzyTermQuery, Occur, Query, TermQuery};
use tantivy::schema::{
    document::Value, Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions, FAST,
    STORED, STRING, TEXT,
};
use tantivy::tokenizer::NgramTokenizer;
use tantivy::{Index, IndexReader, TantivyDocument, Term as TantivyTerm};
use tracing::warn;

use super::corpus::{ExtractionStats, TermDoc, TermType};

/// Bump to force LOV index rebuilds after schema/extraction changes.
const SCHEMA_VERSION: u32 = 1;

/// Candidates fetched per index before re-scoring (also the facet basis).
const CANDIDATE_CAP: usize = 1_500;

/// LOV scoring weights (wHitScore / wOccScore / wDatScore) + local extension.
const W_TEXT: f64 = 1.0;
const W_OCC: f64 = 0.3;
const W_REUSE: f64 = 0.5;
const W_LOCAL: f64 = 0.4;

/// Damping applied when falling back from term-level to vocab-level metrics.
const VOCAB_METRIC_DAMP: f64 = 0.25;

// ─── Schema ──────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct Fields {
    iri: Field,
    local_name: Field,
    local_ngram: Field,
    prefixed: Field,
    prefixed_lower: Field,
    ttype: Field,
    vocab: Field,
    tag: Field,
    labels: Field,
    secondary: Field,
    vocab_text: Field,
    occ: Field,
    reused: Field,
    vocab_occ: Field,
    vocab_reused: Field,
    /// Precomputed popularity (fast field) for empty-query browsing.
    pop: Field,
    source: Field,
    model_id: Field,
}

fn build_schema() -> (Schema, Fields) {
    let mut b = Schema::builder();
    let ngram_indexing = TextFieldIndexing::default()
        .set_tokenizer("vocab_ngram")
        .set_index_option(IndexRecordOption::WithFreqsAndPositions);
    let fields = Fields {
        iri: b.add_text_field("iri", STRING | STORED),
        local_name: b.add_text_field("localname", TEXT | STORED),
        local_ngram: b.add_text_field(
            "localname_ngram",
            TextOptions::default().set_indexing_options(ngram_indexing),
        ),
        prefixed: b.add_text_field("prefixed", STRING | STORED),
        prefixed_lower: b.add_text_field("prefixed_lower", STRING),
        ttype: b.add_text_field("ttype", STRING | STORED),
        vocab: b.add_text_field("vocab", STRING | STORED),
        tag: b.add_text_field("tag", STRING | STORED),
        labels: b.add_text_field("labels", TEXT | STORED),
        secondary: b.add_text_field("secondary", TEXT | STORED),
        vocab_text: b.add_text_field("vocab_text", TEXT),
        occ: b.add_u64_field("occ", STORED),
        reused: b.add_u64_field("reused", STORED),
        vocab_occ: b.add_u64_field("vocab_occ", STORED),
        vocab_reused: b.add_u64_field("vocab_reused", STORED),
        pop: b.add_u64_field("pop", FAST | STORED),
        source: b.add_text_field("source", STRING | STORED),
        model_id: b.add_text_field("model_id", STRING | STORED),
    };
    (b.build(), fields)
}

/// Popularity used for empty-query browsing and autocomplete ordering:
/// term-level LOD occurrences, falling back to dampened vocab-level metrics.
fn popularity_of(doc: &TermDoc) -> u64 {
    doc.occurrences
        .max(doc.vocab_occurrences / 4)
        .max(doc.reused_by_datasets * 1_000)
}

fn register_tokenizers(index: &Index) {
    // Infix matching for local names (LOV uses nGram(2,30); 3..8 bounds the
    // index size while still matching e.g. "erso" → "Person").
    if let Ok(tok) = NgramTokenizer::new(3, 8, false) {
        index.tokenizers().register("vocab_ngram", tok);
    }
}

// ─── One physical index ──────────────────────────────────────────────────────

struct VocabIndex {
    reader: IndexReader,
    index: Index,
    fields: Fields,
    doc_count: u64,
}

impl VocabIndex {
    fn build(dir: Option<&Path>, docs: &[TermDoc]) -> tantivy::Result<Self> {
        let (schema, fields) = build_schema();
        let index = match dir {
            Some(d) => {
                std::fs::create_dir_all(d).ok();
                Index::create_in_dir(d, schema)?
            }
            None => Index::create_in_ram(schema),
        };
        register_tokenizers(&index);
        let mut writer = index.writer(64_000_000)?;
        for doc in docs {
            let mut d = TantivyDocument::default();
            d.add_text(fields.iri, &doc.iri);
            d.add_text(fields.local_name, &doc.local_name);
            d.add_text(fields.local_ngram, &doc.local_name.to_lowercase());
            d.add_text(fields.prefixed, &doc.prefixed);
            d.add_text(fields.prefixed_lower, doc.prefixed.to_lowercase());
            d.add_text(fields.ttype, doc.ttype.as_str());
            d.add_text(fields.vocab, &doc.vocab_prefix);
            for tag in &doc.tags {
                d.add_text(fields.tag, tag);
            }
            d.add_text(fields.labels, doc.labels.join("\n"));
            d.add_text(fields.secondary, doc.secondary.join("\n"));
            d.add_text(fields.vocab_text, &doc.vocab_text);
            d.add_u64(fields.occ, doc.occurrences);
            d.add_u64(fields.reused, doc.reused_by_datasets);
            d.add_u64(fields.vocab_occ, doc.vocab_occurrences);
            d.add_u64(fields.vocab_reused, doc.vocab_reused);
            d.add_u64(fields.pop, popularity_of(doc));
            d.add_text(fields.source, doc.source);
            d.add_text(fields.model_id, &doc.model_id);
            writer.add_document(d)?;
        }
        writer.commit()?;
        let reader = index.reader()?;
        reader.reload()?;
        Ok(Self {
            reader,
            index,
            fields,
            doc_count: docs.len() as u64,
        })
    }

    fn open(dir: &Path) -> tantivy::Result<Self> {
        let index = Index::open_in_dir(dir)?;
        register_tokenizers(&index);
        // Resolve fields against the on-disk schema by name so a stale dir
        // with a different schema fails loudly instead of misreading.
        let schema = index.schema();
        let f = |name: &str| schema.get_field(name);
        let fields = Fields {
            iri: f("iri")?,
            local_name: f("localname")?,
            local_ngram: f("localname_ngram")?,
            prefixed: f("prefixed")?,
            prefixed_lower: f("prefixed_lower")?,
            ttype: f("ttype")?,
            vocab: f("vocab")?,
            tag: f("tag")?,
            labels: f("labels")?,
            secondary: f("secondary")?,
            vocab_text: f("vocab_text")?,
            occ: f("occ")?,
            reused: f("reused")?,
            vocab_occ: f("vocab_occ")?,
            vocab_reused: f("vocab_reused")?,
            pop: f("pop")?,
            source: f("source")?,
            model_id: f("model_id")?,
        };
        let reader = index.reader()?;
        let doc_count = reader.searcher().num_docs();
        Ok(Self {
            reader,
            index,
            fields,
            doc_count,
        })
    }

    /// Build the boosted text query for `q` (LOV multi_match equivalent).
    fn text_query(&self, q: &str) -> Box<dyn Query> {
        let f = self.fields;
        let mut should: Vec<(Occur, Box<dyn Query>)> = Vec::new();

        // Exact prefixed-name match dominates ("foaf:Person").
        let q_lower = q.to_lowercase();
        should.push((
            Occur::Should,
            Box::new(BoostQuery::new(
                Box::new(TermQuery::new(
                    TantivyTerm::from_field_text(f.prefixed_lower, &q_lower),
                    IndexRecordOption::Basic,
                )),
                30.0,
            )),
        ));

        // Field-boosted parse of the raw query (BM25 recall).
        let mut parser = tantivy::query::QueryParser::for_index(
            &self.index,
            vec![
                f.local_name,
                f.local_ngram,
                f.labels,
                f.secondary,
                f.vocab_text,
            ],
        );
        parser.set_field_boost(f.local_name, 12.0);
        parser.set_field_boost(f.local_ngram, 4.0);
        parser.set_field_boost(f.labels, 3.0);
        parser.set_field_boost(f.secondary, 1.5);
        parser.set_field_boost(f.vocab_text, 1.0);
        let (parsed, _errors) = parser.parse_query_lenient(q);
        should.push((Occur::Should, parsed));

        Box::new(BooleanQuery::new(should))
    }

    /// Candidate retrieval with filters compiled into the query.
    fn candidates(
        &self,
        q: &str,
        types: &[TermType],
        vocab: Option<&str>,
        tags: &[String],
        cap: usize,
    ) -> Vec<Candidate> {
        use tantivy::collector::TopDocs;

        let f = self.fields;
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        if q.trim().is_empty() {
            clauses.push((Occur::Must, Box::new(AllQuery)));
        } else {
            clauses.push((Occur::Must, self.text_query(q.trim())));
        }
        if !types.is_empty() {
            let type_terms: Vec<(Occur, Box<dyn Query>)> = types
                .iter()
                .map(|t| {
                    (
                        Occur::Should,
                        Box::new(TermQuery::new(
                            TantivyTerm::from_field_text(f.ttype, t.as_str()),
                            IndexRecordOption::Basic,
                        )) as Box<dyn Query>,
                    )
                })
                .collect();
            clauses.push((Occur::Must, Box::new(BooleanQuery::new(type_terms))));
        }
        if let Some(vp) = vocab {
            clauses.push((
                Occur::Must,
                Box::new(TermQuery::new(
                    TantivyTerm::from_field_text(f.vocab, vp),
                    IndexRecordOption::Basic,
                )),
            ));
        }
        for tag in tags {
            clauses.push((
                Occur::Must,
                Box::new(TermQuery::new(
                    TantivyTerm::from_field_text(f.tag, tag),
                    IndexRecordOption::Basic,
                )),
            ));
        }
        let query = BooleanQuery::new(clauses);

        let searcher = self.reader.searcher();
        // Empty query: BM25 is constant under AllQuery, so retrieve the most
        // POPULAR candidates via the fast field instead of an arbitrary slice.
        let top: Vec<(f32, tantivy::DocAddress)> = if q.trim().is_empty() {
            match searcher.search(
                &query,
                &TopDocs::with_limit(cap).order_by_u64_field("pop", tantivy::index::Order::Desc),
            ) {
                Ok(by_pop) => by_pop
                    .into_iter()
                    .map(|(_pop, addr)| (0.0f32, addr))
                    .collect(),
                Err(_) => return Vec::new(),
            }
        } else {
            match searcher.search(&query, &TopDocs::with_limit(cap).order_by_score()) {
                Ok(t) => t,
                Err(_) => return Vec::new(),
            }
        };
        let mut out = Vec::with_capacity(top.len());
        for (bm25, addr) in top {
            let Ok(doc) = searcher.doc::<TantivyDocument>(addr) else {
                continue;
            };
            let text = |field: Field| {
                doc.get_first(field)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            let num = |field: Field| doc.get_first(field).and_then(|v| v.as_u64()).unwrap_or(0);
            out.push(Candidate {
                iri: text(f.iri),
                local_name: text(f.local_name),
                prefixed: text(f.prefixed),
                ttype: text(f.ttype),
                vocab: text(f.vocab),
                labels: text(f.labels),
                secondary: text(f.secondary),
                tags: doc
                    .get_all(f.tag)
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect(),
                occ: num(f.occ),
                reused: num(f.reused),
                vocab_occ: num(f.vocab_occ),
                vocab_reused: num(f.vocab_reused),
                source: text(f.source),
                model_id: text(f.model_id),
                bm25: bm25 as f64,
                score: 0.0,
            });
        }
        out
    }

    /// Nearest labels for "did you mean" (fuzzy Levenshtein ≤ distance).
    fn fuzzy_suggestions(&self, token: &str, limit: usize) -> Vec<(String, f64)> {
        use tantivy::collector::TopDocs;
        let distance = if token.len() <= 4 { 1 } else { 2 };
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        for field in [self.fields.local_name, self.fields.labels] {
            clauses.push((
                Occur::Should,
                Box::new(FuzzyTermQuery::new(
                    TantivyTerm::from_field_text(field, &token.to_lowercase()),
                    distance,
                    true,
                )),
            ));
        }
        let query = BooleanQuery::new(clauses);
        let searcher = self.reader.searcher();
        let Ok(top) = searcher.search(&query, &TopDocs::with_limit(limit * 4).order_by_score())
        else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (score, addr) in top {
            let Ok(doc) = searcher.doc::<TantivyDocument>(addr) else {
                continue;
            };
            if let Some(prefixed) = doc.get_first(self.fields.prefixed).and_then(|v| v.as_str()) {
                out.push((prefixed.to_string(), score as f64));
            }
            if out.len() >= limit {
                break;
            }
        }
        out
    }
}

// ─── Candidates & results ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub iri: String,
    pub local_name: String,
    pub prefixed: String,
    pub ttype: String,
    pub vocab: String,
    pub labels: String,
    pub secondary: String,
    pub tags: Vec<String>,
    pub occ: u64,
    pub reused: u64,
    pub vocab_occ: u64,
    pub vocab_reused: u64,
    pub source: String,
    pub model_id: String,
    #[serde(skip)]
    pub bm25: f64,
    pub score: f64,
}

#[derive(Debug, Default, Serialize)]
pub struct Aggregations {
    pub types: Vec<(String, usize)>,
    pub vocabs: Vec<(String, usize)>,
    pub tags: Vec<(String, usize)>,
}

#[derive(Debug, Serialize)]
pub struct TermSearchOutcome {
    pub total_results: usize,
    pub results: Vec<Candidate>,
    pub aggregations: Aggregations,
    /// False while the LOV index is still building at boot.
    pub lov_index_ready: bool,
}

/// An autocomplete entry (prefix-typeahead over prefixed names/local names).
#[derive(Debug, Clone, Serialize)]
pub struct AutoEntry {
    pub iri: String,
    pub prefixed: String,
    pub local_name: String,
    pub ttype: String,
    pub vocab: String,
    #[serde(skip)]
    popularity: u64,
}

#[derive(Default)]
struct AutoTable {
    /// Sorted by lowercase prefixed name.
    by_prefixed: Vec<(String, usize)>,
    /// Sorted by lowercase local name.
    by_local: Vec<(String, usize)>,
    entries: Vec<AutoEntry>,
}

impl AutoTable {
    fn build(docs: impl Iterator<Item = AutoEntry>) -> Self {
        let entries: Vec<AutoEntry> = docs.collect();
        let mut by_prefixed: Vec<(String, usize)> = entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.prefixed.to_lowercase(), i))
            .collect();
        by_prefixed.sort();
        let mut by_local: Vec<(String, usize)> = entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.local_name.to_lowercase(), i))
            .collect();
        by_local.sort();
        Self {
            by_prefixed,
            by_local,
            entries,
        }
    }

    fn starts_with<'a>(sorted: &'a [(String, usize)], q: &str) -> impl Iterator<Item = usize> + 'a {
        let start = sorted.partition_point(|(k, _)| k.as_str() < q);
        let q_owned = q.to_string();
        sorted[start..]
            .iter()
            .take_while(move |(k, _)| k.starts_with(&q_owned))
            .map(|(_, i)| *i)
    }

    fn complete(&self, q: &str, types: &[TermType], limit: usize) -> Vec<AutoEntry> {
        let q = q.to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let mut seen = std::collections::HashSet::new();
        let mut hits: Vec<&AutoEntry> = Vec::new();
        for idx in
            Self::starts_with(&self.by_prefixed, &q).chain(Self::starts_with(&self.by_local, &q))
        {
            let e = &self.entries[idx];
            if !types.is_empty() && !types.iter().any(|t| t.as_str() == e.ttype) {
                continue;
            }
            if seen.insert(e.iri.as_str()) {
                hits.push(e);
            }
            if hits.len() >= limit * 8 {
                break;
            }
        }
        hits.sort_by(|a, b| {
            b.popularity
                .cmp(&a.popularity)
                .then_with(|| a.prefixed.cmp(&b.prefixed))
        });
        hits.into_iter().take(limit).cloned().collect()
    }
}

// ─── Engine ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Default)]
pub struct EngineStatus {
    pub lov_index_ready: bool,
    pub lov_terms: u64,
    pub lov_vocabularies: usize,
    pub platform_terms: u64,
    pub platform_vocabularies: usize,
    pub instances_dropped: usize,
    pub corpus_available: bool,
}

pub struct VocabSearchEngine {
    data_dir: PathBuf,
    lov: RwLock<Option<Arc<VocabIndex>>>,
    platform: RwLock<Option<Arc<VocabIndex>>>,
    auto: RwLock<Arc<AutoTable>>,
    /// Term IRI → number of uses across this instance's datasets.
    local_usage: RwLock<Arc<HashMap<String, u64>>>,
    lov_stats: RwLock<ExtractionStats>,
    platform_stats: RwLock<ExtractionStats>,
    corpus_available: AtomicBool,
    platform_dirty: AtomicBool,
}

fn read_lock<T>(l: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    l.read().unwrap_or_else(|e| e.into_inner())
}

fn write_lock<T>(l: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    l.write().unwrap_or_else(|e| e.into_inner())
}

impl VocabSearchEngine {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            lov: RwLock::new(None),
            platform: RwLock::new(None),
            auto: RwLock::new(Arc::new(AutoTable::default())),
            local_usage: RwLock::new(Arc::new(HashMap::new())),
            lov_stats: RwLock::new(ExtractionStats::default()),
            platform_stats: RwLock::new(ExtractionStats::default()),
            corpus_available: AtomicBool::new(false),
            platform_dirty: AtomicBool::new(true),
        }
    }

    pub fn mark_platform_dirty(&self) {
        self.platform_dirty.store(true, Ordering::Relaxed);
    }

    pub fn platform_dirty(&self) -> bool {
        self.platform_dirty.load(Ordering::Relaxed)
    }

    pub fn status(&self) -> EngineStatus {
        let lov_stats = read_lock(&self.lov_stats).clone();
        let platform_stats = read_lock(&self.platform_stats).clone();
        EngineStatus {
            lov_index_ready: read_lock(&self.lov).is_some(),
            lov_terms: lov_stats.terms as u64,
            lov_vocabularies: lov_stats.vocabularies,
            platform_terms: platform_stats.terms as u64,
            platform_vocabularies: platform_stats.vocabularies,
            instances_dropped: lov_stats.instances_dropped + platform_stats.instances_dropped,
            corpus_available: self.corpus_available.load(Ordering::Relaxed),
        }
    }

    pub fn set_corpus_available(&self, available: bool) {
        self.corpus_available.store(available, Ordering::Relaxed);
    }

    /// Directory for the persisted LOV index of the given corpus digest.
    pub fn lov_index_dir(&self, corpus_sha256: &str) -> PathBuf {
        let sha8 = &corpus_sha256[..corpus_sha256.len().min(8)];
        self.data_dir
            .join("vocab_index")
            .join(format!("lov-v{SCHEMA_VERSION}-{sha8}"))
    }

    /// Sidecar written only after a successful build — its presence is the
    /// build-completed marker (tantivy writes meta.json before documents are
    /// committed, so meta.json alone can't distinguish an interrupted build
    /// from a finished one) and it preserves extraction stats across boots.
    pub fn lov_stats_marker(dir: &Path) -> PathBuf {
        dir.join("ots-stats.json")
    }

    /// Install a freshly built LOV index, stamping the completion marker.
    pub fn set_lov_index_from_docs(
        &self,
        dir: &Path,
        docs: &[TermDoc],
        stats: ExtractionStats,
    ) -> tantivy::Result<()> {
        let index = VocabIndex::build(Some(dir), docs)?;
        if let Ok(json) = serde_json::to_vec(&stats) {
            if let Err(e) = std::fs::write(Self::lov_stats_marker(dir), json) {
                warn!("vocab-search: cannot write index stats marker: {e}");
            }
        }
        *write_lock(&self.lov) = Some(Arc::new(index));
        *write_lock(&self.lov_stats) = stats;
        self.rebuild_autocomplete();
        Ok(())
    }

    /// Reopen a previously built LOV index (fast path on warm boots).
    ///
    /// Refuses to open a directory without the completion marker so an
    /// interrupted build is rebuilt instead of silently served empty.
    pub fn reopen_lov_index(&self, dir: &Path) -> anyhow::Result<()> {
        let marker = std::fs::read(Self::lov_stats_marker(dir))
            .map_err(|e| anyhow::anyhow!("no completed-build marker: {e}"))?;
        let stats: ExtractionStats = serde_json::from_slice(&marker)
            .map_err(|e| anyhow::anyhow!("corrupt stats marker: {e}"))?;
        let index = VocabIndex::open(dir)?;
        *write_lock(&self.lov_stats) = ExtractionStats {
            terms: index.doc_count as usize,
            ..stats
        };
        *write_lock(&self.lov) = Some(Arc::new(index));
        self.rebuild_autocomplete();
        Ok(())
    }

    /// Rebuild the in-RAM platform index from the given docs.
    pub fn set_platform_docs(&self, docs: &[TermDoc], stats: ExtractionStats) {
        match VocabIndex::build(None, docs) {
            Ok(index) => {
                *write_lock(&self.platform) = Some(Arc::new(index));
                *write_lock(&self.platform_stats) = stats;
                self.platform_dirty.store(false, Ordering::Relaxed);
                self.rebuild_autocomplete();
            }
            Err(e) => warn!("Platform vocab index build failed: {e}"),
        }
    }

    /// Refresh the local-usage popularity map (term IRI → dataset uses).
    pub fn set_local_usage(&self, usage: HashMap<String, u64>) {
        *write_lock(&self.local_usage) = Arc::new(usage);
    }

    fn rebuild_autocomplete(&self) {
        // Autocomplete is rebuilt from both indexes' stored docs.  Bounded by
        // total doc counts; runs off the query path (build/rebuild callers).
        let mut entries: Vec<AutoEntry> = Vec::new();
        for index in [
            read_lock(&self.lov).clone(),
            read_lock(&self.platform).clone(),
        ]
        .into_iter()
        .flatten()
        {
            let searcher = index.reader.searcher();
            let f = index.fields;
            for segment in searcher.segment_readers() {
                let store = match segment.get_store_reader(1024) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                for doc in store.iter::<TantivyDocument>(segment.alive_bitset()) {
                    let Ok(doc) = doc else { continue };
                    let text = |field: Field| {
                        doc.get_first(field)
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string()
                    };
                    let num =
                        |field: Field| doc.get_first(field).and_then(|v| v.as_u64()).unwrap_or(0);
                    let popularity = num(f.pop);
                    entries.push(AutoEntry {
                        iri: text(f.iri),
                        prefixed: text(f.prefixed),
                        local_name: text(f.local_name),
                        ttype: text(f.ttype),
                        vocab: text(f.vocab),
                        popularity,
                    });
                }
            }
        }
        *write_lock(&self.auto) = Arc::new(AutoTable::build(entries.into_iter()));
    }

    // ── Search ───────────────────────────────────────────────────────────────

    /// LOV-style term search over both indexes.
    #[allow(clippy::too_many_arguments)]
    pub fn search_terms(
        &self,
        q: &str,
        types: &[TermType],
        vocab: Option<&str>,
        tags: &[String],
        source: Option<&str>,
        page: usize,
        page_size: usize,
    ) -> TermSearchOutcome {
        let lov = read_lock(&self.lov).clone();
        let platform = read_lock(&self.platform).clone();
        let local_usage = read_lock(&self.local_usage).clone();

        let mut candidates: Vec<Candidate> = Vec::new();
        // Platform first: on IRI collisions (a vocabulary both installed here
        // and present in the LOV corpus) the platform doc wins — it carries
        // the registry model_id for drill-down.
        let mut seen_iris: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (index, index_source) in [(platform, "platform"), (lov.clone(), "lov")] {
            if let Some(filter) = source {
                if filter != index_source {
                    continue;
                }
            }
            let Some(index) = index else { continue };
            let batch = index.candidates(q, types, vocab, tags, CANDIDATE_CAP);
            for c in batch {
                if seen_iris.insert(c.iri.clone()) {
                    candidates.push(c);
                }
            }
        }
        // Normalize BM25 against the combined maximum so a weak-only index
        // can't inflate its matches to parity with strong ones.
        let max_bm25 = candidates.iter().map(|c| c.bm25).fold(0.0f64, f64::max);
        if max_bm25 > 0.0 {
            for c in &mut candidates {
                c.bm25 /= max_bm25;
            }
        }

        // Per-result-set maxima (LOV normalizes against the current query's
        // matches, not global maxima).
        let eff_occ = |c: &Candidate| {
            if c.occ > 0 {
                c.occ as f64
            } else {
                c.vocab_occ as f64 * VOCAB_METRIC_DAMP
            }
        };
        let eff_reuse = |c: &Candidate| {
            if c.reused > 0 {
                c.reused as f64
            } else {
                c.vocab_reused as f64 * VOCAB_METRIC_DAMP
            }
        };
        let local_of = |c: &Candidate| local_usage.get(&c.iri).copied().unwrap_or(0) as f64;

        let max_occ = candidates.iter().map(&eff_occ).fold(0.0f64, f64::max);
        let max_reuse = candidates.iter().map(&eff_reuse).fold(0.0f64, f64::max);
        let max_local = candidates.iter().map(&local_of).fold(0.0f64, f64::max);
        let text_active = !q.trim().is_empty();

        let mut weight_sum = 0.0;
        if text_active {
            weight_sum += W_TEXT;
        }
        if max_occ > 0.0 {
            weight_sum += W_OCC;
        }
        if max_reuse > 0.0 {
            weight_sum += W_REUSE;
        }
        if max_local > 0.0 {
            weight_sum += W_LOCAL;
        }

        for c in &mut candidates {
            if weight_sum == 0.0 {
                c.score = c.bm25;
                continue;
            }
            let mut s = 0.0;
            if text_active {
                s += W_TEXT * c.bm25;
            }
            if max_occ > 0.0 {
                s += W_OCC * (eff_occ(c) / max_occ).sqrt();
            }
            if max_reuse > 0.0 {
                s += W_REUSE * (eff_reuse(c) / max_reuse).sqrt();
            }
            if max_local > 0.0 {
                s += W_LOCAL * (local_of(c) / max_local).sqrt();
            }
            c.score = s / weight_sum;
        }

        candidates.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.prefixed.cmp(&b.prefixed))
        });

        // Facets over the whole candidate set (LOV aggregations).
        let mut agg = Aggregations::default();
        {
            let mut types_c: HashMap<&str, usize> = HashMap::new();
            let mut vocabs_c: HashMap<&str, usize> = HashMap::new();
            let mut tags_c: HashMap<&str, usize> = HashMap::new();
            for c in &candidates {
                *types_c.entry(c.ttype.as_str()).or_default() += 1;
                *vocabs_c.entry(c.vocab.as_str()).or_default() += 1;
                for t in &c.tags {
                    *tags_c.entry(t.as_str()).or_default() += 1;
                }
            }
            let sort_desc = |m: HashMap<&str, usize>| {
                let mut v: Vec<(String, usize)> =
                    m.into_iter().map(|(k, c)| (k.to_string(), c)).collect();
                v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                v
            };
            agg.types = sort_desc(types_c);
            agg.vocabs = sort_desc(vocabs_c);
            agg.vocabs.truncate(25);
            agg.tags = sort_desc(tags_c);
            agg.tags.truncate(25);
        }

        let total = candidates.len();
        let start = page.saturating_sub(1) * page_size;
        let results = candidates.into_iter().skip(start).take(page_size).collect();
        TermSearchOutcome {
            total_results: total,
            results,
            aggregations: agg,
            lov_index_ready: lov.is_some(),
        }
    }

    /// Prefix typeahead over prefixed names and local names.
    pub fn autocomplete(&self, q: &str, types: &[TermType], limit: usize) -> Vec<AutoEntry> {
        read_lock(&self.auto).complete(q, types, limit)
    }

    /// "Did you mean" suggestions via fuzzy matching.
    pub fn suggest(&self, q: &str, limit: usize) -> Vec<(String, f64)> {
        let token = q.split_whitespace().next().unwrap_or("");
        if token.is_empty() {
            return Vec::new();
        }
        let mut best: HashMap<String, f64> = HashMap::new();
        for index in [
            read_lock(&self.platform).clone(),
            read_lock(&self.lov).clone(),
        ]
        .into_iter()
        .flatten()
        {
            for (text, score) in index.fuzzy_suggestions(token, limit) {
                let entry = best.entry(text).or_insert(score);
                if score > *entry {
                    *entry = score;
                }
            }
        }
        let mut out: Vec<(String, f64)> = best.into_iter().collect();
        out.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        out.truncate(limit);
        out
    }
}

/// Compute the sha256 of a corpus file (hex), for index-dir keying.
pub fn file_sha256(path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(
        iri: &str,
        ttype: TermType,
        vocab: &str,
        labels: &[&str],
        secondary: &[&str],
        occ: u64,
        reused: u64,
    ) -> TermDoc {
        let local = iri.rsplit(['#', '/']).next().unwrap_or(iri).to_string();
        TermDoc {
            iri: iri.to_string(),
            prefixed: format!("{vocab}:{local}"),
            local_name: local,
            ttype,
            vocab_prefix: vocab.to_string(),
            labels: labels.iter().map(|s| s.to_string()).collect(),
            secondary: secondary.iter().map(|s| s.to_string()).collect(),
            vocab_text: format!("{vocab} vocabulary"),
            tags: vec!["Test".to_string()],
            occurrences: occ,
            reused_by_datasets: reused,
            vocab_occurrences: occ,
            vocab_reused: reused,
            source: "lov",
            model_id: String::new(),
        }
    }

    fn engine_with(docs: Vec<TermDoc>) -> VocabSearchEngine {
        let engine = VocabSearchEngine::new(std::env::temp_dir().join("vocab-idx-test"));
        engine.set_platform_docs(
            &docs,
            ExtractionStats {
                vocabularies: 1,
                terms: docs.len(),
                instances_dropped: 0,
            },
        );
        engine
    }

    #[test]
    fn exact_local_name_beats_comment_mention() {
        let engine = engine_with(vec![
            doc(
                "http://xmlns.com/foaf/0.1/Person",
                TermType::Class,
                "foaf",
                &["Person"],
                &["A person."],
                100,
                10,
            ),
            doc(
                "http://example.org/v#Employee",
                TermType::Class,
                "ex",
                &["Employee"],
                &["A person employed by an organization."],
                100,
                10,
            ),
        ]);
        let outcome = engine.search_terms("person", &[], None, &[], None, 1, 10);
        assert_eq!(outcome.total_results, 2);
        assert_eq!(outcome.results[0].local_name, "Person");
    }

    #[test]
    fn popularity_reorders_equal_text_matches() {
        let engine = engine_with(vec![
            doc(
                "http://a.example/ns#name",
                TermType::Property,
                "obscure",
                &["name"],
                &[],
                1,
                1,
            ),
            doc(
                "http://xmlns.com/foaf/0.1/name",
                TermType::Property,
                "foaf",
                &["name"],
                &[],
                1_000_000,
                200,
            ),
        ]);
        let outcome = engine.search_terms("name", &[], None, &[], None, 1, 10);
        assert_eq!(outcome.results[0].vocab, "foaf");
        assert!(outcome.results[0].score > outcome.results[1].score);
    }

    #[test]
    fn type_and_vocab_filters_apply() {
        let engine = engine_with(vec![
            doc(
                "http://xmlns.com/foaf/0.1/Person",
                TermType::Class,
                "foaf",
                &["Person"],
                &[],
                10,
                5,
            ),
            doc(
                "http://xmlns.com/foaf/0.1/name",
                TermType::Property,
                "foaf",
                &["name of a person"],
                &[],
                10,
                5,
            ),
        ]);
        let classes = engine.search_terms("person", &[TermType::Class], None, &[], None, 1, 10);
        assert_eq!(classes.total_results, 1);
        assert_eq!(classes.results[0].ttype, "class");

        let scoped = engine.search_terms("person", &[], Some("nope"), &[], None, 1, 10);
        assert_eq!(scoped.total_results, 0);
    }

    #[test]
    fn empty_query_is_popularity_browse() {
        let engine = engine_with(vec![
            doc(
                "http://a.example/ns#rare",
                TermType::Class,
                "a",
                &["rare"],
                &[],
                1,
                1,
            ),
            doc(
                "http://b.example/ns#famous",
                TermType::Class,
                "b",
                &["famous"],
                &[],
                9_999,
                999,
            ),
        ]);
        let outcome = engine.search_terms("", &[], None, &[], None, 1, 10);
        assert_eq!(outcome.results[0].local_name, "famous");
    }

    #[test]
    fn local_usage_boosts_platform_adopted_terms() {
        let engine = engine_with(vec![
            doc(
                "http://a.example/ns#label",
                TermType::Property,
                "a",
                &["label"],
                &[],
                50,
                5,
            ),
            doc(
                "http://b.example/ns#label",
                TermType::Property,
                "b",
                &["label"],
                &[],
                50,
                5,
            ),
        ]);
        let mut usage = HashMap::new();
        usage.insert("http://b.example/ns#label".to_string(), 5_000u64);
        engine.set_local_usage(usage);
        let outcome = engine.search_terms("label", &[], None, &[], None, 1, 10);
        assert_eq!(
            outcome.results[0].vocab, "b",
            "locally used term ranks first"
        );
    }

    #[test]
    fn autocomplete_prefix_matching() {
        let engine = engine_with(vec![
            doc(
                "http://xmlns.com/foaf/0.1/Person",
                TermType::Class,
                "foaf",
                &["Person"],
                &[],
                100,
                10,
            ),
            doc(
                "http://xmlns.com/foaf/0.1/PersonalProfileDocument",
                TermType::Class,
                "foaf",
                &["PersonalProfileDocument"],
                &[],
                5,
                1,
            ),
        ]);
        let hits = engine.autocomplete("foaf:pe", &[], 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].prefixed, "foaf:Person", "popular first");
        // Local-name completion without the prefix works too.
        let hits = engine.autocomplete("person", &[], 10);
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn suggest_finds_typo_neighbours() {
        let engine = engine_with(vec![doc(
            "http://xmlns.com/foaf/0.1/Person",
            TermType::Class,
            "foaf",
            &["person"],
            &[],
            100,
            10,
        )]);
        let suggestions = engine.suggest("persn", 5);
        assert!(
            suggestions.iter().any(|(s, _)| s == "foaf:Person"),
            "{suggestions:?}"
        );
    }
}
