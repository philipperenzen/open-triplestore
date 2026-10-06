//! JSON source for RML (RML-IO registry §JSONPath).
//!
//! The iterator is an RFC 9535 JSONPath query over the document; every node
//! it selects is one logical iteration, and the root of the reference
//! expressions evaluated on it. A reference is a JSONPath query too, and may
//! select several values (`$.tags[*]`). A reference written as a bare name
//! (`Name`, `Country Code`), as the legacy RML vocabulary writes them, is the
//! member of that name. A JSON Lines source is a sequence of documents, each
//! iterated in turn.
//!
//! **Values.** A string is its text; a number, `true` and `false` are their
//! JSON text, typed `xsd:integer`, `xsd:double` or `xsd:boolean` under
//! RML-Core (the registry's natural mapping) and plain under the legacy
//! vocabulary; `null` is no value. A reference that selects an array or an
//! object selected no value the registry can map, which RML-Core treats as an
//! error (select the elements with `[*]`); the legacy vocabulary read an
//! array's elements and an object's JSON text, which it keeps.

use std::collections::HashMap;

use ots_plugin_api::sources::ValueKind;
use serde_json::Value;
use serde_json_path::JsonPath;

use super::ReadAs;
use crate::rml::terms::{Iteration, Val};

/// A reference expression as the JSONPath query it means.
fn as_jsonpath(expr: &str) -> String {
    let e = expr.trim();
    if e.starts_with('$') {
        return e.to_string();
    }
    if let Some(rest) = e.strip_prefix('@') {
        return format!("${rest}");
    }
    let plain = e
        .chars()
        .all(|c| c != '.' && c != '[' && c != ']' && c != '*');
    if plain {
        // A bare member name, spaces and all.
        return format!("$['{}']", e.replace('\\', "\\\\").replace('\'', "\\'"));
    }
    format!("$.{e}")
}

fn compile(expr: &str, what: &str) -> Result<JsonPath, String> {
    let query = as_jsonpath(expr);
    JsonPath::parse(&query).map_err(|e| format!("the JSONPath {what} \"{expr}\" is invalid: {e}"))
}

/// The values one selected node contributes.
fn values_of(v: &Value, expr: &str, read_as: ReadAs, out: &mut Vec<Val>) -> Result<(), String> {
    let core = read_as.rml_core;
    match v {
        Value::Null => {
            if !read_as.empty_is_value {
                // A legacy version read a JSON null as the empty string.
                out.push(Val::text(""));
            }
        }
        Value::String(s) => out.push(Val::text(s.clone())),
        Value::Bool(b) => out.push(Val {
            lex: b.to_string(),
            kind: core.then_some(ValueKind::Boolean),
        }),
        Value::Number(n) => {
            let kind = if n.is_i64() || n.is_u64() {
                ValueKind::Integer
            } else {
                ValueKind::Float
            };
            out.push(Val {
                lex: n.to_string(),
                kind: core.then_some(kind),
            })
        }
        Value::Array(items) if !core => {
            for item in items {
                match item {
                    Value::Array(_) | Value::Object(_) => out.push(Val::text(item.to_string())),
                    scalar => values_of(scalar, expr, read_as, out)?,
                }
            }
        }
        Value::Object(_) if !core => out.push(Val::text(v.to_string())),
        Value::Array(_) | Value::Object(_) => {
            return Err(format!(
                "the JSONPath reference \"{expr}\" selects {}, which is not a value a term can be \
                 made from; select its members (for an array, \"{expr}[*]\")",
                if v.is_array() {
                    "an array"
                } else {
                    "an object"
                }
            ))
        }
    }
    Ok(())
}

/// The logical iterations of a JSON (or JSON Lines) source.
pub fn load(
    text: &str,
    iterator: Option<&str>,
    references: &[String],
    json_lines: bool,
    read_as: ReadAs,
) -> Result<Vec<Iteration>, String> {
    let iterator_expr = iterator
        .map(str::trim)
        .filter(|i| !i.is_empty())
        .unwrap_or("$");
    let iterator_path = compile(iterator_expr, "iterator")?;
    let mut compiled: Vec<(&String, JsonPath)> = Vec::with_capacity(references.len());
    for r in references {
        compiled.push((r, compile(r, "reference")?));
    }

    let documents: Vec<Value> = if json_lines {
        text.lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty())
            .map(|(i, l)| {
                serde_json::from_str(l).map_err(|e| format!("JSON Lines line {}: {e}", i + 1))
            })
            .collect::<Result<_, _>>()?
    } else {
        vec![serde_json::from_str(text).map_err(|e| format!("JSON parse error: {e}"))?]
    };

    let mut out = Vec::new();
    for doc in &documents {
        let mut nodes: Vec<&Value> = iterator_path.query(doc).all();
        // The legacy vocabulary iterated the elements of an array its
        // iterator selected (`$.people`, or `$` over a top-level array).
        if !read_as.rml_core && nodes.len() == 1 {
            if let Value::Array(items) = nodes[0] {
                nodes = items.iter().collect();
            }
        }
        for node in nodes {
            let mut values: HashMap<String, Vec<Val>> = HashMap::with_capacity(compiled.len());
            for (expr, path) in &compiled {
                let mut vals = Vec::new();
                for v in path.query(node).all() {
                    values_of(v, expr, read_as, &mut vals)?;
                }
                if !vals.is_empty() {
                    values.insert((*expr).clone(), vals);
                }
            }
            out.push(Iteration { values });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rml::terms::Record;

    const CORE: ReadAs = ReadAs {
        rml_core: true,
        empty_is_value: true,
    };
    const LEGACY_VOCAB: ReadAs = ReadAs {
        rml_core: false,
        empty_is_value: true,
    };

    fn refs(r: &[&str]) -> Vec<String> {
        r.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_reference_may_select_several_values_with_their_types() {
        let its = load(
            r#"{"people":[{"id":1,"tags":["a","b"],"w":1.5,"ok":true,"none":null}]}"#,
            Some("$.people[*]"),
            &refs(&["$.id", "$.tags[*]", "$.w", "$.ok", "$.none", "$.missing"]),
            false,
            CORE,
        )
        .unwrap();
        assert_eq!(its.len(), 1);
        let it = &its[0];
        assert_eq!(it.values("$.tags[*]").len(), 2);
        assert_eq!(it.values("$.id").get(0), ("1", Some(ValueKind::Integer)));
        assert_eq!(it.values("$.w").get(0), ("1.5", Some(ValueKind::Float)));
        assert_eq!(it.values("$.ok").get(0), ("true", Some(ValueKind::Boolean)));
        assert!(it.values("$.none").is_empty(), "null is no value");
        assert!(it.values("$.missing").is_empty());
    }

    #[test]
    fn an_array_or_object_value_is_an_error_under_rml_core_only() {
        let doc = r#"[{"a":[1,2],"o":{"x":1}}]"#;
        let err = load(doc, Some("$[*]"), &refs(&["$.a"]), false, CORE).unwrap_err();
        assert!(err.contains("an array") && err.contains("$.a[*]"), "{err}");
        let its = load(doc, Some("$[*]"), &refs(&["a", "o"]), false, LEGACY_VOCAB).unwrap();
        assert_eq!(
            its[0].values("a").len(),
            2,
            "the legacy vocabulary reads the elements"
        );
        assert_eq!(its[0].values("o").first(), Some("{\"x\":1}"));
        assert_eq!(its[0].values("a").get(0).1, None, "and types nothing");
    }

    #[test]
    fn bare_names_are_members_spaces_and_all() {
        let its = load(
            r#"{"c":[{"Country Code":7,"a":{"b":"deep"}}]}"#,
            Some("$.c[*]"),
            &refs(&["Country Code", "a.b"]),
            false,
            LEGACY_VOCAB,
        )
        .unwrap();
        assert_eq!(its[0].values("Country Code").first(), Some("7"));
        assert_eq!(its[0].values("a.b").first(), Some("deep"));
    }

    #[test]
    fn the_legacy_vocabulary_iterates_a_selected_array() {
        let its = load(
            r#"[{"n":1},{"n":2}]"#,
            None,
            &refs(&["n"]),
            false,
            LEGACY_VOCAB,
        )
        .unwrap();
        assert_eq!(its.len(), 2);
        let core = load(
            r#"[{"n":1},{"n":2}]"#,
            Some("$[*]"),
            &refs(&["$.n"]),
            false,
            CORE,
        )
        .unwrap();
        assert_eq!(core.len(), 2);
    }

    #[test]
    fn json_lines_are_documents_in_turn() {
        let its = load(
            "{\"n\":1}\n\n{\"n\":2}\n",
            Some("$"),
            &refs(&["$.n"]),
            true,
            CORE,
        )
        .unwrap();
        assert_eq!(its.len(), 2);
        assert_eq!(its[1].values("$.n").first(), Some("2"));
    }

    #[test]
    fn an_invalid_jsonpath_is_a_mapping_error() {
        let err = load("{}", Some("$.students[*]]"), &[], false, CORE).unwrap_err();
        assert!(err.contains("iterator") && err.contains("invalid"), "{err}");
    }
}
