//! Parsing YAML into the [`Node`](super::node::Node) tree.
//!
//! The only place a YAML crate is named. Everything else in this module works
//! against `Node`, so the parser is one dependency behind one function.

use std::collections::BTreeMap;

use yaml_rust2::{Yaml, YamlLoader};

use super::node::Node;

/// Parse a single YAML document. A file with several documents is an error:
/// a mapping file describes one mapping set, and silently taking the first
/// would drop the rest.
pub fn parse(text: &str) -> Result<Node, String> {
    let docs = YamlLoader::load_from_str(text).map_err(|e| e.to_string())?;
    match docs.len() {
        0 => Err("the document is empty".to_string()),
        1 => Ok(convert(&docs[0])),
        n => Err(format!(
            "the file holds {n} YAML documents; a mapping file describes exactly one"
        )),
    }
}

fn convert(value: &Yaml) -> Node {
    match value {
        Yaml::Null | Yaml::BadValue => Node::Null,
        // Every scalar becomes text. A mapping document's scalars are IRIs,
        // column names, templates and datatypes — never arithmetic — and
        // keeping one representation means the rules never branch on YAML's
        // guess at a type. YAML would otherwise read a column named `on` as a
        // boolean and a version `1.10` as the number 1.1.
        Yaml::Real(s) | Yaml::String(s) => Node::Scalar(s.clone()),
        Yaml::Integer(i) => Node::Scalar(i.to_string()),
        Yaml::Boolean(b) => Node::Scalar(b.to_string()),
        Yaml::Array(items) => Node::Seq(items.iter().map(convert).collect()),
        Yaml::Hash(map) => {
            let mut out = BTreeMap::new();
            for (k, v) in map {
                if let Some(key) = key_of(k) {
                    out.insert(key, convert(v));
                }
            }
            Node::Map(out)
        }
        // An unresolved alias means the anchor was never defined; treating it
        // as absent lets the surrounding rule report the missing setting by
        // name rather than failing on the alias.
        Yaml::Alias(_) => Node::Null,
    }
}

fn key_of(key: &Yaml) -> Option<String> {
    match key {
        Yaml::String(s) | Yaml::Real(s) => Some(s.clone()),
        Yaml::Integer(i) => Some(i.to_string()),
        Yaml::Boolean(b) => Some(b.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_and_flow_styles_both_parse() {
        let block =
            parse("prefixes:\n  ex: http://example.org/\nmappings:\n  a:\n    s: x\n").unwrap();
        let flow =
            parse("{prefixes: {ex: \"http://example.org/\"}, mappings: {a: {s: x}}}").unwrap();
        assert_eq!(block, flow, "the prompt's own fixtures mix both styles");
        assert_eq!(
            block.get("prefixes").unwrap().str_at(&["ex"]).as_deref(),
            Some("http://example.org/")
        );
    }

    #[test]
    fn scalars_keep_their_written_form() {
        let d = parse("a: on\nb: 1.10\nc: 42\nd: true\ne: null\n").unwrap();
        assert_eq!(
            d.str_at(&["a"]).as_deref(),
            Some("on"),
            "a column named 'on' is not a boolean"
        );
        assert_eq!(
            d.str_at(&["b"]).as_deref(),
            Some("1.10"),
            "a version is not a float"
        );
        assert_eq!(d.str_at(&["c"]).as_deref(), Some("42"));
        assert_eq!(d.str_at(&["d"]).as_deref(), Some("true"));
        assert_eq!(d.get("e"), None, "an explicit null reads as absent");
    }

    #[test]
    fn nested_lists_of_maps_survive() {
        let d = parse(
            "po:\n  - [ex:name, $(name)]\n  - p: ex:price\n    o:\n      value: $(price)\n      datatype: xsd:decimal\n",
        )
        .unwrap();
        let entries = d.get("po").unwrap().as_seq();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].as_seq().len(), 2);
        assert_eq!(
            entries[1]
                .get("o")
                .unwrap()
                .str_at(&["datatype"])
                .as_deref(),
            Some("xsd:decimal")
        );
    }

    #[test]
    fn malformed_and_multi_document_files_are_refused() {
        assert!(parse("a:\n  - b\n : broken").is_err());
        assert!(parse("").unwrap_err().contains("empty"));
        let err = parse("a: 1\n---\nb: 2\n").unwrap_err();
        assert!(err.contains("exactly one"), "{err}");
    }

    #[test]
    fn a_quoted_scalar_keeps_its_special_characters() {
        let d = parse("q: \"a: b {c} $(d)\"\n").unwrap();
        assert_eq!(d.str_at(&["q"]).as_deref(), Some("a: b {c} $(d)"));
    }
}
