//! Logical sources for RML: a file's bytes turned into logical iterations.
//!
//! RML-IO describes how a source is read — its encoding, its compression,
//! its NULL values, a CSV dialect — and the RML-IO registry how each
//! reference formulation iterates and evaluates references:
//!
//! * **CSV** ([`csv_source`]): one iteration per row; a reference is a
//!   column name and has one value. A `csvw:Table` source may set the
//!   dialect (delimiter, quote, header, comment prefix, …).
//! * **JSONPath** ([`json_source`]): the iterator is an RFC 9535 JSONPath
//!   query, each node it selects an iteration, and a reference a JSONPath
//!   query run on that node — which may select several values. JSON Lines
//!   files are a sequence of documents.
//! * **XPath** ([`xml_source`]): the iterator and references are XPath 1.0
//!   expressions, with the namespaces the reference formulation declares.
//!
//! [`bytes`] undoes the compression and the encoding first.

pub mod bytes;
pub mod csv_source;
pub mod json_source;
pub mod xml_source;

use crate::rml::model::{LogicalSource, ReferenceFormulation};
use crate::rml::terms::Iteration;

/// A file source's logical iterations, with the source's column names when
/// the format has a fixed set (a CSV header).
pub struct Loaded {
    pub columns: Option<Vec<String>>,
    pub iterations: Vec<Iteration>,
}

/// How a file source is read, beyond its logical source.
#[derive(Debug, Clone, Copy)]
pub struct ReadAs {
    /// RML-Core / RML-IO rules for JSON (natural datatypes, an array or
    /// object as a value is an error) rather than the legacy vocabulary's.
    pub rml_core: bool,
    /// What an empty value is. RML-IO: nothing in CSV or XML is NULL unless
    /// `rml:null` says so, and in JSON only `null` is — so an empty XML
    /// element reads as `""` and a JSON `null` as no value. A legacy mapping
    /// version reads an empty XML element and a JSON `null` as this engine
    /// always did (no value, and `""`), and its term maps then generate
    /// nothing from an empty value.
    pub empty_is_value: bool,
}

/// Read the logical iterations of `source` from `data`, evaluating
/// `references` — every expression the triples map reads — on each.
pub fn load(
    data: &[u8],
    path: &str,
    source: &LogicalSource,
    references: &[String],
    read_as: ReadAs,
) -> Result<Loaded, String> {
    let text = bytes::decode(data, &source.access, path)?;
    match &source.reference_formulation {
        ReferenceFormulation::Csv => {
            let (columns, iterations) = csv_source::load(&text, &source.access.dialect)?;
            Ok(Loaded {
                columns: Some(columns),
                iterations,
            })
        }
        ReferenceFormulation::JsonPath => Ok(Loaded {
            columns: None,
            iterations: json_source::load(
                &text,
                source.iterator.as_deref(),
                references,
                source.access.json_lines,
                read_as,
            )?,
        }),
        ReferenceFormulation::XPath => Ok(Loaded {
            columns: None,
            iterations: xml_source::load(
                &text,
                source.iterator.as_deref(),
                references,
                &source.access.namespaces,
                read_as,
            )?,
        }),
        // A relational source does not read a file: its rows come from a
        // connection, through `crate::rml::sql`.
        ReferenceFormulation::Sql => Err(
            "a relational logical source is executed through POST /api/sources/{id}/runs, not \
             from an uploaded file"
                .to_string(),
        ),
        ReferenceFormulation::Other(iri) => {
            Err(format!("Unsupported reference formulation: {iri}"))
        }
    }
}
