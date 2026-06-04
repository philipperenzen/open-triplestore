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
}


