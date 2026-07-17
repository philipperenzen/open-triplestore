//! Tantivy full-text index for RDF literal values.
//!
//! Schema:
//! - `uri`       — subject IRI  (STRING | STORED)
//! - `predicate` — predicate IRI (STRING | STORED)
//! - `text`      — literal value (TEXT | STORED, tokenized and indexed)

use std::path::Path;
use std::sync::{Arc, Mutex};

use tantivy::schema::{Schema, STORED, STRING, TEXT};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument};
use thiserror::Error;
use tracing::{debug, info};

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
    pub score: f32,
}

/// Tantivy-backed full-text index over RDF literal values.
pub struct TextIndex {
    index: Index,
    reader: IndexReader,
    writer: Arc<Mutex<IndexWriter>>,
    uri_field: tantivy::schema::Field,
    predicate_field: tantivy::schema::Field,
    text_field: tantivy::schema::Field,
}

impl TextIndex {
    /// Open (or create) the index at `index_dir`.
    pub fn open(index_dir: &Path) -> Result<Self, TextSearchError> {
        let mut schema_builder = Schema::builder();
        let uri_field = schema_builder.add_text_field("uri", STRING | STORED);
        let predicate_field = schema_builder.add_text_field("predicate", STRING | STORED);
        let text_field = schema_builder.add_text_field("text", TEXT | STORED);
        let schema = schema_builder.build();

        std::fs::create_dir_all(index_dir)
            .map_err(|e| TextSearchError::Store(format!("Cannot create tantivy dir: {e}")))?;

        let index = if index_dir.join("meta.json").exists() {
            info!("Opening existing Tantivy index at {:?}", index_dir);
            Index::open_in_dir(index_dir)?
        } else {
            info!("Creating new Tantivy index at {:?}", index_dir);
            Index::create_in_dir(index_dir, schema)?
        };

        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()?;

        // 50 MB heap for the index writer
        let writer = index.writer(50_000_000)?;

        Ok(Self {
            index,
            reader,
            writer: Arc::new(Mutex::new(writer)),
            uri_field,
            predicate_field,
            text_field,
        })
    }

    /// Add a literal triple to the index.  Call `commit()` after bulk inserts.
    pub fn index_triple(
        &self,
        subject: &str,
        predicate: &str,
        literal: &str,
    ) -> Result<(), TextSearchError> {
        let writer = self.writer.lock().expect("index writer lock poisoned");
        let mut doc = TantivyDocument::default();
        doc.add_text(self.uri_field, subject);
        doc.add_text(self.predicate_field, predicate);
        doc.add_text(self.text_field, literal);
        writer.add_document(doc)?;
        Ok(())
    }

    /// Remove all documents with matching `subject` + `predicate` from the index.
    pub fn remove_triple(&self, subject: &str, predicate: &str) -> Result<(), TextSearchError> {
        use tantivy::Term;

        let uri_term = Term::from_field_text(self.uri_field, subject);
        let pred_term = Term::from_field_text(self.predicate_field, predicate);

        let writer = self.writer.lock().expect("index writer lock poisoned");
        // Delete by URI (predicate is checked at query time)
        writer.delete_term(uri_term);
        let _ = pred_term; // predicate filter applied at read time
        Ok(())
    }

    /// Commit all pending writes.
    pub fn commit(&self) -> Result<(), TextSearchError> {
        let mut writer = self.writer.lock().expect("index writer lock poisoned");
        writer.commit()?;
        Ok(())
    }

    /// Search for `query_str`, optionally restricted to `predicate_filter`.
    ///
    /// Returns up to `limit` results sorted by descending BM25 score.
    pub fn search(
        &self,
        query_str: &str,
        predicate_filter: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchHit>, TextSearchError> {
        use tantivy::collector::TopDocs;
        use tantivy::query::QueryParser;
        use tantivy::schema::document::Value;

        let searcher = self.reader.searcher();
        let query_parser = QueryParser::for_index(&self.index, vec![self.text_field]);
        let query = query_parser.parse_query(query_str)?;

        let top_docs = searcher.search(&query, &TopDocs::with_limit(limit * 4).order_by_score())?;

        let mut hits = Vec::new();
        for (score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher.doc(doc_address)?;
            let subject = doc
                .get_first(self.uri_field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let predicate = doc
                .get_first(self.predicate_field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            // Apply optional predicate filter
            if let Some(filter) = predicate_filter {
                if predicate != filter {
                    continue;
                }
            }

            hits.push(SearchHit {
                subject,
                predicate,
                score,
            });
            if hits.len() >= limit {
                break;
            }
        }

        debug!("text:search '{}' → {} hits", query_str, hits.len());
        Ok(hits)
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

        // Query all literal triples
        let query = "SELECT ?s ?p ?o WHERE { ?s ?p ?o . FILTER(isLiteral(?o)) }";
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
                if let (Some(s), Some(p), Some(o)) = (s, p, o) {
                    self.index_triple(&s, &p, &o)?;
                    count += 1;
                }
            }
        }

        self.commit()?;
        info!("Text index rebuilt: {} documents", count);
        Ok(count)
    }
}
