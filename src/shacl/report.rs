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
