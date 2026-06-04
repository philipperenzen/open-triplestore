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
/// we must emit as a string literal)?
fn is_iri(s: &str) -> bool {
    let s = s.trim();
    !s.is_empty()
        && !s.starts_with("_:")
        && s.contains(':')
        && !s.contains(' ')
        && !s.contains('"')
        && (s.starts_with("http://")
            || s.starts_with("https://")
            || s.starts_with("urn:")
            || s.contains("://"))
}

/// Emit `term` as an IRI reference when it looks like one, else as a quoted
/// literal. SHACL `sh:focusNode`/`sh:value` may legitimately be either.
fn term(s: &str) -> String {
    if is_iri(s) {
        format!("<{}>", s.trim())
    } else {
        format!("\"{}\"", esc(s))
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
        // Separate results with a comma; close the report on the last one.
        if i + 1 < n {
            t.push_str("    ] ,\n");
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
