pub mod engine;
pub mod parallel_mirror;
pub mod path_cache;
pub mod query_cache;
pub mod recovery;

pub use engine::{StoreError, TripleStore};

/// Percent-encode the characters that are illegal inside a SPARQL `IRIREF`
/// (the `<...>` form): `<>"{}|^`\` plus any control/space char (≤ 0x20).
///
/// Well-formed absolute IRIs contain none of these, so this is a no-op for all
/// real input. It exists as a defense-in-depth guard for the code paths that
/// interpolate stored/registry IRIs into SPARQL via `format!`: even if a
/// malformed value ever reached one, it can no longer terminate the `<...>`
/// and inject surrounding SPARQL syntax.
pub fn escape_sparql_iri(iri: &str) -> String {
    let mut out = String::with_capacity(iri.len());
    for c in iri.chars() {
        match c {
            '<' | '>' | '"' | '{' | '}' | '|' | '^' | '`' | '\\' => {
                out.push_str(&format!("%{:02X}", c as u32));
            }
            c if (c as u32) <= 0x20 => out.push_str(&format!("%{:02X}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Escape a string for use inside a SPARQL/Turtle double-quoted string literal
/// (`"..."`). Escapes `\`, `"`, and the CR/LF/TAB control characters that are
/// illegal raw inside a `STRING_LITERAL_QUOTE`. Pair with [`escape_sparql_iri`]
/// for IRIs when building SPARQL via `format!`.
pub fn escape_sparql_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{escape_sparql_iri, escape_sparql_literal};

    #[test]
    fn valid_iri_unchanged() {
        let iri = "http://example.org/path/Resource#frag-1";
        assert_eq!(escape_sparql_iri(iri), iri);
    }

    #[test]
    fn injection_chars_encoded() {
        // A payload trying to break out of <...> and append a triple pattern.
        let evil = "http://x/a> ?p ?o }} INSERT DATA {{ <http://x/y";
        let escaped = escape_sparql_iri(evil);
        assert!(!escaped.contains('>'));
        assert!(!escaped.contains('<'));
        assert!(!escaped.contains('{'));
        assert!(!escaped.contains('}'));
        assert!(!escaped.contains(' '));
    }

    #[test]
    fn literal_escapes_quote_backslash_and_controls() {
        assert_eq!(escape_sparql_literal("a\"b"), "a\\\"b");
        assert_eq!(escape_sparql_literal("a\\b"), "a\\\\b");
        assert_eq!(escape_sparql_literal("a\nb\rc\td"), "a\\nb\\rc\\td");
    }
}
