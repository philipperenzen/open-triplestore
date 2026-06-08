//! Data-model (ontology/shape package) management.
//!
//! Handles uploading, versioning, diffing, and merging reusable data models,
//! plus minting their version IRIs and dereferencing them over HTTP. See
//! [`registry`] for storage, [`upload`] for ingest, [`diff`]/[`merge`] for
//! version comparison, and [`routes`]/[`handlers`] for the HTTP surface.

pub mod deref;
pub mod diff;
pub mod handlers;
pub mod merge;
pub mod models;
pub mod registry;
pub mod routes;
pub mod seed_vocab;
pub mod upload;
pub mod version_iri;
