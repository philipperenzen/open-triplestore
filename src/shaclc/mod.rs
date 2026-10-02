//! SHACL Compact Syntax (SHACL-C).
//!
//! - [`parser`]: the W3C SHACL Compact Syntax (SHACL Community Group report,
//!   <https://w3c.github.io/shacl/shacl-compact-syntax/>), the default.
//! - [`serializer`]: shapes graph → SHACL-C, lossless or an explicit list of
//!   losses.
//! - [`legacy`]: the pre-W3C dialect this server accepted before (`closed`
//!   keyword, `// "msg"`, `or( … )`, a bare IRI meaning `sh:node`).
//!   Deprecated: still parsed for one release when a request passes
//!   `dialect=legacy`, with a warning in the log.

pub mod legacy;
pub mod parser;
pub mod serializer;
pub mod vocab;

pub use parser::{document_to_turtle, parse_document, Document, ParseError};
pub use serializer::{
    serialize, serialize_graph, serialize_graph_lossy, serialize_lossy_with, serialize_with, Loss,
    SerializeError,
};

use std::collections::HashMap;

/// Which SHACL-C grammar a request's body is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Dialect {
    /// The W3C SHACL Compact Syntax (CG report grammar).
    #[default]
    W3c,
    /// The pre-W3C dialect. Deprecated: accepted for one more release, then removed.
    Legacy,
}

/// Parse W3C SHACL-C into Turtle (no initial base IRI).
pub fn parse(input: &str) -> Result<String, String> {
    let doc = parse_document(input, None).map_err(|e| e.to_string())?;
    document_to_turtle(&doc)
}

/// Parse SHACL-C in `dialect` into Turtle. `lenient` exists only for the
/// legacy dialect (drop unrecognised trailing input); the W3C grammar has no
/// lenient mode. `base` is the initial base IRI for the W3C grammar.
pub fn parse_as(
    input: &str,
    dialect: Dialect,
    lenient: bool,
    base: Option<&str>,
) -> Result<String, String> {
    match dialect {
        Dialect::W3c => {
            if lenient {
                return Err(
                    "`lenient` applies only to the deprecated legacy dialect (`dialect=legacy`); \
                     the W3C SHACL Compact Syntax is always parsed strictly"
                        .into(),
                );
            }
            let doc = parse_document(input, base).map_err(|e| e.to_string())?;
            document_to_turtle(&doc)
        }
        Dialect::Legacy => {
            if lenient {
                legacy::parse_lenient(input)
            } else {
                legacy::parse(input)
            }
        }
    }
}

/// Parse a request body as the request's query parameters say:
/// `dialect=w3c|legacy` (default `w3c`), `lenient=true` (legacy only) and
/// `base=<iri>` (the W3C grammar's initial base IRI). Legacy use logs a
/// deprecation warning naming `route`.
pub fn parse_request(
    input: &str,
    query: &HashMap<String, String>,
    route: &str,
) -> Result<String, String> {
    let dialect = match query.get("dialect").map(String::as_str) {
        None | Some("") | Some("w3c") | Some("W3C") => Dialect::W3c,
        Some("legacy") => Dialect::Legacy,
        Some(other) => return Err(format!(
            "unknown SHACL-C dialect `{other}`: use `w3c` (the default) or `legacy` (deprecated)"
        )),
    };
    let lenient = query
        .get("lenient")
        .is_some_and(|v| v == "true" || v == "1");
    if dialect == Dialect::Legacy {
        tracing::warn!(
            route,
            "deprecated: SHACL-C parsed with dialect=legacy (the pre-W3C dialect); it is accepted for \
             one more release only — migrate the document to the W3C SHACL Compact Syntax \
             (docs/shacl.md, \"Migrating from the legacy dialect\")"
        );
    }
    parse_as(
        input,
        dialect,
        lenient,
        query
            .get("base")
            .map(String::as_str)
            .filter(|b| !b.is_empty()),
    )
}
