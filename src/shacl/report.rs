use super::shapes::PropertyPath;
use oxigraph::model::Term;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Severity levels for SHACL validation results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Violation,
    Warning,
    Info,
}

impl Severity {
    /// The SHACL IRI of a built-in severity.
    pub fn iri(&self) -> &'static str {
        match self {
            Severity::Violation => "http://www.w3.org/ns/shacl#Violation",
            Severity::Warning => "http://www.w3.org/ns/shacl#Warning",
            Severity::Info => "http://www.w3.org/ns/shacl#Info",
        }
    }

    pub fn from_iri(iri: &str) -> Self {
        if iri.ends_with("Warning") {
            Severity::Warning
        } else if iri.ends_with("Info") {
            Severity::Info
        } else {
            Severity::Violation
        }
    }
}

/// A single SHACL validation result.
///
/// The string fields are display forms (a literal's lexical value, a path in
/// SPARQL syntax, `source_constraint` as e.g. `sh:minCount 1`); the UI and the
/// JSON API use them. The RDF report (`shacl_studio::report_rdf`) is written
/// from `source_constraint_component` and the typed [`ResultTerms`] instead.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ValidationResult {
    pub severity: Severity,
    pub focus_node: String,
    pub path: Option<String>,
    pub value: Option<String>,
    pub source_shape: String,
    pub source_constraint: String,
    /// IRI of the constraint component that produced the result
    /// (`sh:sourceConstraintComponent`), e.g.
    /// `http://www.w3.org/ns/shacl#MinCountConstraintComponent`. Empty on a
    /// result that no constraint produced (a gate error) and on reports
    /// stored before the field existed.
    #[serde(default)]
    pub source_constraint_component: String,
    pub message: String,
    /// The result annotations (SHACL-AF §4) a SPARQL-based constraint or
    /// validator declared with `sh:resultAnnotation`, as display strings.
    /// Omitted from the JSON when empty, which is every result but those.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub annotations: Vec<ResultAnnotationValue>,
    /// The typed terms behind the display strings. Not serialised: a report
    /// read back from JSON has none, and the RDF writer falls back to the
    /// strings.
    #[serde(skip)]
    pub terms: ResultTerms,
}

/// One result annotation value: `property` (an IRI) set to `value` (a
/// literal's lexical form, an IRI, or `_:label`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ResultAnnotationValue {
    pub property: String,
    pub value: String,
}

/// The typed RDF terms of a validation result, as the engine saw them.
#[derive(Debug, Clone, Default)]
pub struct ResultTerms {
    pub focus_node: Option<Term>,
    pub value: Option<Term>,
    pub path: Option<PropertyPath>,
    /// The shape that declared the constraint (an IRI or a shapes-graph blank node).
    pub source_shape: Option<Term>,
    /// `sh:sourceConstraint`: the `sh:sparql` node of a SPARQL-based
    /// constraint, the node expression of an expression constraint.
    pub source_constraint: Option<Term>,
    /// The declared `sh:severity` IRI when it is not one of the three built-in
    /// ones ([`Severity`] keeps only those); `None` means `severity.iri()`.
    pub severity: Option<String>,
    /// The result annotations behind [`ValidationResult::annotations`].
    pub annotations: Vec<(oxigraph::model::NamedNode, Term)>,
}

impl ResultTerms {
    /// The severity IRI to report for a result of `severity`.
    pub fn severity_iri<'a>(&'a self, severity: &Severity) -> &'a str {
        self.severity.as_deref().unwrap_or(severity.iri())
    }
}

/// SHACL validation report.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ValidationReport {
    pub conforms: bool,
    pub results: Vec<ValidationResult>,
    pub results_count: usize,
    /// What the run read and how long it took; absent on reports built
    /// outside the engine (a gate error, a stored report from before it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<RunMetrics>,
}

/// A validation run's own account of itself — carried in the report,
/// stored on the run row, summarised by the workload telemetry.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RunMetrics {
    /// Who asked: `dataset` (the validate route), `gate` (a write gate),
    /// `pipeline` (a Studio run) or `engine` (a direct call).
    pub path: String,
    pub duration_ms: u64,
    /// Quads in the data graphs, from the count index.
    pub quads: u64,
    pub graphs: u32,
    /// The data source the run read: `mirror`, `snapshot` or `live`.
    pub source: String,
    /// Whether a run index was built for it.
    pub run_index: bool,
}

/// The JSON object `fields` plus `"report": report`, exactly as
/// `serde_json::to_vec(&json!({ ..fields, "report": report }))` writes it
/// (keys sorted, as `serde_json::Value` keeps them), in pieces: the text
/// before the first result, each result as it is reached, the text after the
/// last. Each result is converted and written on its own and then dropped,
/// so a large report is never held twice more over, as a value tree and as
/// one buffer, while it goes out (issue #532).
pub struct ReportJson {
    head: Option<Vec<u8>>,
    results: std::vec::IntoIter<ValidationResult>,
    first: bool,
    tail: Option<Vec<u8>>,
}

/// Bytes a [`ReportJson`] piece collects results into before it is handed on.
const REPORT_JSON_PIECE: usize = 256 * 1024;

impl ReportJson {
    pub fn new(
        fields: serde_json::Map<String, serde_json::Value>,
        report: ValidationReport,
    ) -> serde_json::Result<Self> {
        let ValidationReport {
            conforms,
            results,
            results_count,
            metrics,
        } = report;
        // The report without its results, as the value tree would hold it.
        let shell = serde_json::to_value(ValidationReport {
            conforms,
            results: Vec::new(),
            results_count,
            metrics,
        })?;
        let mut top = fields;
        top.insert("report".to_string(), shell);
        // Written into `head` up to the opening of the results array, into
        // `tail` after its close.
        let mut parts = [Vec::new(), Vec::new()];
        let mut cur = 0;
        parts[cur].push(b'{');
        for (i, (key, value)) in top.iter().enumerate() {
            if i > 0 {
                parts[cur].push(b',');
            }
            serde_json::to_writer(&mut parts[cur], key)?;
            parts[cur].push(b':');
            let serde_json::Value::Object(report) = value else {
                serde_json::to_writer(&mut parts[cur], value)?;
                continue;
            };
            if key != "report" {
                serde_json::to_writer(&mut parts[cur], value)?;
                continue;
            }
            parts[cur].push(b'{');
            for (j, (rk, rv)) in report.iter().enumerate() {
                if j > 0 {
                    parts[cur].push(b',');
                }
                serde_json::to_writer(&mut parts[cur], rk)?;
                parts[cur].push(b':');
                if rk == "results" {
                    parts[cur].push(b'[');
                    cur = 1;
                    parts[cur].push(b']');
                } else {
                    serde_json::to_writer(&mut parts[cur], rv)?;
                }
            }
            parts[cur].push(b'}');
        }
        parts[cur].push(b'}');
        let [head, tail] = parts;
        Ok(Self {
            head: Some(head),
            results: results.into_iter(),
            first: true,
            tail: Some(tail),
        })
    }
}

impl Iterator for ReportJson {
    type Item = serde_json::Result<Vec<u8>>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(head) = self.head.take() {
            return Some(Ok(head));
        }
        let mut piece = Vec::new();
        for r in self.results.by_ref() {
            if !std::mem::take(&mut self.first) {
                piece.push(b',');
            }
            // Through a value, as the whole report was: the same key order.
            let value = match serde_json::to_value(&r) {
                Ok(v) => v,
                Err(e) => return Some(Err(e)),
            };
            if let Err(e) = serde_json::to_writer(&mut piece, &value) {
                return Some(Err(e));
            }
            if piece.len() >= REPORT_JSON_PIECE {
                return Some(Ok(piece));
            }
        }
        if !piece.is_empty() {
            return Some(Ok(piece));
        }
        self.tail.take().map(Ok)
    }
}

impl RunMetrics {
    /// Fold a second run into this one (a route validates one shapes graph
    /// at a time and merges the reports): durations add, the scope is the
    /// larger one, a source that differs becomes `mixed`.
    pub fn merge(mut self, other: RunMetrics) -> RunMetrics {
        self.duration_ms += other.duration_ms;
        self.quads = self.quads.max(other.quads);
        self.graphs = self.graphs.max(other.graphs);
        if self.source != other.source {
            self.source = "mixed".to_string();
        }
        self.run_index |= other.run_index;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(i: usize, annotated: bool) -> ValidationResult {
        ValidationResult {
            severity: if i % 2 == 0 {
                Severity::Violation
            } else {
                Severity::Warning
            },
            focus_node: format!("http://example.org/n{i}"),
            path: (i % 3 != 0).then(|| "<http://example.org/p>".to_string()),
            value: (i % 4 != 0).then(|| format!("välue \"{i}\"\n\u{1F600}")),
            source_shape: "http://example.org/S".to_string(),
            source_constraint: "sh:minCount 1".to_string(),
            source_constraint_component: "http://www.w3.org/ns/shacl#MinCountConstraintComponent"
                .to_string(),
            message: format!("message {i}"),
            annotations: if annotated {
                vec![ResultAnnotationValue {
                    property: "http://example.org/a".to_string(),
                    value: "x".to_string(),
                }]
            } else {
                Vec::new()
            },
            terms: ResultTerms::default(),
        }
    }

    fn report(n: usize, metrics: bool) -> ValidationReport {
        let results: Vec<ValidationResult> = (0..n).map(|i| result(i, i % 5 == 1)).collect();
        ValidationReport {
            conforms: results.is_empty(),
            results_count: results.len(),
            results,
            metrics: metrics.then(|| RunMetrics {
                path: "dataset".to_string(),
                duration_ms: 12,
                quads: 34,
                graphs: 2,
                source: "snapshot".to_string(),
                run_index: true,
            }),
        }
    }

    fn streamed(fields: serde_json::Value, report: ValidationReport) -> Vec<u8> {
        let serde_json::Value::Object(fields) = fields else {
            unreachable!()
        };
        ReportJson::new(fields, report)
            .unwrap()
            .map(Result::unwrap)
            .collect::<Vec<_>>()
            .concat()
    }

    /// The streamed response is byte for byte what the value tree gave, for
    /// a test run, an official run, an empty report and one large enough to
    /// take several pieces.
    #[test]
    fn report_json_writes_what_the_value_tree_wrote() {
        for (n, metrics) in [(0, false), (1, true), (7, true), (3000, false)] {
            let r = report(n, metrics);
            let expected = serde_json::to_vec(&serde_json::json!({
                "report": r.clone(),
                "run_id": null,
                "ran_at": null,
                "test": true,
                "partial": n % 2 == 1,
            }))
            .unwrap();
            let got = streamed(
                serde_json::json!({
                    "run_id": null,
                    "ran_at": null,
                    "test": true,
                    "partial": n % 2 == 1,
                }),
                r.clone(),
            );
            assert_eq!(
                String::from_utf8(got).unwrap(),
                String::from_utf8(expected).unwrap(),
                "test run, {n} results"
            );
            let expected = serde_json::to_vec(&serde_json::json!({
                "report": r.clone(),
                "run_id": "run-1",
                "ran_at": "2026-10-15T00:00:00Z",
            }))
            .unwrap();
            let got = streamed(
                serde_json::json!({ "run_id": "run-1", "ran_at": "2026-10-15T00:00:00Z" }),
                r,
            );
            assert_eq!(got, expected, "official run, {n} results");
        }
        // Large reports go out in more than one piece.
        let serde_json::Value::Object(fields) = serde_json::json!({ "test": true }) else {
            unreachable!()
        };
        let pieces = ReportJson::new(fields, report(3000, false))
            .unwrap()
            .count();
        assert!(pieces > 3, "{pieces} pieces");
    }
}
