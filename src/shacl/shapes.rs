use oxigraph::model::Term;

/// A SHACL shape (either node shape or property shape).
///
/// Internal model only — focus/value nodes and constraint constants are typed
/// [`Term`]s end-to-end; conversion to display strings happens at
/// report-building time (see `constraints::display_term`).
#[derive(Debug, Clone)]
pub struct Shape {
    pub iri: String,
    /// Informational (sh:NodeShape vs own-path property shape); not consulted
    /// during evaluation.
    #[allow(dead_code)]
    pub shape_type: ShapeType,
    pub targets: Vec<Target>,
    pub constraints: Vec<Constraint>,
    pub property_shapes: Vec<PropertyShape>,
    pub severity: Option<String>,
    pub message: Option<String>,
    pub deactivated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShapeType {
    NodeShape,
    PropertyShape,
}

/// Target declarations for a shape.
#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)]
pub enum Target {
    TargetClass(String),
    /// The target node term as written in the shapes graph — may be an IRI,
    /// a literal (`sh:targetNode 42`) or a blank node.
    TargetNode(Term),
    TargetSubjectsOf(String),
    TargetObjectsOf(String),
    SparqlTarget(String), // SHACL-AF: custom SPARQL target
}

/// A property shape with path and constraints.
#[derive(Debug, Clone)]
pub struct PropertyShape {
    pub iri: Option<String>,
    pub path: PropertyPath,
    pub constraints: Vec<Constraint>,
    /// Informational (`sh:name`/`sh:description`); not consulted during evaluation.
    #[allow(dead_code)]
    pub name: Option<String>,
    #[allow(dead_code)]
    pub description: Option<String>,
    /// `sh:severity` on the property shape itself — overrides the parent
    /// shape's severity for results produced by this property shape.
    pub severity: Option<String>,
    /// `sh:message` on the property shape itself — overrides the engine's
    /// default result message for results produced by this property shape.
    pub message: Option<String>,
}

/// SHACL property paths.
#[derive(Debug, Clone)]
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
    /// Convert to a SPARQL property path expression. Composite sub-paths are
    /// parenthesised so operator precedence is preserved — e.g. a sequence over an
    /// alternative renders as `<a>/(<b>|<c>)`, not the mis-parsed `<a>/<b>|<c>`.
    pub fn to_sparql(&self) -> String {
        match self {
            PropertyPath::Predicate(iri) => format!("<{}>", iri),
            PropertyPath::Inverse(inner) => format!("^{}", inner.to_sparql_atom()),
            PropertyPath::Sequence(paths) => paths
                .iter()
                .map(|p| p.to_sparql_atom())
                .collect::<Vec<_>>()
                .join("/"),
            PropertyPath::Alternative(paths) => paths
                .iter()
                .map(|p| p.to_sparql_atom())
                .collect::<Vec<_>>()
                .join("|"),
            PropertyPath::ZeroOrMore(inner) => format!("{}*", inner.to_sparql_atom()),
            PropertyPath::OneOrMore(inner) => format!("{}+", inner.to_sparql_atom()),
            PropertyPath::ZeroOrOne(inner) => format!("{}?", inner.to_sparql_atom()),
        }
    }

    /// SPARQL rendering of this path when used as a sub-path of another: a bare
    /// predicate stays atomic; anything composite is wrapped in parentheses.
    fn to_sparql_atom(&self) -> String {
        match self {
            PropertyPath::Predicate(iri) => format!("<{}>", iri),
            other => format!("({})", other.to_sparql()),
        }
    }
}

/// SHACL constraint components.
#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)]
pub enum Constraint {
    // Value type constraints
    Class(String),
    Datatype(String),
    NodeKind(NodeKind),

    // Cardinality constraints
    MinCount(usize),
    MaxCount(usize),

    // Value range constraints (the bound is the typed literal from the shapes graph)
    MinExclusive(Term),
    MinInclusive(Term),
    MaxExclusive(Term),
    MaxInclusive(Term),

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
    /// `sh:node` — the referenced shape is loaded inline at parse time (named or
    /// blank), so inline `sh:node [ … ]` bodies are enforced. `Shape::iri` keeps
    /// the reference for messages.
    Node(Box<Shape>),
    /// `sh:property` nested inside a *property* shape (SHACL §2.1.3 — a property
    /// shape's value nodes are themselves validated against the nested property
    /// shape). Top-level `sh:property` on node shapes stays in
    /// `Shape::property_shapes`; this variant only carries the nested case.
    Property(Box<PropertyShape>),
    QualifiedValueShape {
        // The value shape is stored inline (loaded at parse time) so that the standard
        // SHACL idiom `sh:qualifiedValueShape [ … ]` — an inline blank node that is not a
        // top-level shape — is enforced, mirroring how sh:not/and/or carry inline shapes.
        shape: Box<Shape>,
        min_count: Option<usize>,
        max_count: Option<usize>,
        /// `sh:qualifiedValueShapesDisjoint true`: value nodes that conform to a
        /// sibling property shape's qualified value shape are excluded from the count.
        disjoint: bool,
        /// Qualified value shapes of sibling property shapes (only populated when
        /// `disjoint` is true; wired after all siblings of the parent are loaded).
        sibling_shapes: Vec<Shape>,
    },

    // Other constraints
    Closed {
        ignored_properties: Vec<String>,
        /// Predicate-path IRIs of the shape's own property shapes — these are
        /// the properties a closed shape allows (SHACL §4.8.1).
        allowed_properties: Vec<String>,
    },
    HasValue(Term),
    In(Vec<Term>),

    // SHACL-AF: SPARQL-based constraint. `severity` is the optional sh:severity declared
    // on the sh:SPARQLConstraint node itself (e.g. sh:Warning), overriding the shape's.
    SparqlConstraint {
        select: String,
        message: Option<String>,
        severity: Option<String>,
    },

    // SHACL-AF: sh:expression (node expression) — path + comparison subset. The values
    // reached along `path` from the focus node must satisfy every constraint in `checks`
    // (e.g. sh:minExclusive); a single violation is reported with `message`.
    Expression {
        path: PropertyPath,
        checks: Vec<Constraint>,
        message: Option<String>,
    },
}

/// sh:nodeKind values.
#[derive(Debug, Clone, PartialEq, Eq)]
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
