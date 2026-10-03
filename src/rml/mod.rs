//! RDF Mapping Language (RML) support.
//!
//! Parses RML mapping documents (stored as RDF/Turtle) and executes them
//! against CSV, JSON, or XML source data to produce RDF triples.
//!
//! # Supported features
//! - Three vocabularies, mixed freely: R2RML (`rr:`), legacy RML
//!   (`http://semweb.mmlab.be/ns/rml#`) and RML-Core / RML-IO
//!   (`http://w3id.org/rml/`) — see [`vocab`]
//! - Relational logical sources (`rr:tableName` / `rml:query`) over a
//!   registered datasource, streamed in batches — see [`sql`]
//! - File logical sources: CSV (with CSVW dialects), JSON and JSON Lines
//!   (RFC 9535 JSONPath), XML (XPath 1.0 with namespaces), compressed (gzip,
//!   zip, tar.gz, tar.xz) and in any WHATWG encoding — see [`sources`]
//! - Multi-valued references: a term per value, templates as the cartesian
//!   product of their references' values
//! - `rr:TriplesMap` with subject/predicate/object maps
//! - `rr:template`, `rml:reference` / `rr:column`, `rr:constant` term maps
//! - `rr:class` assertions on subjects
//! - `rr:termType`: IRI, BlankNode, Literal, with R2RML's defaults (§7.4),
//!   and RML-Core's `rml:URI`, `rml:UnsafeIRI`, `rml:UnsafeURI`; a blank-node
//!   term map with no expression
//! - `rr:datatype` and `rr:language` for literals, and RML-Core's
//!   `rml:datatypeMap` / `rml:languageMap`
//! - `rr:graphMap` / `rr:graph` on subject and predicate-object maps, with
//!   R2RML's union semantics and `rr:defaultGraph`
//! - Joins on every source, with RML-Core's `rml:childMap` / `rml:parentMap`
//! - A base IRI for relative IRIs: the run's, or a triples map's `rml:baseIRI`
//!
//! Which term-generation rules apply depends on [`model::Semantics`]: a frozen
//! mapping version written before this engine followed R2RML there keeps the
//! rules it was written against.

pub mod checks;
pub mod executor;
pub mod iri;
pub mod model;
pub mod parser;
pub mod sample;
pub mod sources;
pub mod sql;
pub mod sqlident;
pub mod terms;
pub mod vocab;
pub mod xsd;

pub use executor::execute_with;
pub use parser::{parse_from_store_as, parse_rml};
pub use sql::execute_relational;
