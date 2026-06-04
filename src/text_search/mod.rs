//! SPARQL + full-text search via Tantivy.
//!
//! Enabled with the `text-search` Cargo feature.
//!
//! # Magic property
//!
//! ```sparql
//! PREFIX text: <http://oxigraph.org/text#>
//! SELECT ?s ?score WHERE {
//!     (?s ?score) text:search ("machine learning" rdfs:label 10) .
//!     ?s a :Paper .
//! }
//! ```
//!
//! The `text:search` triple pattern is detected by [`sparql_fn::preprocess_text_search`]
//! *before* the query reaches the SPARQL engine.  It executes a Tantivy search
//! and injects a `VALUES` clause with the scored results.
//!
//! # Index directory
//!
//! Stored at `{data_dir}/tantivy/`.  Created on first run.
//!
//! # Auto-sync
//!
//! After any SPARQL UPDATE or Graph Store Protocol PUT/POST, the index is
//! automatically marked dirty.  The next SPARQL query that uses `text:search`
//! will trigger a full rebuild before execution.  For manual control use
//! `POST /api/text-search/reindex`.

pub mod index;
pub mod sparql_fn;

#[cfg(feature = "text-search")]
pub use index::TextIndex;
