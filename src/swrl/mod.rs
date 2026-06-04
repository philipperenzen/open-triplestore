//! SWRL (Semantic Web Rule Language) rule engine.
//!
//! Implements the W3C SWRL specification for OWL-based rules. Rules are
//! translated to SPARQL INSERT WHERE queries and executed in a fixed-point
//! loop (same pattern as RDFS/OWL reasoning).
//!
//! # Modules
//!
//! - `parser` — SWRL XML/OWL parser (quick-xml based)
//! - `engine` — Rule evaluation via SPARQL INSERT WHERE translation

pub mod engine;
pub mod parser;

pub use engine::execute_rules;
