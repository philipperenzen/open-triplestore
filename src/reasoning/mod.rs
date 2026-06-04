//! Entailment and reasoning engines.
//!
//! Each sub-module implements a different OWL 2 profile or RDFS entailment
//! regime.  All engines follow the same API contract:
//!
//! - Accept a `&TripleStore` and source/target graph IRIs.
//! - Execute SPARQL INSERT rules in a fixed-point loop (mirroring
//!   `shacl::engine::infer`).
//! - Write entailed triples into a **separate named graph** (default
//!   `urn:entailment:{regime}`) so they can be cleared and rebuilt
//!   independently of asserted data.
//! - Return a `ReasoningReport` on success or `ReasoningError` on failure /
//!   inconsistency detection.
//!
//! Feature gates:
//! - `rdfs-entailment` — enables `rdfs`
//! - `owl2-rl`         — enables `owl2_rl`  (implies `rdfs-entailment`)
//! - `owl2-el`         — enables `owl2_el`  (implies `rdfs-entailment`)
//! - `owl2-ql`         — enables `owl2_ql`  (implies `rdfs-entailment`)
//! - `owl2-dl`         — enables `owl2_dl`  (implies `owl2-rl`)

pub mod common;

#[cfg(feature = "rdfs-entailment")]
pub mod rdfs;

#[cfg(feature = "owl2-rl")]
pub mod owl2_rl;

#[cfg(feature = "owl2-el")]
pub mod owl2_el;

#[cfg(feature = "owl2-ql")]
pub mod owl2_ql;

#[cfg(feature = "owl2-dl")]
pub mod owl2_dl;

#[cfg(feature = "owl2-dl")]
pub mod konclude_bridge;

pub use common::ReasoningReport;
