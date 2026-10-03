//! IRIs and blank-node labels as R2RML generates them.
//!
//! * The **IRI-safe** form of a template value (R2RML §7.3): every character
//!   outside RFC 3987 `iunreserved` is UTF-8 percent-encoded. `~A_17.1-2` and
//!   `葉篤正` stay as they are; `Hello World!` becomes `Hello%20World%21`.
//! * **Base IRI** resolution (§11.2): a value that is not an absolute IRI is
//!   appended to the base IRI, and the result must be one.
//! * **Blank-node labels** (§11.2, §9.1): a blank node is unique to its value
//!   and scoped to one graph. Hashing the run, the graph and the value gives a
//!   label that needs no lookup table, is the same in every batch, and is the
//!   same whichever side of a pushed-down join computes it.

use oxigraph::model::NamedNode;
use sha2::{Digest, Sha256};

/// RFC 3987 `iunreserved`: `ALPHA / DIGIT / "-" / "." / "_" / "~" / ucschar`.
pub fn is_iunreserved(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~') || is_ucschar(c)
}

/// RFC 3987 `ucschar`.
fn is_ucschar(c: char) -> bool {
    let u = c as u32;
    matches!(u, 0xA0..=0xD7FF | 0xF900..=0xFDCF | 0xFDF0..=0xFFEF | 0xE1000..=0xEFFFD)
        // %x10000-1FFFD … %xD0000-DFFFD: every supplementary plane but the
        // last two code points of each.
        || ((0x10000..=0xDFFFD).contains(&u) && (u & 0xFFFF) <= 0xFFFD)
}

/// The IRI-safe version of a template value (R2RML §7.3).
pub fn iri_safe(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut buf = [0u8; 4];
    for c in value.chars() {
        if is_iunreserved(c) {
            out.push(c);
        } else {
            for b in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

/// `value` as an IRI: itself when it is absolute, otherwise appended to
/// `base` (R2RML resolves by concatenation, not by RFC 3986 reference
/// resolution). `None` when neither is a valid absolute IRI.
pub fn absolute_iri(value: &str, base: Option<&str>) -> Option<NamedNode> {
    NamedNode::new(value)
        .ok()
        .or_else(|| NamedNode::new(format!("{}{value}", base?)).ok())
}

/// A blank-node label unique to `value` within `graph` (`None` = the target
/// graph) for one run. `prefix` keeps two runs' nodes apart.
pub fn blank_node_label(prefix: &str, graph: Option<&str>, value: &str) -> String {
    let mut h = Sha256::new();
    // Length-prefixed, so ("ab", "c") and ("a", "bc") cannot collide.
    let g = graph.unwrap_or("");
    h.update((g.len() as u64).to_be_bytes());
    h.update(g.as_bytes());
    h.update(value.as_bytes());
    let digest = h.finalize();
    // 128 bits: no collision to worry about at any row count a run reaches.
    format!("_:{prefix}{}", hex::encode(&digest[..16]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_iri_safe_examples_of_r2rml_7_3() {
        assert_eq!(iri_safe("42"), "42");
        assert_eq!(iri_safe("Hello World!"), "Hello%20World%21");
        assert_eq!(iri_safe("2011-08-23T22:17:00Z"), "2011-08-23T22%3A17%3A00Z");
        assert_eq!(iri_safe("~A_17.1-2"), "~A_17.1-2");
        assert_eq!(iri_safe("葉篤正"), "葉篤正");
        assert_eq!(iri_safe("NEW YORK"), "NEW%20YORK");
    }

    #[test]
    fn delimiters_and_controls_are_encoded_as_utf8_octets() {
        assert_eq!(iri_safe("a/b?c#d"), "a%2Fb%3Fc%23d");
        assert_eq!(iri_safe("<>\""), "%3C%3E%22");
        assert_eq!(iri_safe("\u{7f}"), "%7F");
        // U+FFFE is outside ucschar, U+10000 inside it.
        assert_eq!(iri_safe("\u{FFFE}"), "%EF%BF%BE");
        assert_eq!(iri_safe("\u{10000}"), "\u{10000}");
        assert_eq!(iri_safe("\u{1FFFE}"), "%F0%9F%BF%BE");
    }

    #[test]
    fn relative_values_resolve_against_the_base_by_concatenation() {
        assert_eq!(
            absolute_iri("http://x/a", Some("http://base/"))
                .unwrap()
                .as_str(),
            "http://x/a"
        );
        assert_eq!(
            absolute_iri("Student/10", Some("http://example.com/base/"))
                .unwrap()
                .as_str(),
            "http://example.com/base/Student/10"
        );
        assert_eq!(absolute_iri("Student/10", None), None);
        assert_eq!(absolute_iri("has space", Some("http://b/")), None);
    }

    #[test]
    fn blank_node_labels_depend_on_run_graph_and_value_only() {
        let a = blank_node_label("r1_", None, "x");
        assert_eq!(a, blank_node_label("r1_", None, "x"), "deterministic");
        assert!(a.starts_with("_:r1_"));
        assert_ne!(a, blank_node_label("r2_", None, "x"), "per run");
        assert_ne!(
            a,
            blank_node_label("r1_", Some("http://g"), "x"),
            "per graph"
        );
        assert_ne!(a, blank_node_label("r1_", None, "y"), "per value");
        assert_ne!(
            blank_node_label("p", Some("ab"), "c"),
            blank_node_label("p", Some("a"), "bc")
        );
    }
}
