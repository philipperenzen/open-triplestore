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
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ValidationResult {
    pub severity: Severity,
    pub focus_node: String,
    pub path: Option<String>,
    pub value: Option<String>,
    pub source_shape: String,
    pub source_constraint: String,
    pub message: String,
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
