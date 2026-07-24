//! XML source for RML: iterates over XML elements matching an iterator XPath-like expression.
//!
//! Uses quick-xml to parse and a simple path matching scheme.
//! Each selected element's child text nodes become columns.

use super::{Row, RowIter};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::HashMap;

/// Load XML source data.
///
/// The `iterator` is a simple XPath-like expression such as `/root/item`.
/// Each matching element is returned as a row where child element names
/// are columns and their text content is the value.
pub fn load(source_data: &str, iterator: Option<&str>) -> Result<RowIter, String> {
    let iterator_path = iterator.unwrap_or("/");
    let rows = parse_xml(source_data, iterator_path)?;
    Ok(Box::new(rows.into_iter().map(Ok)))
}

fn parse_xml(source_data: &str, iterator: &str) -> Result<Vec<Row>, String> {
    // Parse the path segments (skip empty leading slash)
    let segments: Vec<&str> = iterator.split('/').filter(|s| !s.is_empty()).collect();

    let mut reader = Reader::from_str(source_data);
    reader.config_mut().trim_text(true);

    let mut rows = Vec::new();
    let mut element_stack: Vec<String> = Vec::new();
    let mut current_row: Option<HashMap<String, String>> = None;
    let mut current_field: Option<String> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.name().0).to_string();
                element_stack.push(name.clone());

                // Check if this element matches the iterator path
                if path_matches(&element_stack, &segments) {
                    current_row = Some(HashMap::new());
                } else if current_row.is_some() {
                    // Inside an iterator element — track field names
                    current_field = Some(name);
                }
            }
            Ok(Event::Text(e)) => {
                if let (Some(ref mut row), Some(ref field)) =
                    (current_row.as_mut(), current_field.as_ref())
                {
                    let text = e
                        .decode()
                        .ok()
                        .and_then(|s| {
                            quick_xml::escape::unescape(&s).ok().map(|u| u.into_owned())
                        })
                        .unwrap_or_default();
                    if !text.is_empty() {
                        row.insert(field.to_string(), text);
                    }
                }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.name().0).to_string();

                if path_matches(&element_stack, &segments) {
                    // Closing the iterator element — emit the row
                    if let Some(row) = current_row.take() {
                        rows.push(row);
                    }
                } else if current_row.is_some() && current_field.as_deref() == Some(name.as_str()) {
                    current_field = None;
                }

                element_stack.pop();
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("XML parse error: {e}")),
            _ => {}
        }
    }

    Ok(rows)
}

/// Check if the current element stack matches the iterator path segments.
fn path_matches(stack: &[String], segments: &[&str]) -> bool {
    if segments.is_empty() {
        return true;
    }
    if stack.len() < segments.len() {
        return false;
    }
    let tail = &stack[stack.len() - segments.len()..];
    tail.iter().zip(segments.iter()).all(|(a, b)| a == b)
}
