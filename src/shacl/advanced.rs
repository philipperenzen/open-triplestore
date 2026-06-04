//! SHACL Advanced Features (SHACL-AF) support.
//!
//! This module provides:
//! - SPARQL-based targets (`sh:target` with `sh:select`)
//! - SPARQL-based constraints (`sh:sparql` with `sh:select`)
//! - SPARQL-based rules (`sh:rule` with `sh:construct`)
//! - Triple rules (`sh:rule` with `sh:subject`/`sh:predicate`/`sh:object`)
//!
//! These are integrated into the main engine (engine.rs). This module
//! provides additional utility types and helpers for SHACL-AF features.
