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
//! - `rr:termType`: IRI, BlankNode, Literal, with R2RML's defaults (§7.4)
//! - `rr:datatype` and `rr:language` for literals
//! - `rr:graphMap` / `rr:graph` on subject and predicate-object maps, with
//!   R2RML's union semantics and `rr:defaultGraph`
//! - A base IRI for relative IRIs: the run's, or a triples map's `rml:baseIRI`
//!
//! Which term-generation rules apply depends on [`model::Semantics`]: a frozen
//! mapping version written before this engine followed R2RML there keeps the
//! rules it was written against.

pub mod executor;
pub mod iri;
pub mod model;
pub mod parser;
pub mod sample;
pub mod sources;
pub mod sql;
pub mod sqlident;
pub mod terms;

pub use executor::{execute, execute_authorized};
pub use parser::{parse_from_store_as, parse_rml};
pub use sql::execute_relational;
