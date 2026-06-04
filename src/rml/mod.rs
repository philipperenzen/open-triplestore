//! RDF Mapping Language (RML) support.
//!
//! Parses RML mapping documents (stored as RDF/Turtle) and executes them
//! against CSV, JSON, or XML source data to produce RDF triples.
//!
//! # Supported features
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

pub use executor::execute;
pub use parser::parse_rml;
