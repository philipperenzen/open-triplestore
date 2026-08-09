//! SPARQL + full-text search via Tantivy.
//!
//! Enabled with the `text-search` Cargo feature.
//!
//! # Magic property
//!
//! ```sparql
//! PREFIX ft: <tag:open-triplestore,2024:ft:>
//! SELECT ?s ?score WHERE {
//!     (?s ?score) ft:search ("machine learning" <http://www.w3.org/2000/01/rdf-schema#label> 10) .
//!     ?s a :Paper .
//! }
//! ```
//!
//! The pattern is detected by [`sparql_fn::preprocess_text_search`] *before* the
//! query reaches the SPARQL engine. It executes a Tantivy search and replaces
//! the whole pattern — tuple included — with a `VALUES` clause holding the
//! scored results. The historical `text:` spelling
//! (`http://oxigraph.org/text#`) is accepted as an alias.
//!
//! # Substring push-down
//!
//! [`sparql_fn::preprocess_substring_pushdown`] answers `CONTAINS` and
//! `STRSTARTS` filters from the index where it can prove the candidate list is
//! a superset of the true answer, leaving the original `FILTER` in place to
//! decide the result. See its docs for the cases it deliberately declines.
//!
//! # Read scoping
//!
//! The index spans every graph in the store, and an expanded pattern can be
//! nothing but a `VALUES` clause — which no `FROM` scoping constrains. Both
//! entry points therefore take a [`index::GraphScope`] and apply the caller's
//! read boundary to the index lookup itself.
//!
//! # Index directory
//!
//! Stored at `{data_dir}/tantivy/` (override with `--text-search-dir`). It is a
//! derived cache: safe to delete, and an index written by an older schema is
//! discarded and rebuilt on open.
//!
//! # Auto-sync
//!
//! The index is built at startup after the boot seed chain. After any SPARQL
//! UPDATE or Graph Store Protocol PUT/POST it is marked dirty, and the next
//! query that can actually use it triggers a full rebuild first. Rebuilds are
//! serialised, so concurrent queries share one rather than racing. For manual
//! control use `POST /api/text-search/reindex`.
//!
//! A rebuild reads the whole store and the searches themselves touch the index
//! files, so both are blocking work: `AppState::apply_text_search` runs them on
//! `spawn_blocking`, and the functions here must never be called straight from
//! an async task.

pub mod index;
pub mod sparql_fn;

#[cfg(feature = "text-search")]
pub use index::TextIndex;
