//! Upload checks on a shapes document, run on the raw text before it is stored.
//!
//! The store keeps `xsd:boolean` as a native value, so `"1"^^xsd:boolean`
//! reads back as `true` and `"0"^^xsd:boolean` as `false`. SHACL activates a
//! flag only for the literal `true` (W3C test core/property/uniqueLang-002),
//! so once stored, a `"1"` the author meant as "not `true`, so off" is
//! indistinguishable from `true`. The engine can never see the difference;
//! the upload can, so it refuses the ambiguous form and asks for `true` or
//! `false`. This does not cover every write path (Graph Store, SPARQL Update,
//! imports and seeds store what they are given); see `docs/shacl.md`.

use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::vocab::xsd;
use oxigraph::model::Term;

const SH: &str = "http://www.w3.org/ns/shacl#";

/// The boolean flags whose activation depends on the literal `true`.
const ACTIVATION_FLAGS: &[&str] = &[
    "uniqueLang",
    "closed",
    "deactivated",
    "qualifiedValueShapesDisjoint",
    "optional",
];

/// At most this many offending triples are named in the refusal.
const MAX_REPORTED: usize = 5;

/// Refuse a Turtle shapes document that writes an activation flag as an
/// `xsd:boolean` other than `true` or `false` (`"1"`, `"0"`, …). A document
/// that does not parse passes: storing it fails with the parser's own error.
pub fn check_activation_flags(turtle: &str) -> Result<(), String> {
    let mut offending = Vec::new();
    let mut total = 0usize;
    for quad in RdfParser::from_format(RdfFormat::Turtle).for_slice(turtle.as_bytes()) {
        let Ok(quad) = quad else {
            return Ok(());
        };
        let Some(flag) = quad
            .predicate
            .as_str()
            .strip_prefix(SH)
            .filter(|local| ACTIVATION_FLAGS.contains(local))
        else {
            continue;
        };
        let Term::Literal(lit) = &quad.object else {
            continue;
        };
        if lit.datatype() != xsd::BOOLEAN || matches!(lit.value(), "true" | "false") {
            continue;
        }
        total += 1;
        if offending.len() < MAX_REPORTED {
            offending.push(format!("{} sh:{flag} {lit}", quad.subject));
        }
    }
    if offending.is_empty() {
        return Ok(());
    }
    let more = total - offending.len();
    Err(format!(
        "SHACL activates sh:uniqueLang, sh:closed, sh:deactivated, \
         sh:qualifiedValueShapesDisjoint and sh:optional only for the literal true, \
         but this store keeps booleans as values, so another form would read back as \
         true or false and change what the shape means. Write true or false instead: {}{}",
        offending.join("; "),
        if more > 0 {
            format!(" (and {more} more)")
        } else {
            String::new()
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PFX: &str = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
        @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
        @prefix ex: <http://example.org/> .\n";

    #[test]
    fn canonical_booleans_pass() {
        let doc = format!(
            "{PFX}ex:S sh:closed true ; sh:deactivated false ; \
             sh:uniqueLang \"true\"^^xsd:boolean ; sh:qualifiedValueShapesDisjoint \"false\"^^xsd:boolean ."
        );
        assert_eq!(check_activation_flags(&doc), Ok(()));
    }

    #[test]
    fn a_non_canonical_boolean_on_a_flag_is_refused() {
        for (flag, lexical) in [
            ("uniqueLang", "1"),
            ("closed", "0"),
            ("deactivated", "1"),
            ("qualifiedValueShapesDisjoint", "1"),
            ("optional", "0"),
        ] {
            let doc = format!("{PFX}ex:S sh:{flag} \"{lexical}\"^^xsd:boolean .");
            let err = check_activation_flags(&doc).expect_err(flag);
            assert!(
                err.contains(&format!("sh:{flag}")) && err.contains(&format!("\"{lexical}\"")),
                "{err}"
            );
        }
    }

    #[test]
    fn other_predicates_and_unparseable_documents_pass() {
        // Not an activation flag.
        let doc =
            format!("{PFX}ex:S ex:flag \"1\"^^xsd:boolean ; sh:hasValue \"1\"^^xsd:boolean .");
        assert_eq!(check_activation_flags(&doc), Ok(()));
        // The store reports the parse error itself.
        assert_eq!(check_activation_flags("this is not turtle"), Ok(()));
    }

    #[test]
    fn the_refusal_names_a_bounded_number_of_triples() {
        let body: String = (0..8)
            .map(|i| format!("ex:S{i} sh:closed \"1\"^^xsd:boolean .\n"))
            .collect();
        let err = check_activation_flags(&format!("{PFX}{body}")).unwrap_err();
        assert!(err.contains("(and 3 more)"), "{err}");
    }
}
