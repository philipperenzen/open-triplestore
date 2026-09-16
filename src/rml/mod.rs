//! RDF Mapping Language (RML) support.
//!
//! Parses RML mapping documents (stored as RDF/Turtle) and executes them
//! against CSV, JSON, or XML source data to produce RDF triples.
//!
//! # Supported features
//! - Relational logical sources (`rr:tableName` / `rml:query`) over a
//!   registered datasource, streamed in batches — see [`sql`]
//! - `rml:LogicalSource` with CSV, JSONPath, and XPath reference formulations
//! - `rr:TriplesMap` with subject/predicate/object maps
//! - `rr:template`, `rml:reference` / `rr:column`, `rr:constant` term maps
//! - `rr:class` assertions on subjects
//! - `rr:termType`: IRI, BlankNode, Literal
//! - `rr:datatype` and `rr:language` for literals
//! - Optional `rr:graphMap` for named graph targeting

pub mod executor;
pub mod model;
pub mod parser;
pub mod sources;
pub mod sql;
pub mod terms;

pub use executor::{execute, execute_authorized};
pub use parser::{parse_from_store, parse_rml};
pub use sql::execute_relational;
