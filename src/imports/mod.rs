//! Bulk multi-file RDF import.
//!
//! A single multipart POST uploads N files at once. All files are parsed in
//! parallel on the blocking thread pool, then their quads are concatenated and
//! loaded into the store with a single `bulk_delete_graphs` + `bulk_insert_quads`
//! call — avoiding the N-round-trip + per-file re-indexing cost of the
//! one-file-per-request path used by the legacy `DataImport` wizard.

pub mod bulk;
pub mod cityjson;
pub mod handlers;
pub mod ifc;
pub mod routes;
