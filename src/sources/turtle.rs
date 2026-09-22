//! Turtle for the documents this feature serves: a prefix header for the
//! namespaces a document actually uses, and nothing declared that it does not.
//!
//! Two resolvers. [`fixed`] declares from a list the caller owns, for documents
//! whose vocabulary is the store's own — a profile, a PROV trail — so they read
//! the same on every deployment and in a test whose prefix registry is empty.
//! A registry-backed closure (`|ns| registry.declaration_for(ns)`) is for
//! documents written in the author's namespaces, such as a mapping or the
//! entities a dry-run produced.

use oxigraph::io::RdfFormat;
use oxigraph::model::Triple;

use crate::store::TripleStore;

/// Serialise `triples` as Turtle, declaring a prefix for each namespace
/// `resolve` answers for. The header claims each label once, exact
/// declarations first, exactly as a Graph Store read does.
pub fn turtle_of<F>(triples: &[Triple], resolve: F) -> String
where
    F: Fn(&str) -> Option<(String, String)>,
{
    if triples.is_empty() {
        return String::new();
    }
    let mut nt = String::with_capacity(triples.len() * 96);
    for t in triples {
        nt.push_str(&format!("{} {} {} .\n", t.subject, t.predicate, t.object));
    }
    ntriples_to_turtle(&nt, resolve)
}

/// The same, from N-Triples text — what an entity description comes as.
///
/// Falls back to the N-Triples it was given, which is valid Turtle, rather
/// than failing a request over a prefix header.
pub fn ntriples_to_turtle<F>(ntriples: &str, resolve: F) -> String
where
    F: Fn(&str) -> Option<(String, String)>,
{
    if ntriples.trim().is_empty() {
        return String::new();
    }
    let Ok(temp) = TripleStore::in_memory() else {
        return ntriples.to_string();
    };
    if temp.load_str(ntriples, RdfFormat::NTriples, None).is_err() {
        return ntriples.to_string();
    }
    match temp.dump_prefixed(RdfFormat::Turtle, None, resolve) {
        Ok(bytes) => String::from_utf8(bytes).unwrap_or_else(|_| ntriples.to_string()),
        Err(_) => ntriples.to_string(),
    }
}

/// A resolver over a fixed `(label, namespace)` list: a namespace is declared
/// only when it is exactly one of these.
pub fn fixed<'a>(
    candidates: &'a [(&'a str, &'a str)],
) -> impl Fn(&str) -> Option<(String, String)> + 'a {
    move |ns| {
        candidates
            .iter()
            .find(|(_, candidate)| *candidate == ns)
            .map(|(label, candidate)| (label.to_string(), candidate.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxigraph::model::{Literal, NamedNode, Triple};

    fn triple(s: &str, p: &str, o: &str) -> Triple {
        Triple::new(
            NamedNode::new(s).unwrap(),
            NamedNode::new(p).unwrap(),
            NamedNode::new(o).unwrap(),
        )
    }

    #[test]
    fn only_the_namespaces_used_are_declared() {
        let triples = vec![
            triple(
                "urn:x:1",
                "http://www.w3.org/1999/02/22-rdf-syntax-ns#type",
                "http://example.org/vocab#Thing",
            ),
            Triple::new(
                NamedNode::new("urn:x:1").unwrap(),
                NamedNode::new("http://example.org/vocab#count").unwrap(),
                Literal::new_typed_literal(
                    "3",
                    NamedNode::new("http://www.w3.org/2001/XMLSchema#integer").unwrap(),
                ),
            ),
        ];
        let candidates = [
            ("rdf", "http://www.w3.org/1999/02/22-rdf-syntax-ns#"),
            ("ex", "http://example.org/vocab#"),
            ("xsd", "http://www.w3.org/2001/XMLSchema#"),
            ("unused", "http://example.org/unused#"),
        ];
        let out = turtle_of(&triples, fixed(&candidates));
        assert!(
            out.contains("@prefix ex: <http://example.org/vocab#>"),
            "{out}"
        );
        assert!(
            out.contains("ex:Thing") || out.contains("a ex:Thing"),
            "{out}"
        );
        assert!(!out.contains("@prefix unused:"), "a dead line: {out}");
        assert!(!out.contains("<http://example.org/vocab#count>"), "{out}");
        // The document still parses as Turtle and says the same thing.
        let back = TripleStore::in_memory().unwrap();
        back.load_str(&out, RdfFormat::Turtle, None).unwrap();
        assert_eq!(back.count_graph(None).unwrap(), 2);
    }

    #[test]
    fn nothing_in_nothing_out_and_bad_input_passes_through() {
        assert_eq!(turtle_of(&[], fixed(&[])), "");
        assert_eq!(ntriples_to_turtle("   ", fixed(&[])), "");
        assert_eq!(
            ntriples_to_turtle("not n-triples at all", fixed(&[])),
            "not n-triples at all"
        );
    }
}
