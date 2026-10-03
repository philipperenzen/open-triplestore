//! Logical source implementations for RML.

pub mod csv_source;
pub mod json_source;
pub mod xml_source;

use std::collections::HashMap;

/// A single row of source data: column name → string value.
pub type Row = HashMap<String, String>;

/// A boxed iterator over rows.
pub type RowIter = Box<dyn Iterator<Item = Result<Row, String>>>;

/// Load rows from a source string based on the reference formulation, with
/// the source's column names when the format has a fixed set (a CSV header).
///
/// `empty_is_value` decides what an empty value is. RML-IO: nothing in CSV
/// or XML is NULL unless `rml:null` says so, and in JSON only `null` is — so
/// an empty XML element reads as `""` and a JSON `null` as no value. A legacy
/// mapping version reads an empty XML element and a JSON `null` as this
/// engine always did (no value, and `""`), and its term maps then generate
/// nothing from an empty value.
pub fn load_rows(
    source_data: &str,
    formulation: &crate::rml::model::ReferenceFormulation,
    iterator: Option<&str>,
    empty_is_value: bool,
) -> Result<(Option<Vec<String>>, RowIter), String> {
    use crate::rml::model::ReferenceFormulation;
    match formulation {
        ReferenceFormulation::Csv => {
            csv_source::load(source_data).map(|(headers, rows)| (Some(headers), rows))
        }
        ReferenceFormulation::JsonPath => {
            json_source::load(source_data, iterator, empty_is_value).map(|r| (None, r))
        }
        ReferenceFormulation::XPath => {
            xml_source::load(source_data, iterator, empty_is_value).map(|r| (None, r))
        }
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
