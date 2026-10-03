//! CSV source for RML: parses CSV text into rows with column-name keys.

use super::{Row, RowIter};
use std::collections::HashMap;

/// The header and the rows. Every cell is a value, an empty one included:
/// CSV has no NULL of its own (RML-IO), so only `rml:null` makes one.
pub fn load(source_data: &str) -> Result<(Vec<String>, RowIter), String> {
    let mut rdr = csv::Reader::from_reader(source_data.as_bytes());
    let headers = rdr
        .headers()
        .map_err(|e| format!("CSV header error: {e}"))?
        .clone();
    let headers: Vec<String> = headers.iter().map(|h| h.to_string()).collect();

    let mut rows: Vec<Result<Row, String>> = Vec::new();
    for result in rdr.records() {
        match result {
            Ok(record) => {
                let mut row = HashMap::new();
                for (i, field) in record.iter().enumerate() {
                    if let Some(header) = headers.get(i) {
                        row.insert(header.clone(), field.to_string());
                    }
                }
                rows.push(Ok(row));
            }
            Err(e) => rows.push(Err(format!("CSV record error: {e}"))),
        }
    }

    Ok((headers, Box::new(rows.into_iter())))
}
