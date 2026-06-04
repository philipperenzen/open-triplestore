//! Logical source implementations for RML.

pub mod csv_source;
pub mod json_source;
pub mod xml_source;

use std::collections::HashMap;

/// A single row of source data: column name → string value.
pub type Row = HashMap<String, String>;

/// A boxed iterator over rows.
pub type RowIter = Box<dyn Iterator<Item = Result<Row, String>>>;

/// Load rows from a source string based on the reference formulation.
pub fn load_rows(
    source_data: &str,
    formulation: &crate::rml::model::ReferenceFormulation,
    iterator: Option<&str>,
) -> Result<RowIter, String> {
    use crate::rml::model::ReferenceFormulation;
    match formulation {
        ReferenceFormulation::Csv => csv_source::load(source_data),
        ReferenceFormulation::JsonPath => json_source::load(source_data, iterator),
        ReferenceFormulation::XPath => xml_source::load(source_data, iterator),
        ReferenceFormulation::Other(iri) => {
            Err(format!("Unsupported reference formulation: {iri}"))
        }
    }
}
