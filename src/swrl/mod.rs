//! SWRL (Semantic Web Rule Language) rule engine.
//!
//! Implements the W3C SWRL specification for OWL-based rules. Rules are
//! translated to SPARQL INSERT WHERE queries and executed in a fixed-point
//! loop (same pattern as RDFS/OWL reasoning).
//!
//! # Modules
//!
//! - `parser` — OWL/XML `DLSafeRule` and the ad-hoc text form
//! - `rdf` — the SWRL RDF syntax (`swrl:Imp`), from a document or the store
//! - `functional` — OWL 2 functional-syntax `DLSafeRule`
//! - `swrlapi` — the SWRLAPI human-readable syntax
//! - `ruleml` — the SWRL §4 RuleML XML syntax
//! - `engine` — Rule evaluation via SPARQL INSERT WHERE translation

pub mod engine;
pub mod functional;
mod lexer;
mod names;
pub mod parser;
pub mod rdf;
pub mod ruleml;
pub mod swrlapi;

use std::collections::HashMap;
use std::sync::Arc;

use oxigraph::io::{JsonLdProfileSet, RdfFormat};

use engine::SwrlRule;
pub use engine::{compile_rules, execute_compiled};

/// The rule syntaxes `parse_rules` reads, by their `format` name.
pub const FORMATS: &[&str] = &[
    "text",
    "xml",
    "owlxml",
    "rdf",
    "functional",
    "swrlapi",
    "ruleml",
];

/// Resolves a prefix label to its namespace (the server's prefix registry).
pub type PrefixLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Options some syntaxes take.
#[derive(Default)]
pub struct ParseOptions<'a> {
    /// The RDF serialisation for format `rdf` (default `turtle`): `turtle`,
    /// `ntriples`, `nquads`, `trig`, `rdfxml`, `jsonld`, `n3`, or a media
    /// type.
    pub rdf_format: Option<&'a str>,
    /// Base IRI for relative IRIs in an RDF document.
    pub base_iri: Option<&'a str>,
    /// Prefixes for the SWRLAPI syntax (`""` is the default prefix), ahead
    /// of `prefix_fallback`.
    pub prefixes: HashMap<String, String>,
    /// Consulted for a SWRLAPI prefix `prefixes` does not declare (the
    /// server's prefix registry).
    pub prefix_fallback: Option<PrefixLookup>,
}

/// Parse `text` as rules in `format` (one of [`FORMATS`]).
pub fn parse_rules(
    format: &str,
    text: &str,
    options: &ParseOptions<'_>,
) -> Result<Vec<SwrlRule>, String> {
    match format {
        "text" => parser::parse_swrl_text(text),
        "xml" | "owlxml" => parser::parse_swrl(text),
        "rdf" => {
            let fmt = rdf_format(options.rdf_format.unwrap_or("turtle"))?;
            rdf::parse_swrl_rdf(text, fmt, options.base_iri)
        }
        "functional" => functional::parse_swrl_functional(text),
        "swrlapi" => {
            let mut names = names::Names::default();
            for (prefix, ns) in &options.prefixes {
                names.declare(prefix.trim_end_matches(':'), ns);
            }
            if let Some(fallback) = &options.prefix_fallback {
                names = names.with_fallback(fallback.clone());
            }
            swrlapi::parse_swrlapi(text, &names)
        }
        "ruleml" => ruleml::parse_swrl_ruleml(text),
        other => Err(format!(
            "Unknown SWRL format '{other}': use one of {}",
            FORMATS.join(", ")
        )),
    }
}

/// The RDF serialisation named by `name`.
pub fn rdf_format(name: &str) -> Result<RdfFormat, String> {
    let lower = name.trim().to_ascii_lowercase();
    Ok(match lower.as_str() {
        "turtle" | "ttl" => RdfFormat::Turtle,
        "ntriples" | "n-triples" | "nt" => RdfFormat::NTriples,
        "nquads" | "n-quads" | "nq" => RdfFormat::NQuads,
        "trig" => RdfFormat::TriG,
        "rdfxml" | "rdf/xml" | "rdf+xml" | "xml" | "owl" => RdfFormat::RdfXml,
        "jsonld" | "json-ld" => RdfFormat::JsonLd {
            profile: JsonLdProfileSet::empty(),
        },
        "n3" => RdfFormat::N3,
        other => RdfFormat::from_media_type(other).ok_or_else(|| {
            format!(
                "Unknown rdf_format '{name}': use turtle, ntriples, nquads, trig, rdfxml, \
                 jsonld, n3 or a media type"
            )
        })?,
    })
}
