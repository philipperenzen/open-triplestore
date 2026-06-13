//! Serialise a [`ValidationReport`] to standard SHACL `sh:ValidationReport` /
//! `sh:ValidationResult` RDF (Turtle), so a pipeline can persist its results
//! back into the store (a new graph or version) rather than only as run-history
//! JSON. The shape mirrors the W3C SHACL results vocabulary
//! (<https://www.w3.org/TR/shacl/#results-validation-report>).

use crate::shacl::report::{Severity, ValidationReport};

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// Is `s` a usable absolute IRI we can emit as `<s>` (vs. a plain/bnode label
/// we must emit as a string literal)? `<` / `>` are forbidden inside IRI refs,
/// so composite path expressions (`^<p>`, `<a>/<b>`) are rejected here and
/// fall back to a string literal.
fn is_iri(s: &str) -> bool {
    let s = s.trim();
    !s.is_empty()
        && !s.starts_with("_:")
        && s.contains(':')
        && !s.contains(' ')
        && !s.contains('"')
        && !s.contains('<')
        && !s.contains('>')
        && (s.starts_with("http://")
            || s.starts_with("https://")
            || s.starts_with("urn:")
            || s.contains("://"))
}

/// Emit `term` as an IRI reference when it looks like one, else as a quoted
/// literal. SHACL `sh:focusNode`/`sh:value` may legitimately be either.
/// `sh:resultPath` strings arrive in the engine's SPARQL path rendering, where
/// a plain predicate is already angle-wrapped (`<iri>`) — unwrap before
/// deciding, so we never emit a double-wrapped (invalid) `<<iri>>`.
fn term(s: &str) -> String {
    let t = s.trim();
    let inner = t
        .strip_prefix('<')
        .and_then(|x| x.strip_suffix('>'))
        .unwrap_or(t);
    if is_iri(inner) {
        format!("<{}>", inner)
    } else {
        format!("\"{}\"", esc(t))
    }
}

fn severity_iri(sev: &Severity) -> &'static str {
    match sev {
        Severity::Violation => "sh:Violation",
        Severity::Warning => "sh:Warning",
        Severity::Info => "sh:Info",
    }
}

/// Render `report` as Turtle. `report_iri` is the IRI minted for the
/// `sh:ValidationReport` node (e.g. `urn:system:reports:{pipeline}#run-{ts}`);
/// each result is a blank node linked via `sh:result`.
pub fn report_to_turtle(report: &ValidationReport, report_iri: &str) -> String {
    let mut t = String::new();
    t.push_str("@prefix sh: <http://www.w3.org/ns/shacl#> .\n");
    t.push_str("@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\n");

    t.push_str(&format!("<{report_iri}> a sh:ValidationReport ;\n"));
    t.push_str(&format!(
        "    sh:conforms \"{}\"^^xsd:boolean",
        if report.conforms { "true" } else { "false" }
    ));
    if report.results.is_empty() {
        t.push_str(" .\n");
        return t;
    }
    t.push_str(" ;\n");

    let n = report.results.len();
    for (i, r) in report.results.iter().enumerate() {
        // One inline blank node per result, chained off the report.
        t.push_str("    sh:result [\n");
        t.push_str("        a sh:ValidationResult ;\n");
        t.push_str(&format!(
            "        sh:resultSeverity {} ;\n",
            severity_iri(&r.severity)
        ));
        t.push_str(&format!("        sh:focusNode {} ;\n", term(&r.focus_node)));
        if let Some(p) = &r.path {
            if !p.is_empty() {
                t.push_str(&format!("        sh:resultPath {} ;\n", term(p)));
            }
        }
        if let Some(v) = &r.value {
            if !v.is_empty() {
                t.push_str(&format!("        sh:value {} ;\n", term(v)));
            }
        }
        if !r.source_shape.is_empty() {
            t.push_str(&format!(
                "        sh:sourceShape {} ;\n",
                term(&r.source_shape)
            ));
        }
        if !r.source_constraint.is_empty() {
            t.push_str(&format!(
                "        sh:sourceConstraintComponent {} ;\n",
                term(&r.source_constraint)
            ));
        }
        t.push_str(&format!(
            "        sh:resultMessage \"{}\"\n",
            esc(&r.message)
        ));
        // Each result re-states the `sh:result` predicate, so chain with a
        // semicolon (a comma would demand a bare object next and is invalid
        // Turtle once `sh:result` is written again); close on the last one.
        if i + 1 < n {
            t.push_str("    ] ;\n");
        } else {
            t.push_str("    ] .\n");
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shacl::report::ValidationResult;
    use crate::store::TripleStore;

    fn sample() -> ValidationReport {
        ValidationReport {
            conforms: false,
            results: vec![ValidationResult {
                severity: Severity::Violation,
                focus_node: "http://example.org/alice".into(),
                path: Some("http://example.org/age".into()),
                value: Some("15".into()),
                source_shape: "http://example.org/PersonShape".into(),
                source_constraint: "sh:MinInclusiveConstraintComponent".into(),
                message: "Must be at least 18.".into(),
            }],
            results_count: 1,
        }
    }

    /// The serialised report must parse back into the store as valid Turtle.
    #[test]
    fn report_turtle_round_trips() {
        let ttl = report_to_turtle(&sample(), "urn:system:reports:test#run-1");
        let store = TripleStore::in_memory().unwrap();
        store
            .graph_store_put(
                Some("urn:test:report"),
                &ttl,
                oxigraph::io::RdfFormat::Turtle,
            )
            .expect("serialised report must be valid Turtle");
        // The report node + one result must be present.
        assert!(store.count_graph(Some("urn:test:report")).unwrap() >= 6);
    }

    /// Multi-result reports must stay valid Turtle: each result re-states the
    /// `sh:result` predicate, so the blank nodes chain with `;` (a `,` here
    /// used to make every multi-violation report unparseable — and silently
    /// unpersisted).
    #[test]
    fn multi_result_report_round_trips() {
        let mut r = sample();
        let mut second = r.results[0].clone();
        second.focus_node = "http://example.org/bob".into();
        second.path = Some("<http://example.org/name>".into());
        second.message = "Expected at least 1 values, found 0.".into();
        r.results.push(second);
        r.results_count = 2;
        let ttl = report_to_turtle(&r, "urn:system:reports:test#run-5");
        let store = TripleStore::in_memory().unwrap();
        store
            .graph_store_put(
                Some("urn:test:multi"),
                &ttl,
                oxigraph::io::RdfFormat::Turtle,
            )
            .expect("multi-result report must be valid Turtle");
        // Both results present and linked from the report node.
        let q = "PREFIX sh: <http://www.w3.org/ns/shacl#> \
                 SELECT (COUNT(?res) AS ?n) WHERE { \
                   GRAPH <urn:test:multi> { ?r a sh:ValidationReport ; sh:result ?res } }";
        match store.query(q).unwrap() {
            oxigraph::sparql::QueryResults::Solutions(mut s) => {
                let row = s.next().unwrap().unwrap();
                assert_eq!(
                    row.get("n").unwrap().to_string(),
                    "\"2\"^^<http://www.w3.org/2001/XMLSchema#integer>"
                );
            }
            _ => panic!("expected solutions"),
        }
    }

    /// Engine-rendered paths arrive angle-wrapped (`<iri>`) for plain
    /// predicates and as SPARQL path expressions for composites — both must
    /// serialise to valid Turtle (single-wrapped IRI / string literal).
    #[test]
    fn wrapped_and_composite_paths_round_trip() {
        let mut r = sample();
        r.results[0].path = Some("<http://example.org/age>".into());
        let ttl = report_to_turtle(&r, "urn:system:reports:test#run-3");
        assert!(ttl.contains("sh:resultPath <http://example.org/age>"));
        let store = TripleStore::in_memory().unwrap();
        store
            .graph_store_put(
                Some("urn:test:wrapped"),
                &ttl,
                oxigraph::io::RdfFormat::Turtle,
            )
            .expect("wrapped-path report must be valid Turtle");

        let mut r2 = sample();
        r2.results[0].path = Some("^<http://example.org/parent>".into());
        let ttl2 = report_to_turtle(&r2, "urn:system:reports:test#run-4");
        assert!(ttl2.contains("sh:resultPath \"^<http://example.org/parent>\""));
        store
            .graph_store_put(
                Some("urn:test:composite"),
                &ttl2,
                oxigraph::io::RdfFormat::Turtle,
            )
            .expect("composite-path report must be valid Turtle");
    }

    /// A conforming (empty) report serialises to a single self-closed node.
    #[test]
    fn empty_report_is_valid() {
        let r = ValidationReport {
            conforms: true,
            results: vec![],
            results_count: 0,
        };
        let ttl = report_to_turtle(&r, "urn:system:reports:test#run-2");
        let store = TripleStore::in_memory().unwrap();
        store
            .graph_store_put(
                Some("urn:test:empty"),
                &ttl,
                oxigraph::io::RdfFormat::Turtle,
            )
            .expect("empty report must be valid Turtle");
        assert!(ttl.contains("sh:conforms"));
    }
}
