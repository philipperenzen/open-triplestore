//! Safe "smart injection" of typed variables into a saved query.
//!
//! A saved query's text contains `{{name}}` placeholders. Each declared
//! [`ParamSpec`] says how a supplied value must be rendered into SPARQL — as an
//! IRI, an escaped string literal, a number, a boolean, or a typed date.
//!
//! The value is never pasted verbatim: every type renders a *validated, escaped*
//! term, so a caller cannot break out of a literal or smuggle extra SPARQL.
//! This is the first line of defence; `scope_query_to_authorized` is the second
//! (a `GRAPH <…>` an injection might add still only matches authorised graphs).

use std::collections::HashMap;

use super::models::{ParamSpec, ParamType};

const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

/// Error rendering or substituting parameters into a query.
#[derive(Debug)]
pub struct ParamError(pub String);

impl std::fmt::Display for ParamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Substitute every `{{name}}` placeholder in `query` with the safely-rendered
/// value for that parameter.
///
/// Values come from `provided` (e.g. query string or JSON body); a parameter
/// with no provided value falls back to its declared `default`. A placeholder
/// for an undeclared parameter, or a declared parameter with neither a value nor
/// a default, is an error (it would leave invalid SPARQL behind).
pub fn inject(
    query: &str,
    specs: &[ParamSpec],
    provided: &HashMap<String, String>,
) -> Result<String, ParamError> {
    let placeholders = find_placeholders(query);
    let by_name: HashMap<&str, &ParamSpec> = specs.iter().map(|s| (s.name.as_str(), s)).collect();

    // Reject placeholders that aren't declared parameters.
    for (_, _, name) in &placeholders {
        if !by_name.contains_key(name.as_str()) {
            return Err(ParamError(format!(
                "query references undeclared parameter '{{{{{name}}}}}'"
            )));
        }
    }

    // Pre-render each declared parameter that actually appears.
    let used: std::collections::HashSet<&str> =
        placeholders.iter().map(|(_, _, n)| n.as_str()).collect();
    let mut rendered: HashMap<&str, String> = HashMap::new();
    for spec in specs {
        if !used.contains(spec.name.as_str()) {
            continue;
        }
        let raw = provided
            .get(&spec.name)
            .map(String::as_str)
            .or(spec.default.as_deref());
        let raw = match raw {
            Some(v) => v,
            None => {
                return Err(ParamError(format!(
                    "missing value for required parameter '{}'",
                    spec.name
                )))
            }
        };
        rendered.insert(
            spec.name.as_str(),
            render_term(raw, spec.param_type, &spec.name)?,
        );
    }

    // Rebuild the query, copying everything verbatim except placeholder spans.
    let mut out = String::with_capacity(query.len());
    let mut cursor = 0usize;
    for (start, end, name) in placeholders {
        out.push_str(&query[cursor..start]);
        out.push_str(&rendered[name.as_str()]);
        cursor = end;
    }
    out.push_str(&query[cursor..]);
    Ok(out)
}

/// Validate (without values) that every `{{name}}` placeholder in `query` has a
/// matching declared parameter — used when saving a query so it is runnable as
/// an API.
pub fn declared_check(query: &str, specs: &[ParamSpec]) -> Result<(), ParamError> {
    let names: std::collections::HashSet<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    for (_, _, name) in find_placeholders(query) {
        if !names.contains(name.as_str()) {
            return Err(ParamError(format!(
                "query references undeclared parameter '{{{{{name}}}}}'"
            )));
        }
    }
    Ok(())
}

/// Scan for `{{identifier}}` placeholders (no internal whitespace, so a SPARQL
/// nested group `{{ ?s ?p ?o }}` is never mistaken for one). Returns
/// `(start, end, name)` byte spans covering the full `{{…}}`.
fn find_placeholders(query: &str) -> Vec<(usize, usize, String)> {
    let b = query.as_bytes();
    let n = b.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < n {
        if b[i] == b'{' && b[i + 1] == b'{' {
            let id_start = i + 2;
            let mut j = id_start;
            // identifier: [A-Za-z_][A-Za-z0-9_]*
            if j < n && (b[j].is_ascii_alphabetic() || b[j] == b'_') {
                j += 1;
                while j < n && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                    j += 1;
                }
                if j + 1 < n && b[j] == b'}' && b[j + 1] == b'}' {
                    let name = query[id_start..j].to_string();
                    out.push((i, j + 2, name));
                    i = j + 2;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// Render one value into a SPARQL term according to its declared type, rejecting
/// anything that could escape the term's syntax.
fn render_term(value: &str, ty: ParamType, name: &str) -> Result<String, ParamError> {
    let err = |msg: &str| ParamError(format!("parameter '{name}': {msg}"));
    match ty {
        ParamType::Iri => {
            if value.is_empty() {
                return Err(err("IRI must not be empty"));
            }
            // Characters an IRI may not contain unescaped (RFC 3987 + delimiters).
            if value.chars().any(|c| {
                c.is_control()
                    || matches!(
                        c,
                        ' ' | '<' | '>' | '"' | '{' | '}' | '|' | '^' | '`' | '\\'
                    )
            }) {
                return Err(err("IRI contains illegal characters"));
            }
            Ok(format!("<{value}>"))
        }
        ParamType::String => Ok(format!("\"{}\"", escape_literal(value))),
        ParamType::Integer => {
            let t = value.trim();
            let body = t.strip_prefix(['+', '-']).unwrap_or(t);
            if body.is_empty() || !body.bytes().all(|c| c.is_ascii_digit()) {
                return Err(err("not a valid integer"));
            }
            Ok(t.to_string())
        }
        ParamType::Decimal => {
            let t = value.trim();
            if t.is_empty()
                || !t
                    .bytes()
                    .all(|c| matches!(c, b'0'..=b'9' | b'+' | b'-' | b'.' | b'e' | b'E'))
                || t.parse::<f64>().is_err()
            {
                return Err(err("not a valid decimal"));
            }
            Ok(t.to_string())
        }
        ParamType::Boolean => match value.trim() {
            "true" => Ok("true".to_string()),
            "false" => Ok("false".to_string()),
            _ => Err(err("must be 'true' or 'false'")),
        },
        ParamType::Date => {
            if !is_xsd_date(value.trim()) {
                return Err(err("must be an xsd:date (YYYY-MM-DD)"));
            }
            Ok(format!("\"{}\"^^<{XSD}date>", value.trim()))
        }
        ParamType::DateTime => {
            if !is_xsd_datetime(value.trim()) {
                return Err(err("must be an xsd:dateTime (ISO-8601)"));
            }
            Ok(format!("\"{}\"^^<{XSD}dateTime>", value.trim()))
        }
    }
}

fn escape_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

fn is_xsd_date(s: &str) -> bool {
    let b = s.as_bytes();
    // YYYY-MM-DD (optionally a trailing timezone is allowed by xsd:date, but we
    // keep it strict: callers wanting tz should use dateTime).
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[8..10].iter().all(u8::is_ascii_digit)
}

fn is_xsd_datetime(s: &str) -> bool {
    // Light validation: starts with a date, then 'T', then digits/':'/'.'/tz.
    // Strict enough to reject quotes/spaces/control chars; the store rejects
    // genuinely malformed literals at query time.
    let Some((date, rest)) = s.split_once('T') else {
        return false;
    };
    if !is_xsd_date(date) || rest.is_empty() {
        return false;
    }
    rest.chars()
        .all(|c| c.is_ascii_digit() || matches!(c, ':' | '.' | '+' | '-' | 'Z'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str, ty: ParamType) -> ParamSpec {
        ParamSpec {
            name: name.to_string(),
            param_type: ty,
            required: true,
            default: None,
            description: None,
        }
    }

    fn vals(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn injects_iri_and_string() {
        let q = "SELECT * WHERE { {{who}} <urn:name> {{label}} }";
        let specs = [
            spec("who", ParamType::Iri),
            spec("label", ParamType::String),
        ];
        let out = inject(q, &specs, &vals(&[("who", "urn:bob"), ("label", "Bob")])).unwrap();
        assert_eq!(out, "SELECT * WHERE { <urn:bob> <urn:name> \"Bob\" }");
    }

    #[test]
    fn string_escaping_blocks_breakout() {
        let specs = [spec("x", ParamType::String)];
        // A value trying to close the literal and inject a GRAPH clause is fully
        // escaped, so the closing quote stays part of the literal body.
        let out = inject(
            "FILTER(?v = {{x}})",
            &specs,
            &vals(&[("x", "a\" } GRAPH <urn:secret> { ?s ?p ?o #")]),
        )
        .unwrap();
        assert_eq!(
            out,
            "FILTER(?v = \"a\\\" } GRAPH <urn:secret> { ?s ?p ?o #\")"
        );
        // No *unescaped* double-quote (one not preceded by a backslash) appears
        // inside the literal body, so the caller cannot break out.
        let body = &out["FILTER(?v = \"".len()..out.len() - 2]; // strip wrapper + closing ")
        let bytes = body.as_bytes();
        for (i, &c) in bytes.iter().enumerate() {
            if c == b'"' {
                assert!(
                    i > 0 && bytes[i - 1] == b'\\',
                    "unescaped quote in literal body: {out}"
                );
            }
        }
    }

    #[test]
    fn iri_rejects_illegal_chars() {
        let specs = [spec("g", ParamType::Iri)];
        let e = inject("{{g}}", &specs, &vals(&[("g", "urn:a> { ?s ?p ?o } #")]));
        assert!(e.is_err());
    }

    #[test]
    fn numeric_and_boolean_validation() {
        let specs = [
            spec("n", ParamType::Integer),
            spec("d", ParamType::Decimal),
            spec("b", ParamType::Boolean),
        ];
        assert!(inject(
            "{{n}} {{d}} {{b}}",
            &specs,
            &vals(&[("n", "42"), ("d", "3.14"), ("b", "true")])
        )
        .is_ok());
        assert!(inject(
            "{{n}} {{d}} {{b}}",
            &specs,
            &vals(&[("n", "1; DROP"), ("d", "3.14"), ("b", "true")])
        )
        .is_err());
        assert!(inject(
            "{{n}} {{d}} {{b}}",
            &specs,
            &vals(&[("n", "1"), ("d", "x"), ("b", "true")])
        )
        .is_err());
        assert!(inject(
            "{{n}} {{d}} {{b}}",
            &specs,
            &vals(&[("n", "1"), ("d", "1.0"), ("b", "yes")])
        )
        .is_err());
    }

    #[test]
    fn date_types_rendered_with_full_datatype_iri() {
        let specs = [spec("d", ParamType::Date), spec("t", ParamType::DateTime)];
        let out = inject(
            "{{d}} {{t}}",
            &specs,
            &vals(&[("d", "2026-05-26"), ("t", "2026-05-26T10:00:00Z")]),
        )
        .unwrap();
        assert!(out.contains("\"2026-05-26\"^^<http://www.w3.org/2001/XMLSchema#date>"));
        assert!(
            out.contains("\"2026-05-26T10:00:00Z\"^^<http://www.w3.org/2001/XMLSchema#dateTime>")
        );
        assert!(inject("{{d}}", &specs[..1], &vals(&[("d", "26-05-2026")])).is_err());
    }

    #[test]
    fn default_used_when_value_missing() {
        let mut s = spec("limit", ParamType::Integer);
        s.default = Some("10".into());
        let out = inject("LIMIT {{limit}}", &[s], &HashMap::new()).unwrap();
        assert_eq!(out, "LIMIT 10");
    }

    #[test]
    fn missing_required_without_default_errors() {
        let specs = [spec("x", ParamType::String)];
        assert!(inject("{{x}}", &specs, &HashMap::new()).is_err());
    }

    #[test]
    fn undeclared_placeholder_errors() {
        let specs = [spec("x", ParamType::String)];
        assert!(inject("{{x}} {{y}}", &specs, &vals(&[("x", "a")])).is_err());
    }

    #[test]
    fn nested_group_braces_are_not_placeholders() {
        // `{{ ?s ?p ?o }}` is two SPARQL groups, not a placeholder.
        let q = "SELECT * WHERE {{ ?s ?p ?o }}";
        let out = inject(q, &[], &HashMap::new()).unwrap();
        assert_eq!(out, q);
    }
}
