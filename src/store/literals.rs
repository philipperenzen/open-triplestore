//! Literals a write may not store: an `rdf:JSON` literal whose lexical form
//! is not JSON.
//!
//! `rdf:JSON` (JSON-LD 1.1 §10.2, the RDF 1.2 JSON datatype) has as its
//! lexical space the strings that match the JSON grammar of RFC 8259 §2. A
//! literal outside it is *ill-typed*: it denotes no JSON value, a JSON-LD
//! processor refuses to serialise it (`invalid JSON literal`, json-ld-api
//! `fromRdf` tests js08 and js09), and every JSON-LD download of the graph
//! holding it would fail. So the store refuses it on the way in — uploads,
//! SPARQL UPDATE, the Graph Store Protocol, LDP, RDF Patch and imports alike
//! — with a 400 naming the literal, and never holds one.
//!
//! The JSON grammar is checked with the parser the JSON-LD processor itself
//! uses (`json-event-parser`), so what the store accepts is exactly what the
//! serialiser can write.

use json_event_parser::{JsonEvent, SliceJsonParser};
use oxigraph::model::{LiteralRef, Quad, QuadRef, TermRef};

use super::engine::StoreError;

/// `rdf:JSON`.
pub const RDF_JSON: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#JSON";

/// Why `lexical` is not in the lexical space of `rdf:JSON`: not exactly one
/// JSON value (RFC 8259 §2, whitespace around it allowed). `None` when it is.
pub fn json_lexical_error(lexical: &str) -> Option<String> {
    let mut parser = SliceJsonParser::new(lexical.as_bytes());
    let mut values = 0usize;
    let mut depth = 0usize;
    loop {
        match parser.parse_next() {
            Err(e) => return Some(e.to_string()),
            Ok(JsonEvent::Eof) => {
                return (values != 1).then(|| "no JSON value".to_string());
            }
            Ok(JsonEvent::StartArray | JsonEvent::StartObject) => {
                if depth == 0 {
                    values += 1;
                }
                depth += 1;
            }
            Ok(JsonEvent::EndArray | JsonEvent::EndObject) => depth = depth.saturating_sub(1),
            Ok(JsonEvent::ObjectKey(_)) => {}
            Ok(
                JsonEvent::String(_)
                | JsonEvent::Number(_)
                | JsonEvent::Boolean(_)
                | JsonEvent::Null,
            ) => {
                if depth == 0 {
                    values += 1;
                }
            }
        }
    }
}

/// Why `literal` is ill-typed, for the datatypes the store checks.
pub fn ill_typed(literal: LiteralRef<'_>) -> Option<String> {
    if literal.datatype().as_str() == RDF_JSON {
        return json_lexical_error(literal.value()).map(|why| {
            format!(
                "the rdf:JSON literal {literal} is ill-typed: its lexical form is not JSON \
                 ({why}). rdf:JSON takes a JSON text (JSON-LD 1.1 §10.2, RFC 8259); \
                 nothing was written"
            )
        });
    }
    None
}

/// The first ill-typed literal `term` holds — itself, or one inside a
/// quoted triple.
fn ill_typed_term(term: TermRef<'_>) -> Option<String> {
    match term {
        TermRef::Literal(l) => ill_typed(l),
        #[cfg(feature = "rdf-12")]
        TermRef::Triple(t) => ill_typed_term(t.object.as_ref()),
        _ => None,
    }
}

/// Refuse a quad that holds an ill-typed literal.
pub fn check_quad(quad: QuadRef<'_>) -> Result<(), StoreError> {
    match ill_typed_term(quad.object) {
        Some(why) => Err(StoreError::IllTypedLiteral(why)),
        None => Ok(()),
    }
}

/// Refuse a batch of quads when any holds an ill-typed literal, before any
/// is written.
pub fn check_quads<'a>(quads: impl IntoIterator<Item = &'a Quad>) -> Result<(), StoreError> {
    quads.into_iter().try_for_each(|q| check_quad(q.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxigraph::model::{GraphName, Literal, NamedNode};

    fn json(lexical: &str) -> Quad {
        Quad::new(
            NamedNode::new_unchecked("http://example.org/s"),
            NamedNode::new_unchecked("http://example.org/p"),
            Literal::new_typed_literal(lexical, NamedNode::new_unchecked(RDF_JSON)),
            GraphName::DefaultGraph,
        )
    }

    #[test]
    fn every_json_value_is_in_the_lexical_space() {
        for ok in [
            "true",
            "false",
            "null",
            "0",
            "-1.5e3",
            "\"string\"",
            "[]",
            "{}",
            "[{\"foo\":\"bar\"}]",
            " { \"a\" : [1, 2, {\"b\": null}] } ",
            "\"\\u00e9\"",
        ] {
            assert_eq!(json_lexical_error(ok), None, "{ok:?}");
            assert!(check_quad(json(ok).as_ref()).is_ok(), "{ok:?}");
        }
    }

    /// json-ld-api fromRdf js08 and js09, and what else is not one JSON
    /// value.
    #[test]
    fn what_is_not_one_json_value_is_ill_typed() {
        for bad in [
            "bareword",
            "[{]",
            "",
            "   ",
            "1 2",
            "{} []",
            "{\"a\":}",
            "'single'",
            "[1,]",
            "\"\\ud800\"",
        ] {
            assert!(json_lexical_error(bad).is_some(), "{bad:?}");
            let e = check_quad(json(bad).as_ref()).unwrap_err();
            assert!(matches!(e, StoreError::IllTypedLiteral(_)), "{bad:?}");
            assert!(e.to_string().contains("ill-typed"), "{e}");
        }
    }

    #[test]
    fn other_datatypes_are_not_checked_here() {
        let q = Quad::new(
            NamedNode::new_unchecked("http://example.org/s"),
            NamedNode::new_unchecked("http://example.org/p"),
            Literal::new_simple_literal("bareword"),
            GraphName::DefaultGraph,
        );
        assert!(check_quad(q.as_ref()).is_ok());
        assert!(check_quads([&q, &json("[1]")]).is_ok());
        assert!(check_quads([&q, &json("[1")]).is_err());
    }
}
