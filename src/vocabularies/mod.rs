//! Controlled-vocabulary (SKOS / RDFS) management.
//!
//! Upload, version, diff, and dereference reusable vocabularies. See
//! [`registry`] for storage, [`upload`] for ingest, [`version`]/[`diff`] for
//! version handling, and [`routes`]/[`handlers`] for the HTTP surface.

pub mod deref;
pub mod diff;
pub mod handlers;
pub mod models;
pub mod registry;
pub mod routes;
pub mod upload;
pub mod version;
