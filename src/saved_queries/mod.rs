//! Saved, versioned SPARQL queries.
//!
//! A saved query is a reusable SPARQL query owned by a dataset, organisation or
//! group. Each query keeps its own edit history (revisions) and is exposed as a
//! parameterised HTTP API. Whenever a dataset gains a new version, its saved
//! queries are re-run against the snapshot and the outcome (still works / results
//! changed / broken) is recorded in the query's test history. Broken queries can
//! be acknowledged by owners/admins and repaired (optionally with the LLM).
//!
//! Submodules:
//! - [`models`] — types ([`models::SavedQuery`], [`models::ParamSpec`], …).
//! - [`store`] — SQLite persistence (reuses the auth pool).
//! - [`params`] — safe typed `{{name}}` injection.
//! - [`fingerprint`] — order-insensitive result hashing for change detection.

pub mod exec;
pub mod fingerprint;
pub mod handlers;
pub mod llm;
pub mod metadata;
pub mod models;
pub mod notify;
pub mod openapi;
pub mod params;
pub mod routes;
pub mod seed;
pub mod seed_data;
pub mod store;
pub mod testing;
