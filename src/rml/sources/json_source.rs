//! JSON source for RML: iterates over a JSON array (or uses an iterator path).
//!
//! Uses a simple JSONPath-like iterator to select the array to iterate over.
//! For each array element (object), column references access the object's keys.

use std::collections::HashMap;
use super::{Row, RowIter};

pub fn load(source_data: &str, iterator: Option<&str>) -> Result<RowIter, String> {
    let value: serde_json::Value = serde_json::from_str(source_data)
        .map_err(|e| format!("JSON parse error: {e}"))?;

    // Navigate to the iterator path (simple dotted or $ path)
    let items = navigate_path(&value, iterator.unwrap_or("$"))?;

    let array = match items {
        serde_json::Value::Array(arr) => arr,
        other => vec![other],
    };

    let rows: Vec<Result<Row, String>> = array.into_iter().map(|item| {
        flatten_json_object(&item)
    }).collect();

    Ok(Box::new(rows.into_iter()))
}

/// Navigate a simple JSONPath-like expression to a sub-value.
/// Supports: `$`, `$.key`, `$.key.nested`, `$[*]` (returns the array itself).
fn navigate_path(value: &serde_json::Value, path: &str) -> Result<serde_json::Value, String> {
    let path = path.trim_start_matches('$').trim_start_matches('.');
    if path.is_empty() || path == "[*]" {
        return Ok(value.clone());
    }
    // Remove trailing [*] if present
    let path = path.trim_end_matches("[*]").trim_end_matches('.');
    let mut current = value;
    for part in path.split('.') {
        let part = part.trim_matches('[').trim_matches(']');
        match current.get(part) {
            Some(v) => current = v,
            None => return Err(format!("JSON path part '{part}' not found")),
        }
    }
    Ok(current.clone())
}

/// Flatten a JSON object to a string map (shallow — nested objects become JSON strings).
fn flatten_json_object(value: &serde_json::Value) -> Result<Row, String> {
    match value {
        serde_json::Value::Object(map) => {
            let mut row = HashMap::new();
            for (k, v) in map {
                let s = match v {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    serde_json::Value::Null => String::new(),
                    other => other.to_string(),
                };
                row.insert(k.clone(), s);
            }
            Ok(row)
        }
        other => {
            let mut row = HashMap::new();
            row.insert("value".to_string(), other.to_string().trim_matches('"').to_string());
            Ok(row)
        }
    }
}
