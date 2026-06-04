//! **OpenGraph** — the RDF engine layer behind *Open Triplestore*.
//!
//! OpenGraph is a maintained layer built *over* [Oxigraph]: it depends on the
//! upstream `oxigraph`/`oxrdf`/`spargebra` crates and adds the capabilities we
//! need on top, rather than forking the storage and SPARQL evaluator. Today it
//! provides two things:
//!
//! ## 1. Durable blank-node identity (headline feature)
//!
//! Plain RDF blank nodes are *not durable*: every parse may invent fresh labels,
//! so re-importing or reloading the same data renames every anonymous node. That
//! makes SHACL shapes, RDF lists and GeoSPARQL geometries impossible to address
//! reliably across sessions. OpenGraph fixes this as far as the W3C standards
//! allow, in two layers:
//!
//! * [`canonical`] — assigns each blank node a **deterministic** label derived
//!   from graph structure (RDF Dataset Canonicalization, RDFC-1.0 shape), so the
//!   same logical graph always produces the same labels.
//! * [`skolem`] — optionally replaces blank nodes with **durable Skolem IRIs**
//!   in the `/.well-known/genid/` space (RDF 1.1 §3.5), minted from each node's
//!   canonical hash. These are real IRIs: globally referenceable, query-able,
//!   and stable across re-import, reload and export. [`skolem::deskolemize`]
//!   restores blank nodes for standards-compliant output.
//!
//! ```
//! use opengraph::{canonical, skolem};
//! # use oxrdf::{Quad, Subject, Term, GraphName, NamedNode, BlankNode, Literal};
//! # fn iri(s:&str)->NamedNode{NamedNode::new(s).unwrap()}
//! # let quads = vec![
//! #   Quad::new(Subject::NamedNode(iri("http://ex/a")), iri("http://ex/p"),
//! #             Term::BlankNode(BlankNode::new_unchecked("x")), GraphName::DefaultGraph),
//! #   Quad::new(Subject::BlankNode(BlankNode::new_unchecked("x")), iri("http://ex/v"),
//! #             Term::Literal(Literal::new_simple_literal("1")), GraphName::DefaultGraph),
//! # ];
//! // Stable canonical labels (c14n0, c14n1, …):
//! let canon = canonical::canonicalize(&quads);
//! // Or durable Skolem IRIs under a chosen base:
//! let (skolemized, _map) = skolem::skolemize(&quads, "https://data.example.org");
//! ```
//!
//! ## 2. Query & storage optimisation (planning-stage)
//!
//! | Optimisation | Module | Status |
//! |---|---|---|
//! | Cost-based BGP reordering | [`optimizer`] | usable (query rewriting) |
//! | Hash join | [`hash_join`] | prototype |
//! | RocksDB tuning | [`rocksdb_config`] | prototype |
//! | MVCC read snapshots | [`mvcc`] | prototype |
//!
//! These operate at the query-rewriting and post-processing levels; native join
//! strategies will require forking `spareval`/`sparopt` once upstream exposes
//! pluggable execution.
//!
//! [Oxigraph]: https://crates.io/crates/oxigraph

// Durable blank-node identity (headline feature).
pub mod canonical;
pub mod skolem;

// Query & storage optimisation.
pub mod hash_join;
pub mod mvcc;
pub mod optimizer;
pub mod rocksdb_config;

// Convenience re-exports for the durable blank-node API.
pub use canonical::{
    canonical_hashes, canonicalize, stable_relabel, Canonicalized, CANON_PREFIX, STABLE_PREFIX,
};
pub use skolem::{deskolemize, is_skolem_iri, skolemize, DEFAULT_SKOLEM_BASE, GENID_PATH};

// Re-export the core Oxigraph crates so downstream code can depend on a single
// crate and stay version-aligned with the engine.
pub use oxigraph;
pub use oxrdf;
pub use spargebra;
