use serde::{Deserialize, Serialize};

/// A SHACL shape (either node shape or property shape).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shape {
    pub iri: String,
    pub shape_type: ShapeType,
    pub targets: Vec<Target>,
    pub constraints: Vec<Constraint>,
    pub property_shapes: Vec<PropertyShape>,
    pub severity: Option<String>,
    pub message: Option<String>,
    pub deactivated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeType {
    NodeShape,
    PropertyShape,
}

/// Target declarations for a shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::enum_variant_names)]
pub enum Target {
    TargetClass(String),
    TargetNode(String),
    TargetSubjectsOf(String),
    TargetObjectsOf(String),
    SparqlTarget(String), // SHACL-AF: custom SPARQL target
}

/// A property shape with path and constraints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PropertyShape {
    pub iri: Option<String>,
    pub path: PropertyPath,
    pub constraints: Vec<Constraint>,
    pub name: Option<String>,
    pub description: Option<String>,
}

/// SHACL property paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PropertyPath {
    Predicate(String),
    Inverse(Box<PropertyPath>),
    Sequence(Vec<PropertyPath>),
    Alternative(Vec<PropertyPath>),
    ZeroOrMore(Box<PropertyPath>),
    OneOrMore(Box<PropertyPath>),
    ZeroOrOne(Box<PropertyPath>),
}

impl PropertyPath {
    /// Convert to a SPARQL property path expression.
    pub fn to_sparql(&self) -> String {
        match self {
            PropertyPath::Predicate(iri) => format!("<{}>", iri),
            PropertyPath::Inverse(inner) => format!("^({})", inner.to_sparql()),
            PropertyPath::Sequence(paths) => paths
                .iter()
                .map(|p| p.to_sparql())
                .collect::<Vec<_>>()
                .join("/"),
            PropertyPath::Alternative(paths) => paths
                .iter()
                .map(|p| p.to_sparql())
                .collect::<Vec<_>>()
                .join("|"),
            PropertyPath::ZeroOrMore(inner) => format!("({})*", inner.to_sparql()),
            PropertyPath::OneOrMore(inner) => format!("({})+", inner.to_sparql()),
            PropertyPath::ZeroOrOne(inner) => format!("({})?", inner.to_sparql()),
        }
    }
}

/// SHACL constraint components.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::enum_variant_names)]
pub enum Constraint {
    // Value type constraints
    Class(String),
    Datatype(String),
    NodeKind(NodeKind),

    // Cardinality constraints
    MinCount(usize),
    MaxCount(usize),

    // Value range constraints
    MinExclusive(String),
    MinInclusive(String),
    MaxExclusive(String),
    MaxInclusive(String),

    // String-based constraints
    MinLength(usize),
    MaxLength(usize),
    Pattern {
        pattern: String,
        flags: Option<String>,
    },
    LanguageIn(Vec<String>),
    UniqueLang(bool),

    // Property pair constraints
    Equals(String),
    Disjoint(String),
    LessThan(String),
    LessThanOrEquals(String),

    // Logical constraints
    Not(Box<Shape>),
    And(Vec<Shape>),
    Or(Vec<Shape>),
    Xone(Vec<Shape>),

    // Shape-based constraints
    Node(String), // Reference to another shape by IRI
    QualifiedValueShape {
        shape_iri: String,
        min_count: Option<usize>,
        max_count: Option<usize>,
    },

    // Other constraints
    Closed {
        ignored_properties: Vec<String>,
    },
    HasValue(String),
    In(Vec<String>),

    // SHACL-AF: SPARQL-based constraint. `severity` is the optional sh:severity declared
    // on the sh:SPARQLConstraint node itself (e.g. sh:Warning), overriding the shape's.
    SparqlConstraint {
        select: String,
        message: Option<String>,
        severity: Option<String>,
    },
}

/// sh:nodeKind values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[allow(clippy::upper_case_acronyms)]
pub enum NodeKind {
    BlankNode,
    IRI,
    Literal,
    BlankNodeOrIRI,
    BlankNodeOrLiteral,
    IRIOrLiteral,
}

impl NodeKind {
    pub fn from_iri(iri: &str) -> Option<Self> {
        // Match the composite kinds first: their suffixes ("…OrIRI", "…OrLiteral")
        // end in "IRI"/"Literal", so the single-kind arms would otherwise shadow
        // them (e.g. "BlankNodeOrIRI".ends_with("IRI") is true).
        match iri {
            s if s.ends_with("BlankNodeOrIRI") => Some(NodeKind::BlankNodeOrIRI),
            s if s.ends_with("BlankNodeOrLiteral") => Some(NodeKind::BlankNodeOrLiteral),
            s if s.ends_with("IRIOrLiteral") => Some(NodeKind::IRIOrLiteral),
            s if s.ends_with("BlankNode") => Some(NodeKind::BlankNode),
            s if s.ends_with("IRI") => Some(NodeKind::IRI),
            s if s.ends_with("Literal") => Some(NodeKind::Literal),
            _ => None,
        }
    }
}
