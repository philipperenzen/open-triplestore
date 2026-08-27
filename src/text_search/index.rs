//! Tantivy full-text index for RDF literal values.
//!
//! Schema:
//! - `uri`       — subject IRI  (STRING | STORED)
//! - `predicate` — predicate IRI (STRING | STORED)
//! - `graph`     — named-graph IRI (STRING | STORED) — the read-authorization key
//! - `text`      — literal value (TEXT | STORED, tokenized and indexed)
//! - `text_raw`  — literal value verbatim (STRING, one un-tokenized term)
//!
//! `text` answers relevance queries (`text:search`). `text_raw` exists so a
//! *substring* predicate — SPARQL's `CONTAINS` / `STRSTARTS` — can be answered
//! with a regex over whole literals. Tokenized matching cannot do that job:
//! `CONTAINS("drawbridge", "bridge")` is true, yet the token `drawbridge` does
//! not match the term `bridge`, so a tokenized candidate set is not a superset
//! of the true answer and must never be used to *restrict* a query.

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tantivy::query::{BooleanQuery, Occur, Query, RegexQuery, TermSetQuery};
use tantivy::schema::{Schema, STORED, STRING, TEXT};
use tantivy::tokenizer::MAX_TOKEN_LEN;
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term};
use thiserror::Error;
use tracing::{debug, info, warn};

use crate::store::TripleStore;

/// Errors produced by the text index.
#[derive(Debug, Error)]
pub enum TextSearchError {
    #[error("Tantivy error: {0}")]
    Tantivy(#[from] tantivy::TantivyError),
    #[error("Query parse error: {0}")]
    QueryParse(#[from] tantivy::query::QueryParserError),
    #[error("Store error: {0}")]
    Store(String),
}

/// A single text search result.
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub subject: String,
    pub predicate: String,
    pub graph: String,
    pub score: f32,
}

/// The graphs a caller may read.
///
/// The index spans every graph in the store, so every read has to be filtered:
/// a hit is only returned when its graph is in the caller's read scope. Without
/// this an expansion that produces nothing but a `VALUES` clause — which carries
/// no triple pattern for the endpoint's `FROM` scoping to bite on — would leak
/// subject IRIs out of private graphs.
#[derive(Debug, Clone, Copy)]
pub enum GraphScope<'a> {
    /// No filtering (admin readers, and internal callers that rescope later).
    All,
    /// Only hits whose graph IRI is in this set.
    Only(&'a HashSet<String>),
}

/// An owned [`GraphScope`], for handing a read boundary to a blocking task.
///
/// Searching the index is blocking work that runs on `spawn_blocking`, and a
/// `'static` closure cannot borrow the caller's graph set. The `Arc` means
/// crossing that boundary costs a refcount bump rather than cloning the set
/// once per query.
#[derive(Debug, Clone)]
pub enum GraphScopeOwned {
    /// See [`GraphScope::All`].
    All,
    /// See [`GraphScope::Only`].
    Only(Arc<HashSet<String>>),
}

impl GraphScopeOwned {
    /// Borrow this as the scope the search functions take.
    pub fn as_scope(&self) -> GraphScope<'_> {
        match self {
            GraphScopeOwned::All => GraphScope::All,
            GraphScopeOwned::Only(graphs) => GraphScope::Only(graphs),
        }
    }
}

/// How a substring candidate search should treat case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchCase {
    /// `CONTAINS` / `STRSTARTS` semantics.
    Sensitive,
    /// The literal was wrapped in `LCASE(…)` / `UCASE(…)`, or `REGEX(…, "i")`.
    Insensitive,
}

/// Where the search term must sit inside the literal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchAnchor {
    /// `CONTAINS` — anywhere.
    Anywhere,
    /// `STRSTARTS` — at the start.
    Prefix,
}

/// Candidate subjects for a substring predicate.
///
/// `complete` is the contract that makes push-down safe: it is `true` only when
/// `subjects` is a *superset* of every subject whose literal satisfies the
/// predicate. A caller must not use the set to restrict a query when it is
/// `false` — doing so silently drops correct results.
#[derive(Debug, Clone)]
pub struct SubstringCandidates {
    pub subjects: Vec<String>,
    pub complete: bool,
}

/// Upper bound on candidates collected for a push-down. Beyond this the
/// `VALUES` clause stops paying for itself anyway, and the set is reported
/// incomplete so it is never used to restrict a query.
const MAX_PUSHDOWN_CANDIDATES: usize = 10_000;

/// Tantivy-backed full-text index over RDF literal values.
pub struct TextIndex {
    index: Index,
    reader: IndexReader,
    writer: Arc<Mutex<IndexWriter>>,
    uri_field: tantivy::schema::Field,
    predicate_field: tantivy::schema::Field,
    graph_field: tantivy::schema::Field,
    text_field: tantivy::schema::Field,
    text_raw_field: tantivy::schema::Field,
    /// Cleared when a literal was too long for Tantivy to index as one raw
    /// term (`MAX_TOKEN_LEN`), which makes `text_raw` an incomplete view of the
    /// store and disables substring push-down.
    raw_complete: AtomicBool,
}

/// The schema every index built by this module uses.
fn build_schema() -> Schema {
    let mut b = Schema::builder();
    b.add_text_field("uri", STRING | STORED);
    b.add_text_field("predicate", STRING | STORED);
    b.add_text_field("graph", STRING | STORED);
    b.add_text_field("text", TEXT | STORED);
    b.add_text_field("text_raw", STRING);
    b.build()
}

impl TextIndex {
    /// Open (or create) the index at `index_dir`.
    ///
    /// The index is a derived cache, so an on-disk index written by an older
    /// schema is discarded and rebuilt rather than opened — reusing it would
    /// bind our field handles to the wrong columns.
    pub fn open(index_dir: &Path) -> Result<Self, TextSearchError> {
        let schema = build_schema();

        std::fs::create_dir_all(index_dir)
            .map_err(|e| TextSearchError::Store(format!("Cannot create tantivy dir: {e}")))?;

        let existing = index_dir.join("meta.json").exists();
        let index = if existing {
            match Index::open_in_dir(index_dir) {
                Ok(idx) if idx.schema() == schema => {
                    info!("Opening existing Tantivy index at {:?}", index_dir);
                    idx
                }
                Ok(_) => {
                    info!(
                        "Tantivy index at {:?} was built with an older schema — rebuilding",
                        index_dir
                    );
                    recreate_dir(index_dir)?;
                    Index::create_in_dir(index_dir, schema)?
                }
                Err(e) => {
                    warn!("Tantivy index at {index_dir:?} could not be opened ({e}) — rebuilding");
                    recreate_dir(index_dir)?;
                    Index::create_in_dir(index_dir, schema)?
                }
            }
        } else {
            info!("Creating new Tantivy index at {:?}", index_dir);
            Index::create_in_dir(index_dir, schema)?
        };

        let schema = index.schema();
        let field = |name: &str| -> Result<tantivy::schema::Field, TextSearchError> {
            schema
                .get_field(name)
                .map_err(|e| TextSearchError::Store(format!("missing field `{name}`: {e}")))
        };

        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()?;

        // 50 MB heap for the index writer
        let writer = index.writer(50_000_000)?;

        Ok(Self {
            uri_field: field("uri")?,
            predicate_field: field("predicate")?,
            graph_field: field("graph")?,
            text_field: field("text")?,
            text_raw_field: field("text_raw")?,
            index,
            reader,
            writer: Arc::new(Mutex::new(writer)),
            raw_complete: AtomicBool::new(true),
        })
    }

    /// Add a literal triple to the index.  Call `commit()` after bulk inserts.
    pub fn index_triple(
        &self,
        subject: &str,
        predicate: &str,
        graph: &str,
        literal: &str,
    ) -> Result<(), TextSearchError> {
        let writer = self.writer.lock().expect("index writer lock poisoned");
        let mut doc = TantivyDocument::default();
        doc.add_text(self.uri_field, subject);
        doc.add_text(self.predicate_field, predicate);
        doc.add_text(self.graph_field, graph);
        doc.add_text(self.text_field, literal);
        if literal.len() <= MAX_TOKEN_LEN {
            doc.add_text(self.text_raw_field, literal);
        } else {
            // Tantivy drops over-long tokens, so this literal is invisible to
            // `text_raw`. Substring push-down is no longer a superset of the
            // truth anywhere in this index, so switch it off.
            self.raw_complete.store(false, Ordering::Relaxed);
        }
        writer.add_document(doc)?;
        Ok(())
    }

    /// Remove the documents for `subject` + `predicate` from the index.
    pub fn remove_triple(&self, subject: &str, predicate: &str) -> Result<(), TextSearchError> {
        let query = BooleanQuery::new(vec![
            (
                Occur::Must,
                Box::new(TermSetQuery::new([Term::from_field_text(
                    self.uri_field,
                    subject,
                )])) as Box<dyn Query>,
            ),
            (
                Occur::Must,
                Box::new(TermSetQuery::new([Term::from_field_text(
                    self.predicate_field,
                    predicate,
                )])),
            ),
        ]);

        let writer = self.writer.lock().expect("index writer lock poisoned");
        writer.delete_query(Box::new(query))?;
        Ok(())
    }

    /// Commit all pending writes and make them visible to searches.
    ///
    /// The reader is reloaded synchronously: its reload policy only refreshes
    /// after a delay, and the caller that just wrote is usually the one about
    /// to search (`sync_text_index_if_dirty` reindexes and the same request
    /// then queries), which would otherwise read the pre-commit segments.
    pub fn commit(&self) -> Result<(), TextSearchError> {
        {
            let mut writer = self.writer.lock().expect("index writer lock poisoned");
            writer.commit()?;
        }
        self.reader.reload()?;
        Ok(())
    }

    /// Whether substring push-down may be used against this index.
    pub fn substring_pushdown_available(&self) -> bool {
        self.raw_complete.load(Ordering::Relaxed)
    }

    /// Search for `query_str`, restricted to `scope` and optionally to
    /// `predicate_filter`.
    ///
    /// Returns up to `limit` results sorted by descending BM25 score.
    pub fn search(
        &self,
        query_str: &str,
        predicate_filter: Option<&str>,
        scope: GraphScope<'_>,
        limit: usize,
    ) -> Result<Vec<SearchHit>, TextSearchError> {
        use tantivy::collector::TopDocs;
        use tantivy::query::QueryParser;

        if query_str.trim().is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        let query_parser = QueryParser::for_index(&self.index, vec![self.text_field]);
        let text_query = query_parser.parse_query(query_str)?;

        let Some(query) = self.with_filters(text_query, predicate_filter, scope) else {
            // Empty read scope — nothing is visible.
            return Ok(Vec::new());
        };

        let searcher = self.reader.searcher();
        // Filters are part of the query, so `limit` docs collected are `limit`
        // docs returned: no over-fetch-then-drop, which silently loses hits
        // whenever the discarded surplus would have qualified.
        let top_docs = searcher.search(&query, &TopDocs::with_limit(limit).order_by_score())?;

        let mut hits = Vec::with_capacity(top_docs.len());
        for (score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher.doc(doc_address)?;
            hits.push(SearchHit {
                subject: self.stored_text(&doc, self.uri_field),
                predicate: self.stored_text(&doc, self.predicate_field),
                graph: self.stored_text(&doc, self.graph_field),
                score,
            });
        }

        debug!("text:search '{}' → {} hits", query_str, hits.len());
        Ok(hits)
    }

    /// Collect every subject whose literal *contains* (or starts with) `needle`.
    ///
    /// This is the only search safe to use as a query restriction: it matches
    /// whole literals with a regex over the un-tokenized `text_raw` field, so
    /// the result is a true superset of the SPARQL predicate. The returned set
    /// is flagged incomplete — and must then be ignored — when the index cannot
    /// guarantee that, so a caller can never narrow a query on partial data.
    pub fn search_substring(
        &self,
        needle: &str,
        anchor: MatchAnchor,
        case: MatchCase,
        scope: GraphScope<'_>,
    ) -> Result<SubstringCandidates, TextSearchError> {
        use tantivy::collector::TopDocs;

        let incomplete = SubstringCandidates {
            subjects: Vec::new(),
            complete: false,
        };

        if needle.is_empty() || !self.substring_pushdown_available() {
            return Ok(incomplete);
        }

        let escaped = regex_escape(needle);
        let pattern = match (anchor, case) {
            (MatchAnchor::Anywhere, MatchCase::Sensitive) => format!("(?s).*{escaped}.*"),
            (MatchAnchor::Anywhere, MatchCase::Insensitive) => format!("(?is).*{escaped}.*"),
            (MatchAnchor::Prefix, MatchCase::Sensitive) => format!("(?s){escaped}.*"),
            (MatchAnchor::Prefix, MatchCase::Insensitive) => format!("(?is){escaped}.*"),
        };

        let regex = match RegexQuery::from_pattern(&pattern, self.text_raw_field) {
            Ok(q) => q,
            // An unrepresentable pattern is not an error: it just means we
            // cannot prove a superset, so no push-down happens.
            Err(e) => {
                debug!("substring push-down unavailable for {needle:?}: {e}");
                return Ok(incomplete);
            }
        };

        let Some(query) = self.with_filters(Box::new(regex), None, scope) else {
            // Empty read scope: nothing matches, and that *is* the complete answer.
            return Ok(SubstringCandidates {
                subjects: Vec::new(),
                complete: true,
            });
        };

        let searcher = self.reader.searcher();
        // Collect one past the cap so a full bucket is recognisable as "there
        // may be more" rather than mistaken for an exhaustive set.
        let top_docs = searcher.search(
            &query,
            &TopDocs::with_limit(MAX_PUSHDOWN_CANDIDATES + 1).order_by_score(),
        )?;
        if top_docs.len() > MAX_PUSHDOWN_CANDIDATES {
            debug!("substring push-down for {needle:?} exceeded the candidate cap");
            return Ok(incomplete);
        }

        let mut seen = HashSet::new();
        let mut subjects = Vec::new();
        for (_score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher.doc(doc_address)?;
            let subject = self.stored_text(&doc, self.uri_field);
            if !subject.is_empty() && seen.insert(subject.clone()) {
                subjects.push(subject);
            }
        }

        Ok(SubstringCandidates {
            subjects,
            complete: true,
        })
    }

    /// Combine a text query with the predicate and graph-scope filters.
    ///
    /// Returns `None` when the scope is empty (nothing can match).
    fn with_filters(
        &self,
        text_query: Box<dyn Query>,
        predicate_filter: Option<&str>,
        scope: GraphScope<'_>,
    ) -> Option<Box<dyn Query>> {
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = vec![(Occur::Must, text_query)];

        if let Some(pred) = predicate_filter {
            clauses.push((
                Occur::Must,
                Box::new(TermSetQuery::new([Term::from_field_text(
                    self.predicate_field,
                    pred,
                )])),
            ));
        }

        if let GraphScope::Only(graphs) = scope {
            if graphs.is_empty() {
                return None;
            }
            let terms = graphs
                .iter()
                .map(|g| Term::from_field_text(self.graph_field, g));
            clauses.push((Occur::Must, Box::new(TermSetQuery::new(terms))));
        }

        if clauses.len() == 1 {
            Some(clauses.pop().expect("one clause").1)
        } else {
            Some(Box::new(BooleanQuery::new(clauses)))
        }
    }

    fn stored_text(&self, doc: &TantivyDocument, field: tantivy::schema::Field) -> String {
        use tantivy::schema::document::Value;
        doc.get_first(field)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    }

    /// Refresh the documents of exactly `graphs`: drop everything indexed under
    /// those graph IRIs, re-read their current literal triples from the store,
    /// and commit once. `O(size of the named graphs)` — never a full-store scan.
    ///
    /// This is the write-path companion to [`Self::reindex_from_store`]: a bulk
    /// import (or Graph Store write) knows which graphs it touched, so it can
    /// keep the index warm for the cost of the data it just wrote instead of
    /// marking the whole index dirty and making some later query pay for a
    /// whole-store rebuild (measured at ~10s for 670k documents — inline on the
    /// first `CONTAINS` query after an upload).
    ///
    /// A graph that no longer exists (replace-then-drop, DELETE) simply
    /// contributes zero documents — its old ones are still removed.
    pub fn refresh_graphs(
        &self,
        store: &TripleStore,
        graphs: &[String],
    ) -> Result<usize, TextSearchError> {
        use oxigraph::model::{GraphNameRef, NamedNodeRef, Term};

        if graphs.is_empty() {
            return Ok(0);
        }

        {
            // `graph` is a STRING field: one un-tokenized term per doc, so a
            // term-set delete removes exactly these graphs' documents.
            let writer = self.writer.lock().expect("index writer lock poisoned");
            let query = TermSetQuery::new(
                graphs
                    .iter()
                    .map(|g| tantivy::Term::from_field_text(self.graph_field, g)),
            );
            writer.delete_query(Box::new(query))?;
        }

        let mut count = 0usize;
        for graph in graphs {
            let Ok(g) = NamedNodeRef::new(graph) else {
                continue;
            };
            let quads = store
                .quads_for_graph(GraphNameRef::NamedNode(g))
                .map_err(|e| TextSearchError::Store(e.to_string()))?;
            for q in quads {
                let oxigraph::model::NamedOrBlankNode::NamedNode(s) = &q.subject else {
                    continue;
                };
                let Term::Literal(lit) = &q.object else {
                    continue;
                };
                self.index_triple(s.as_str(), q.predicate.as_str(), graph, lit.value())?;
                count += 1;
            }
        }

        self.commit()?;
        debug!(
            "text index refreshed: {} graphs, {} documents",
            graphs.len(),
            count
        );
        Ok(count)
    }

    /// Rebuild the index from all literal triples in the store.
    pub fn reindex_from_store(&self, store: &TripleStore) -> Result<usize, TextSearchError> {
        info!("Rebuilding text index from store");

        // Clear existing index
        {
            let mut writer = self.writer.lock().expect("index writer lock poisoned");
            writer.delete_all_documents()?;
            writer.commit()?;
        }
        // A fresh build re-establishes the raw-field guarantee until a literal
        // proves otherwise.
        self.raw_complete.store(true, Ordering::Relaxed);

        // Every literal triple, with the graph it lives in. The default graph
        // is covered by the UNION so a store that keeps data outside named
        // graphs still gets indexed.
        let query = "SELECT ?s ?p ?o ?g WHERE { \
             { GRAPH ?g { ?s ?p ?o } } UNION { ?s ?p ?o } \
             FILTER(isLiteral(?o)) }";
        let results = store
            .query(query)
            .map_err(|e| TextSearchError::Store(e.to_string()))?;

        let mut count = 0usize;
        if let oxigraph::sparql::QueryResults::Solutions(solutions) = results {
            for sol in solutions.flatten() {
                let s = sol.get("s").and_then(|v| match v {
                    oxigraph::model::Term::NamedNode(nn) => Some(nn.as_str().to_string()),
                    _ => None,
                });
                let p = sol.get("p").and_then(|v| match v {
                    oxigraph::model::Term::NamedNode(nn) => Some(nn.as_str().to_string()),
                    _ => None,
                });
                let o = sol.get("o").and_then(|v| match v {
                    oxigraph::model::Term::Literal(lit) => Some(lit.value().to_string()),
                    _ => None,
                });
                let g = match sol.get("g") {
                    Some(oxigraph::model::Term::NamedNode(nn)) => nn.as_str().to_string(),
                    // Unbound `?g` is the default graph.
                    _ => DEFAULT_GRAPH_IRI.to_string(),
                };
                if let (Some(s), Some(p), Some(o)) = (s, p, o) {
                    self.index_triple(&s, &p, &g, &o)?;
                    count += 1;
                }
            }
        }

        self.commit()?;
        info!("Text index rebuilt: {} documents", count);
        Ok(count)
    }
}

/// Stand-in graph key for triples held outside any named graph.
pub const DEFAULT_GRAPH_IRI: &str = "urn:open-triplestore:default-graph";

/// Wipe and recreate `dir` so a stale index can be rebuilt in place.
fn recreate_dir(dir: &Path) -> Result<(), TextSearchError> {
    std::fs::remove_dir_all(dir)
        .map_err(|e| TextSearchError::Store(format!("Cannot clear tantivy dir: {e}")))?;
    std::fs::create_dir_all(dir)
        .map_err(|e| TextSearchError::Store(format!("Cannot create tantivy dir: {e}")))
}

/// Escape every regex metacharacter so `needle` matches literally.
fn regex_escape(needle: &str) -> String {
    let mut out = String::with_capacity(needle.len() + 8);
    for ch in needle.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(graphs: &[&str]) -> HashSet<String> {
        graphs.iter().map(|g| (*g).to_string()).collect()
    }

    fn indexed(triples: &[(&str, &str, &str, &str)]) -> (tempfile::TempDir, TextIndex) {
        let dir = tempfile::tempdir().unwrap();
        let idx = TextIndex::open(dir.path()).unwrap();
        for (s, p, g, o) in triples {
            idx.index_triple(s, p, g, o).unwrap();
        }
        idx.commit().unwrap();
        (dir, idx)
    }

    const LABEL: &str = "http://www.w3.org/2000/01/rdf-schema#label";

    #[test]
    fn commit_is_immediately_visible_to_search() {
        // Regression: the reader's reload policy is delayed, so without an
        // explicit reload the search that follows a reindex sees the emptied
        // index and every text query comes back with nothing.
        let (_d, idx) = indexed(&[("http://ex.org/s1", LABEL, "urn:g", "machine learning")]);
        let hits = idx.search("machine", None, GraphScope::All, 10).unwrap();
        assert_eq!(hits.len(), 1, "a committed document must be searchable");
        assert_eq!(hits[0].subject, "http://ex.org/s1");
        assert_eq!(hits[0].graph, "urn:g");
    }

    #[test]
    fn graph_scope_hides_unreadable_graphs() {
        let (_d, idx) = indexed(&[
            ("http://ex.org/pub", LABEL, "urn:public", "shared secret"),
            ("http://ex.org/priv", LABEL, "urn:private", "shared secret"),
        ]);

        let readable = scope(&["urn:public"]);
        let hits = idx
            .search("secret", None, GraphScope::Only(&readable), 10)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].subject, "http://ex.org/pub");

        let none = scope(&[]);
        assert!(idx
            .search("secret", None, GraphScope::Only(&none), 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn predicate_filter_does_not_lose_hits_to_over_fetching() {
        // The old implementation fetched `limit * 4` docs and then dropped the
        // ones with a non-matching predicate, so a popular predicate could
        // crowd the wanted one out of the window entirely.
        let mut triples: Vec<(String, String, String, String)> = (0..200)
            .map(|i| {
                (
                    format!("http://ex.org/noise{i}"),
                    "http://ex.org/comment".to_string(),
                    "urn:g".to_string(),
                    "bridge".to_string(),
                )
            })
            .collect();
        triples.push((
            "http://ex.org/wanted".to_string(),
            LABEL.to_string(),
            "urn:g".to_string(),
            "bridge".to_string(),
        ));
        let borrowed: Vec<(&str, &str, &str, &str)> = triples
            .iter()
            .map(|(s, p, g, o)| (s.as_str(), p.as_str(), g.as_str(), o.as_str()))
            .collect();
        let (_d, idx) = indexed(&borrowed);

        let hits = idx
            .search("bridge", Some(LABEL), GraphScope::All, 5)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].subject, "http://ex.org/wanted");
    }

    #[test]
    fn substring_search_finds_terms_inside_longer_words() {
        // The whole point of `text_raw`: a tokenized index cannot answer this,
        // and a candidate set that misses it is not safe to restrict a query.
        let (_d, idx) = indexed(&[
            ("http://ex.org/a", LABEL, "urn:g", "Drawbridge"),
            ("http://ex.org/b", LABEL, "urn:g", "Waalbrug"),
        ]);

        let got = idx
            .search_substring(
                "bridge",
                MatchAnchor::Anywhere,
                MatchCase::Sensitive,
                GraphScope::All,
            )
            .unwrap();
        assert!(got.complete);
        assert_eq!(got.subjects, vec!["http://ex.org/a".to_string()]);

        // The tokenized index really does miss it — this is the bug the raw
        // field exists to avoid.
        assert!(idx
            .search("bridge", None, GraphScope::All, 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn substring_search_honours_case_and_anchor() {
        let (_d, idx) = indexed(&[
            ("http://ex.org/a", LABEL, "urn:g", "Waalbrug"),
            (
                "http://ex.org/b",
                LABEL,
                "urn:g",
                "de waalbrug bij Nijmegen",
            ),
        ]);

        let sensitive = idx
            .search_substring(
                "Waalbrug",
                MatchAnchor::Anywhere,
                MatchCase::Sensitive,
                GraphScope::All,
            )
            .unwrap();
        assert_eq!(sensitive.subjects, vec!["http://ex.org/a".to_string()]);

        let insensitive = idx
            .search_substring(
                "waalbrug",
                MatchAnchor::Anywhere,
                MatchCase::Insensitive,
                GraphScope::All,
            )
            .unwrap();
        assert_eq!(insensitive.subjects.len(), 2);

        let prefix = idx
            .search_substring(
                "Waalbrug",
                MatchAnchor::Prefix,
                MatchCase::Sensitive,
                GraphScope::All,
            )
            .unwrap();
        assert_eq!(prefix.subjects, vec!["http://ex.org/a".to_string()]);
    }

    #[test]
    fn substring_search_treats_the_needle_as_a_literal() {
        let (_d, idx) = indexed(&[
            ("http://ex.org/a", LABEL, "urn:g", "a.b"),
            ("http://ex.org/b", LABEL, "urn:g", "axb"),
        ]);
        let got = idx
            .search_substring(
                "a.b",
                MatchAnchor::Anywhere,
                MatchCase::Sensitive,
                GraphScope::All,
            )
            .unwrap();
        assert_eq!(
            got.subjects,
            vec!["http://ex.org/a".to_string()],
            "`.` must not act as a wildcard"
        );
    }

    #[test]
    fn oversized_literal_disables_substring_pushdown() {
        let long = "x".repeat(MAX_TOKEN_LEN + 1);
        let (_d, idx) = indexed(&[
            ("http://ex.org/a", LABEL, "urn:g", "bridge"),
            ("http://ex.org/b", LABEL, "urn:g", &long),
        ]);

        assert!(!idx.substring_pushdown_available());
        let got = idx
            .search_substring(
                "bridge",
                MatchAnchor::Anywhere,
                MatchCase::Sensitive,
                GraphScope::All,
            )
            .unwrap();
        assert!(
            !got.complete,
            "a literal Tantivy could not index makes the candidate set unusable"
        );
    }

    #[test]
    fn remove_triple_keeps_other_predicates() {
        let (_d, idx) = indexed(&[
            ("http://ex.org/a", LABEL, "urn:g", "bridge"),
            (
                "http://ex.org/a",
                "http://ex.org/comment",
                "urn:g",
                "bridge",
            ),
        ]);

        idx.remove_triple("http://ex.org/a", LABEL).unwrap();
        idx.commit().unwrap();

        let hits = idx.search("bridge", None, GraphScope::All, 10).unwrap();
        assert_eq!(hits.len(), 1, "only the named predicate should be removed");
        assert_eq!(hits[0].predicate, "http://ex.org/comment");
    }

    #[test]
    fn refresh_graphs_replaces_only_the_named_graphs_documents() {
        use oxigraph::model::{Literal, NamedNode, Quad};

        let dir = tempfile::tempdir().unwrap();
        let idx = TextIndex::open(dir.path()).unwrap();
        // Stale documents for two graphs.
        idx.index_triple("http://ex.org/a", LABEL, "urn:g1", "old bridge")
            .unwrap();
        idx.index_triple("http://ex.org/b", LABEL, "urn:g2", "kept tunnel")
            .unwrap();
        idx.commit().unwrap();

        // The store's current contents for g1 differ from what is indexed.
        let store = TripleStore::in_memory().unwrap();
        store
            .store_quad(Quad::new(
                NamedNode::new("http://ex.org/a2").unwrap(),
                NamedNode::new(LABEL).unwrap(),
                Literal::new_simple_literal("new viaduct"),
                NamedNode::new("urn:g1").unwrap(),
            ))
            .unwrap();

        let n = idx.refresh_graphs(&store, &["urn:g1".to_string()]).unwrap();
        assert_eq!(n, 1);

        // g1's stale document is gone and its live one is searchable; g2's
        // document was not touched.
        assert!(idx
            .search("bridge", None, GraphScope::All, 10)
            .unwrap()
            .is_empty());
        assert_eq!(
            idx.search("viaduct", None, GraphScope::All, 10)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            idx.search("tunnel", None, GraphScope::All, 10)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn reopening_an_older_schema_rebuilds_rather_than_misreads() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut b = Schema::builder();
            b.add_text_field("uri", STRING | STORED);
            b.add_text_field("predicate", STRING | STORED);
            b.add_text_field("text", TEXT | STORED);
            Index::create_in_dir(dir.path(), b.build()).unwrap();
        }

        let idx = TextIndex::open(dir.path()).expect("an old index must be rebuilt, not rejected");
        idx.index_triple("http://ex.org/a", LABEL, "urn:g", "bridge")
            .unwrap();
        idx.commit().unwrap();
        assert_eq!(
            idx.search("bridge", None, GraphScope::All, 10)
                .unwrap()
                .len(),
            1
        );
    }
}
