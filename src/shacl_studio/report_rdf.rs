//! Serialise a [`ValidationReport`] to standard SHACL `sh:ValidationReport` /
//! `sh:ValidationResult` RDF (Turtle), so a pipeline can persist its results
//! back into the store (a new graph or version) rather than only as run-history
//! JSON. The shape follows the W3C SHACL results vocabulary
//! (<https://www.w3.org/TR/shacl/#results-validation-report>): typed focus
//! nodes and values, `sh:resultPath` as a SHACL path structure, the
//! constraint component IRI, `sh:sourceConstraint` for SPARQL-based and
//! expression constraints, and SHACL-AF result annotations.
//!
//! The typed terms come from [`ValidationResult::terms`]. A report without
//! them (read back from stored JSON, or built outside the engine) falls back
//! to its display strings: an IRI where the string is one, else a string
//! literal.

use crate::shacl::report::{ValidationReport, ValidationResult};
use crate::shacl::shapes::PropertyPath;
use oxigraph::io::{RdfFormat, RdfSerializer};
use oxigraph::model::{BlankNode, Literal, NamedNode, NamedOrBlankNode, Term, Triple};

const SH: &str = "http://www.w3.org/ns/shacl#";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

fn sh(local: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("{SH}{local}"))
}

fn rdf(local: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("{RDF}{local}"))
}

/// A display string back as a term: an absolute IRI (angle-wrapped or not)
/// becomes a named node, anything else a plain string literal. Composite
/// path renderings (`^<p>`, `<a>/<b>`) are not IRIs and stay literals.
fn term_from_display(s: &str) -> Term {
    let t = s.trim();
    let inner = t
        .strip_prefix('<')
        .and_then(|x| x.strip_suffix('>'))
        .unwrap_or(t);
    let looks_absolute = inner.contains("://") || inner.starts_with("urn:");
    match NamedNode::new(inner) {
        Ok(nn) if looks_absolute => nn.into(),
        _ => Literal::new_simple_literal(t).into(),
    }
}

/// Builds the report's triples.
struct Writer {
    triples: Vec<Triple>,
}

impl Writer {
    fn add(&mut self, s: impl Into<NamedOrBlankNode>, p: NamedNode, o: impl Into<Term>) {
        self.triples.push(Triple::new(s, p, o));
    }

    /// An RDF collection of `items`; returns its head.
    fn list(&mut self, items: Vec<Term>) -> Term {
        let mut head: Term = rdf("nil").into();
        for item in items.into_iter().rev() {
            let cell = BlankNode::default();
            self.add(cell.clone(), rdf("first"), item);
            self.add(cell.clone(), rdf("rest"), head);
            head = cell.into();
        }
        head
    }

    /// A property path as the SHACL path structure it was read from (§2.3.1).
    fn path(&mut self, path: &PropertyPath) -> Term {
        let wrap = |w: &mut Writer, predicate: &str, inner: Term| -> Term {
            let node = BlankNode::default();
            w.add(node.clone(), sh(predicate), inner);
            node.into()
        };
        match path {
            PropertyPath::Predicate(iri) => NamedNode::new_unchecked(iri.as_str()).into(),
            PropertyPath::Sequence(steps) => {
                let items = steps.iter().map(|p| self.path(p)).collect();
                self.list(items)
            }
            PropertyPath::Alternative(options) => {
                let items = options.iter().map(|p| self.path(p)).collect();
                let list = self.list(items);
                wrap(self, "alternativePath", list)
            }
            PropertyPath::Inverse(p) => {
                let inner = self.path(p);
                wrap(self, "inversePath", inner)
            }
            PropertyPath::ZeroOrMore(p) => {
                let inner = self.path(p);
                wrap(self, "zeroOrMorePath", inner)
            }
            PropertyPath::OneOrMore(p) => {
                let inner = self.path(p);
                wrap(self, "oneOrMorePath", inner)
            }
            PropertyPath::ZeroOrOne(p) => {
                let inner = self.path(p);
                wrap(self, "zeroOrOnePath", inner)
            }
        }
    }

    fn result(&mut self, report: &NamedNode, r: &ValidationResult) {
        let node = BlankNode::default();
        self.add(report.clone(), sh("result"), node.clone());
        self.add(node.clone(), rdf("type"), sh("ValidationResult"));
        let severity = r.terms.severity_iri(&r.severity);
        let severity = NamedNode::new(severity).unwrap_or_else(|_| sh("Violation"));
        self.add(node.clone(), sh("resultSeverity"), severity);

        let focus = r
            .terms
            .focus_node
            .clone()
            .or_else(|| (!r.focus_node.is_empty()).then(|| term_from_display(&r.focus_node)));
        if let Some(focus) = focus {
            self.add(node.clone(), sh("focusNode"), focus);
        }
        let path = match &r.terms.path {
            Some(p) => Some(self.path(p)),
            None => r
                .path
                .as_deref()
                .filter(|p| !p.is_empty())
                .map(term_from_display),
        };
        if let Some(path) = path {
            self.add(node.clone(), sh("resultPath"), path);
        }
        let value = r.terms.value.clone().or_else(|| {
            r.value
                .as_deref()
                .filter(|v| !v.is_empty())
                .map(term_from_display)
        });
        if let Some(value) = value {
            self.add(node.clone(), sh("value"), value);
        }
        let shape =
            r.terms.source_shape.clone().or_else(|| {
                (!r.source_shape.is_empty()).then(|| term_from_display(&r.source_shape))
            });
        if let Some(shape) = shape {
            self.add(node.clone(), sh("sourceShape"), shape);
        }
        // The component IRI; a stored report from before the field existed
        // may still carry an IRI in the display string (a SHACL-AF component).
        let component = [r.source_constraint_component.as_str(), &r.source_constraint]
            .into_iter()
            .find_map(|c| match term_from_display(c) {
                Term::NamedNode(nn) => Some(nn),
                _ => None,
            });
        if let Some(component) = component {
            self.add(node.clone(), sh("sourceConstraintComponent"), component);
        }
        if let Some(constraint) = &r.terms.source_constraint {
            self.add(node.clone(), sh("sourceConstraint"), constraint.clone());
        }
        // SHACL-AF §4 result annotations: the typed terms, else (a report
        // read back from JSON) the display strings.
        if r.terms.annotations.is_empty() {
            for a in &r.annotations {
                if let Ok(property) = NamedNode::new(&a.property) {
                    self.add(node.clone(), property, term_from_display(&a.value));
                }
            }
        } else {
            for (property, value) in &r.terms.annotations {
                self.add(node.clone(), property.clone(), value.clone());
            }
        }
        if !r.message.is_empty() {
            self.add(
                node,
                sh("resultMessage"),
                Literal::new_simple_literal(&r.message),
            );
        }
    }
}

/// Render `report` as Turtle. `report_iri` is the IRI minted for the
/// `sh:ValidationReport` node (e.g. `urn:system:reports:{pipeline}#run-{ts}`);
/// each result is a fresh blank node linked via `sh:result`.
pub fn report_to_turtle(report: &ValidationReport, report_iri: &str) -> String {
    let report_node = NamedNode::new_unchecked(report_iri);
    let mut w = Writer {
        triples: Vec::new(),
    };
    w.add(report_node.clone(), rdf("type"), sh("ValidationReport"));
    w.add(
        report_node.clone(),
        sh("conforms"),
        Literal::from(report.conforms),
    );
    for r in &report.results {
        w.result(&report_node, r);
    }

    let serialize = || -> std::io::Result<Vec<u8>> {
        let mut out = RdfSerializer::from_format(RdfFormat::Turtle)
            .with_prefix("sh", SH)
            .and_then(|s| s.with_prefix("xsd", "http://www.w3.org/2001/XMLSchema#"))
            .map_err(std::io::Error::other)?
            .for_writer(Vec::new());
        for t in &w.triples {
            out.serialize_triple(t)?;
        }
        out.finish()
    };
    match serialize() {
        Ok(bytes) => String::from_utf8(bytes).unwrap_or_default(),
        Err(e) => {
            tracing::warn!("validation report could not be serialised: {e}");
            String::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shacl::report::{ResultTerms, Severity, ValidationResult};
    use crate::store::TripleStore;

    /// Load `ttl` into `graph` of a fresh store and answer `ask` over it.
    fn ask(ttl: &str, graph: &str, ask: &str) -> bool {
        let store = TripleStore::in_memory().unwrap();
        store
            .graph_store_put(Some(graph), ttl, oxigraph::io::RdfFormat::Turtle)
            .unwrap_or_else(|e| panic!("report must be valid Turtle: {e}\n{ttl}"));
        let q = format!(
            "PREFIX sh: <http://www.w3.org/ns/shacl#> \
             PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> \
             PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> \
             PREFIX ex: <http://example.org/> \
             ASK {{ GRAPH <{graph}> {{ {ask} }} }}"
        );
        match store.query(&q).unwrap() {
            oxigraph::sparql::QueryResults::Boolean(b) => b,
            _ => panic!("expected a boolean"),
        }
    }

    fn sample() -> ValidationReport {
        ValidationReport {
            conforms: false,
            results: vec![ValidationResult {
                severity: Severity::Violation,
                focus_node: "http://example.org/alice".into(),
                path: Some("http://example.org/age".into()),
                value: Some("15".into()),
                source_shape: "http://example.org/PersonShape".into(),
                source_constraint: "sh:minInclusive 18".into(),
                source_constraint_component:
                    "http://www.w3.org/ns/shacl#MinInclusiveConstraintComponent".into(),
                message: "Must be at least 18.".into(),
                annotations: Vec::new(),
                terms: Default::default(),
            }],
            results_count: 1,
            metrics: None,
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

    /// Without typed terms (a report read back from JSON) the display strings
    /// are used: a plain or angle-wrapped IRI becomes an IRI, a composite SPARQL
    /// path rendering a string literal — never an invalid `<<iri>>`.
    #[test]
    fn display_string_fallback_paths() {
        let mut r = sample();
        r.results[0].path = Some("<http://example.org/age>".into());
        let ttl = report_to_turtle(&r, "urn:system:reports:test#run-3");
        assert!(ask(
            &ttl,
            "urn:test:wrapped",
            "?res sh:resultPath ex:age ; sh:value \"15\" ; \
             sh:sourceConstraintComponent sh:MinInclusiveConstraintComponent",
        ));

        let mut r2 = sample();
        r2.results[0].path = Some("^<http://example.org/parent>".into());
        let ttl2 = report_to_turtle(&r2, "urn:system:reports:test#run-4");
        assert!(ask(
            &ttl2,
            "urn:test:composite",
            "?res sh:resultPath \"^<http://example.org/parent>\"",
        ));
    }

    /// A stored report from before `source_constraint_component` existed still
    /// names a SHACL-AF component whose IRI was its display string, and does
    /// not turn a display label like `sh:minCount 1` into a component.
    #[test]
    fn legacy_component_fallback() {
        let mut r = sample();
        r.results[0].source_constraint_component = String::new();
        r.results[0].source_constraint = "http://example.org/MyComponent".into();
        let ttl = report_to_turtle(&r, "urn:system:reports:test#run-6");
        assert!(ask(
            &ttl,
            "urn:test:legacy",
            "?res sh:sourceConstraintComponent ex:MyComponent",
        ));
        r.results[0].source_constraint = "sh:minCount 1".into();
        let ttl = report_to_turtle(&r, "urn:system:reports:test#run-7");
        assert!(!ask(
            &ttl,
            "urn:test:legacy2",
            "?res sh:sourceConstraintComponent ?c",
        ));
    }

    /// Typed terms are written as the W3C report has them: literals keep
    /// datatype and language, paths become SHACL path structures, the
    /// component and the `sh:sparql` node are IRIs, a custom severity is kept.
    #[test]
    fn typed_terms_are_written() {
        use oxigraph::model::{Literal, NamedNode, Term};
        let ex = |l: &str| format!("http://example.org/{l}");
        let nn = |l: &str| Term::NamedNode(NamedNode::new_unchecked(ex(l)));
        let mut r = sample();
        r.results[0].source_constraint_component =
            "http://www.w3.org/ns/shacl#SPARQLConstraintComponent".into();
        r.results[0].terms = ResultTerms {
            focus_node: Some(nn("alice")),
            value: Some(Term::Literal(Literal::new_typed_literal(
                "15",
                NamedNode::new_unchecked("http://www.w3.org/2001/XMLSchema#integer"),
            ))),
            // ^ex:parent / (ex:a | ex:b)*
            path: Some(PropertyPath::Sequence(vec![
                PropertyPath::Inverse(Box::new(PropertyPath::Predicate(ex("parent")))),
                PropertyPath::ZeroOrMore(Box::new(PropertyPath::Alternative(vec![
                    PropertyPath::Predicate(ex("a")),
                    PropertyPath::Predicate(ex("b")),
                ]))),
            ])),
            source_shape: Some(nn("PersonShape")),
            source_constraint: Some(nn("PersonShape-sparql")),
            severity: Some(ex("MySeverity")),
            annotations: Vec::new(),
        };
        let mut second = r.results[0].clone();
        second.terms.value = Some(Term::Literal(
            Literal::new_language_tagged_literal("Alice", "en").unwrap(),
        ));
        second.terms.path = Some(PropertyPath::Predicate(ex("name")));
        second.terms.source_constraint = None;
        second.terms.severity = None;
        second.severity = Severity::Warning;
        r.results.push(second);
        let ttl = report_to_turtle(&r, "urn:system:reports:test#run-8");
        let g = "urn:test:typed";
        assert!(ask(
            &ttl,
            g,
            "?rep a sh:ValidationReport ; sh:conforms false ; sh:result ?res . \
             ?res a sh:ValidationResult ; sh:focusNode ex:alice ; \
               sh:value 15 ; sh:resultSeverity ex:MySeverity ; \
               sh:sourceShape ex:PersonShape ; sh:sourceConstraint ex:PersonShape-sparql ; \
               sh:sourceConstraintComponent sh:SPARQLConstraintComponent ; \
               sh:resultPath ?seq . \
             ?seq rdf:first [ sh:inversePath ex:parent ] ; rdf:rest ?rest . \
             ?rest rdf:first [ sh:zeroOrMorePath [ sh:alternativePath ?alt ] ] ; rdf:rest rdf:nil . \
             ?alt rdf:first ex:a ; rdf:rest/rdf:first ex:b ; rdf:rest/rdf:rest rdf:nil .",
        ));
        assert!(ask(
            &ttl,
            g,
            "?res sh:value \"Alice\"@en ; sh:resultPath ex:name ; sh:resultSeverity sh:Warning . \
             FILTER NOT EXISTS { ?res sh:sourceConstraint ?c }",
        ));
        // Exactly one value per result: no display-string duplicate.
        assert!(!ask(&ttl, g, "?res sh:value \"15\""));
    }

    /// Result annotations (SHACL-AF §4) are written as properties of the
    /// result node, typed; a report without typed terms (read back from JSON)
    /// writes the display strings.
    #[test]
    fn result_annotations_are_written() {
        use crate::shacl::report::ResultAnnotationValue;
        use oxigraph::model::{Literal, NamedNode, Term};
        let time = NamedNode::new_unchecked("http://example.org/time");
        let note = NamedNode::new_unchecked("http://example.org/note");
        let mut r = sample();
        r.results[0].terms.annotations = vec![
            (
                time.clone(),
                Term::Literal(Literal::new_typed_literal(
                    "2015-03-27T10:58:00",
                    NamedNode::new_unchecked("http://www.w3.org/2001/XMLSchema#dateTime"),
                )),
            ),
            (note.clone(), Term::NamedNode(note.clone())),
            (note.clone(), Literal::new_simple_literal("second").into()),
        ];
        let ttl = report_to_turtle(&r, "urn:system:reports:test#run-10");
        assert!(ask(
            &ttl,
            "urn:test:ann",
            "?res a sh:ValidationResult ; ex:time \"2015-03-27T10:58:00\"^^xsd:dateTime ; \
             ex:note ex:note , \"second\"",
        ));

        let mut r = sample();
        r.results[0].annotations = vec![
            ResultAnnotationValue {
                property: "http://example.org/note".into(),
                value: "http://example.org/thing".into(),
            },
            ResultAnnotationValue {
                property: "not an IRI".into(),
                value: "dropped".into(),
            },
        ];
        let ttl = report_to_turtle(&r, "urn:system:reports:test#run-11");
        assert!(ask(&ttl, "urn:test:ann2", "?res ex:note ex:thing"));
        assert!(!ask(&ttl, "urn:test:ann3", "?res ?p \"dropped\""));
    }

    /// A message with quotes, backslashes and newlines stays valid Turtle.
    #[test]
    fn message_escaping() {
        let mut r = sample();
        r.results[0].message = "line \"one\"\nline \\two".into();
        let ttl = report_to_turtle(&r, "urn:system:reports:test#run-9");
        assert!(ask(
            &ttl,
            "urn:test:msg",
            "?res sh:resultMessage \"line \\\"one\\\"\\nline \\\\two\"",
        ));
    }

    /// A conforming (empty) report serialises to a single self-closed node.
    #[test]
    fn empty_report_is_valid() {
        let r = ValidationReport {
            conforms: true,
            results: vec![],
            results_count: 0,
            metrics: None,
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
