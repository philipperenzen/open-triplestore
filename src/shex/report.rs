//! ShEx validation report types.

use oxigraph::model::Term;
use serde::Serialize;

/// A ShEx validation report: one result per node/shape pair of the shape map.
#[derive(Debug, Clone, Serialize)]
pub struct ShExReport {
    /// Whether every node conforms to its shape.
    pub conforms: bool,
    /// One result per shape map association, in shape map order.
    pub results: Vec<ShExResult>,
}

impl ShExReport {
    pub fn new(results: Vec<ShExResult>) -> Self {
        ShExReport {
            conforms: results
                .iter()
                .all(|r| matches!(r.status, ShExStatus::Conformant)),
            results,
        }
    }
}

/// A single ShEx validation result for one focus node / shape pair.
#[derive(Debug, Clone, Serialize)]
pub struct ShExResult {
    /// The focus node: an IRI as is, a blank node or literal in N-Triples form.
    pub focus_node: String,
    /// The shape label (`_:x` for a blank node label), or `START`.
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

/// How a report names a node.
pub fn node_display(t: &Term) -> String {
    match t {
        Term::NamedNode(n) => n.as_str().to_string(),
        other => other.to_string(),
    }
}
