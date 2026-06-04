//! ShEx validation report types.

use serde::Serialize;

/// A ShEx validation report containing results for all evaluated focus nodes.
#[derive(Debug, Clone, Serialize)]
pub struct ShExReport {
    /// Whether all focus nodes conform to their assigned shapes.
    pub conforms: bool,
    /// Individual validation results.
    pub results: Vec<ShExResult>,
}

/// A single ShEx validation result for one focus node / shape pair.
#[derive(Debug, Clone, Serialize)]
pub struct ShExResult {
    /// The focus node IRI that was validated.
    pub focus_node: String,
    /// The shape IRI it was validated against.
    pub shape: String,
    /// Whether it conforms and why not (if applicable).
    pub status: ShExStatus,
}

/// Conformance status for a single validation check.
#[derive(Debug, Clone, Serialize)]
pub enum ShExStatus {
    /// The focus node conforms to the shape.
    Conformant,
    /// The focus node does not conform; includes the reason.
    NonConformant(String),
}
