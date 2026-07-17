//! # open-triplestore
//!
//! A high-performance RDF triple store with SPARQL 1.1/1.2, GeoSPARQL, OWL 2
//! reasoning, SHACL/ShEx validation, and a Linked Data Platform layer, served
//! over HTTP via [`axum`] on top of an [Oxigraph] storage engine.
//!
//! ## Module map
//!
//! - [`store`] — the RDF storage engine and graph index ([`store::TripleStore`]).
//! - [`server`] — the axum HTTP layer: router, [`server::AppState`], SPARQL,
//!   Graph Store Protocol, browse, and dataset-service endpoints.
//! - [`auth`] — authentication & identity: JWT, API tokens, OAuth/OIDC, SAML,
//!   Argon2id passwords, the SQLite user store, ACLs, and audit logging.
//! - [`sparql`], [`geo`] — SPARQL evaluation helpers and GeoSPARQL functions.
//! - [`reasoning`], [`shacl`], [`shaclc`], [`shex`], [`swrl`] — entailment
//!   (RDFS, OWL 2 RL/EL/QL/DL) and shape/rule validation.
//! - [`imports`], [`rml`], [`data_models`] — bulk ingest, RML mappings, and the
//!   unified model registry (OWL/RDFS ontologies and SKOS vocabularies).
//! - [`catalog`], [`dcat`], [`dataset_versions`], [`commit_log`] — dataset
//!   cataloguing, DCAT metadata, versioning, and the append-only commit log.
//! - [`prefixes`], [`kind_detector`], [`backup`], [`storage`], [`alerting`],
//!   [`ldp`], [`text_search`] — supporting subsystems.
//!
//! Many subsystems are gated behind Cargo features (see `Cargo.toml`): `rdf-12`,
//! `owl2-rl`/`owl2-el`/`owl2-ql`/`owl2-dl`, `text-search`, `ldp`, `shex`, `swrl`,
//! `saml`, `backup-encrypt`, and `alerting`.
//!
//! [`axum`]: https://docs.rs/axum
//! [Oxigraph]: https://github.com/oxigraph/oxigraph

pub mod alerting;
pub mod assets;
pub mod auth;
pub mod backup;
pub mod catalog;
pub mod commit_log;
pub mod data_models;
pub mod dataset_versions;
pub mod dcat;
pub mod docs;
pub mod email;
pub mod geo;
pub mod ifc;
pub mod imports;
pub mod kind_detector;
#[cfg(feature = "ldp")]
pub mod ldp;
pub mod ogcapi;
pub mod prefixes;
pub mod reasoning;
pub mod rml;
pub mod saved_queries;
pub mod seed_bundles;
pub mod server;
pub mod shacl;
pub mod shacl_studio;
pub mod shaclc;
#[cfg(feature = "shex")]
pub mod shex;
pub mod sparql;
pub mod storage;
pub mod store;
#[cfg(feature = "swrl")]
pub mod swrl;
#[cfg(feature = "text-search")]
pub mod text_search;
#[cfg(feature = "geometry3d")]
pub mod tiles3d;
