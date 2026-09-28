//! A minimal YAML document tree.
//!
//! The translators in this module work against [`Node`] rather than a YAML
//! crate's own value type, for two reasons: the translation logic is then
//! testable without writing YAML at all, and swapping the parser is a change
//! to one file ([`super::yaml`]) rather than to every rule.

use std::collections::BTreeMap;

/// A YAML value, reduced to what a mapping document can contain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Null,
    Scalar(String),
    Seq(Vec<Node>),
    Map(BTreeMap<String, Node>),
}

impl Node {
    // Tree builders. Only tests construct a document by hand; parsing builds
    // one from YAML.
    #[cfg(test)]
    pub fn map(pairs: impl IntoIterator<Item = (&'static str, Node)>) -> Node {
        Node::Map(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }
    #[cfg(test)]
    pub fn seq(items: impl IntoIterator<Item = Node>) -> Node {
        Node::Seq(items.into_iter().collect())
    }
    #[cfg(test)]
    pub fn scalar(s: impl Into<String>) -> Node {
        Node::Scalar(s.into())
    }

    /// The value at `key`, for a map. `None` for any other node, and for a
    /// key whose value is explicitly null — an absent setting and one written
    /// as `~` mean the same thing to every rule here.
    pub fn get(&self, key: &str) -> Option<&Node> {
        match self {
            Node::Map(m) => m.get(key).filter(|v| !matches!(v, Node::Null)),
            _ => None,
        }
    }

    /// The first present value among `keys`. YARRRML gives almost every key a
    /// short and a long spelling (`s`/`subject`, `po`/`predicateobjects`), and
    /// a document may mix them freely.
    pub fn get_any(&self, keys: &[&str]) -> Option<&Node> {
        keys.iter().find_map(|k| self.get(k))
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Node::Scalar(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_map(&self) -> Option<&BTreeMap<String, Node>> {
        match self {
            Node::Map(m) => Some(m),
            _ => None,
        }
    }

    /// The node as a sequence. A bare scalar or map counts as a one-element
    /// sequence, which is how YARRRML lets `sources: x` stand for
    /// `sources: [x]`.
    pub fn as_seq(&self) -> Vec<&Node> {
        match self {
            Node::Seq(items) => items.iter().collect(),
            Node::Null => Vec::new(),
            other => vec![other],
        }
    }

    /// A scalar reached through `keys`, trimmed and non-empty.
    pub fn str_at(&self, keys: &[&str]) -> Option<String> {
        self.get_any(keys)
            .and_then(Node::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }

    /// A type name for error messages, so a mistyped document says what it
    /// actually contained.
    pub fn kind(&self) -> &'static str {
        match self {
            Node::Null => "null",
            Node::Scalar(_) => "a scalar",
            Node::Seq(_) => "a list",
            Node::Map(_) => "a map",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> Node {
        Node::map([
            ("s", Node::scalar("http://x/$(id)")),
            ("empty", Node::scalar("   ")),
            ("nothing", Node::Null),
            ("po", Node::seq([Node::scalar("a"), Node::scalar("b")])),
        ])
    }

    #[test]
    fn lookup_treats_absent_and_null_alike() {
        assert_eq!(
            doc().get("nothing"),
            None,
            "an explicit null is not a value"
        );
        assert_eq!(doc().get("missing"), None);
        assert_eq!(
            doc().get("s").and_then(Node::as_str),
            Some("http://x/$(id)")
        );
    }

    #[test]
    fn get_any_takes_the_first_spelling_present() {
        let d = doc();
        assert_eq!(
            d.get_any(&["subject", "s"]).and_then(Node::as_str),
            Some("http://x/$(id)")
        );
        assert_eq!(d.get_any(&["nope", "alsonope"]), None);
    }

    #[test]
    fn str_at_rejects_blank_scalars() {
        assert_eq!(doc().str_at(&["empty"]), None, "whitespace is not a value");
        assert_eq!(doc().str_at(&["s"]).as_deref(), Some("http://x/$(id)"));
    }

    #[test]
    fn a_bare_value_counts_as_a_one_element_sequence() {
        assert_eq!(Node::scalar("x").as_seq().len(), 1);
        assert_eq!(doc().get("po").unwrap().as_seq().len(), 2);
        assert!(Node::Null.as_seq().is_empty());
    }

    #[test]
    fn kind_names_what_was_actually_there() {
        assert_eq!(Node::Null.kind(), "null");
        assert_eq!(Node::scalar("x").kind(), "a scalar");
        assert_eq!(Node::seq([]).kind(), "a list");
        assert_eq!(Node::map([]).kind(), "a map");
    }
}
