//! ShEx (Shape Expressions) validation engine.
//!
//! Implements the W3C Shape Expressions Language specification for RDF
//! validation. ShEx provides a concise, human-readable syntax for describing
//! RDF graph structures (similar to SHACL but with a different philosophy).
//!
//! # Modules
//!
//! - `schema` — AST types for ShEx schemas
//! - `parser` — ShExC compact syntax parser (nom-based)
//! - `validator` — Shape evaluation engine
//! - `report` — Validation report types
#![allow(dead_code)]

pub mod parser;
pub mod report;
pub mod schema;
pub mod validator;

pub use parser::parse_shexc;
pub use validator::validate;
