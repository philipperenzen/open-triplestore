use oxigraph::model::Term;

/// A SHACL shape (either node shape or property shape).
///
/// Internal model only — focus/value nodes and constraint constants are typed
/// [`Term`]s end-to-end; conversion to display strings happens at
/// report-building time (see `constraints::display_term`).
#[derive(Debug, Clone)]
pub struct Shape {
    pub iri: String,
    /// `sh:name` — informational for validation, but the only faithful source
    /// of a specification's name when a shape graph is exported back to an
    /// exchange format.
    pub name: Option<String>,
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
    /// SHACL-AF §3.2: a target whose type is a `sh:SPARQLTargetType` — the
    /// type's `SELECT ?this`, with the target's parameter values bound as
    /// terms (every parameter variable is projected so it can be bound).
    SparqlTargetType {
        query: Box<opengraph::spargebra::Query>,
        bindings: Vec<(oxigraph::sparql::Variable, Term)>,
    },
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
    /// `sh:deactivated true`: every term conforms (SHACL §2.1.6), so the
    /// shape's constraints are not evaluated.
    pub deactivated: bool,
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

const SH_NS: &str = "http://www.w3.org/ns/shacl#";

/// Parse a SHACL property path (SHACL §2.3) starting at `node` into a [`PropertyPath`],
/// over any graph: `objects(subject, predicate)` returns the objects of a node in
/// lexical form (an IRI, `_:label` for a blank node, or a literal's value).
///
/// Handles a predicate IRI; an RDF-list **sequence** path `( p1 p2 … )`; and the blank-node
/// path operators `sh:inversePath`, `sh:alternativePath` (an RDF list), `sh:zeroOrMorePath`,
/// `sh:oneOrMorePath`, `sh:zeroOrOnePath`. Returns `None` for an empty or malformed path so
/// the caller can skip the property shape rather than mis-bind it.
///
/// A node carrying BOTH list cells (`rdf:first`/`rdf:rest`) and a path operator is
/// interpreted as the sequence path — matching the W3C suite's `path-strange-*`
/// expectations, which treat the list reading as authoritative.
pub(crate) fn parse_property_path_with(
    node: &str,
    objects: &dyn Fn(&str, &str) -> Vec<String>,
) -> Option<PropertyPath> {
    // A predicate path is a plain IRI.
    if !node.starts_with("_:") {
        return Some(PropertyPath::Predicate(node.to_string()));
    }
    // Blank node: an RDF-list sequence path takes precedence over operators.
    let seq: Vec<PropertyPath> = rdf_list_elements(node, objects)
        .iter()
        .filter_map(|e| parse_property_path_with(e, objects))
        .collect();
    if !seq.is_empty() {
        return Some(PropertyPath::Sequence(seq));
    }
    let op =
        |p: &str| -> Option<String> { objects(node, &format!("{SH_NS}{p}")).into_iter().next() };
    if let Some(inner) = op("inversePath") {
        return parse_property_path_with(&inner, objects)
            .map(|p| PropertyPath::Inverse(Box::new(p)));
    }
    if let Some(head) = op("alternativePath") {
        let parts: Vec<PropertyPath> = rdf_list_elements(&head, objects)
            .iter()
            .filter_map(|e| parse_property_path_with(e, objects))
            .collect();
        return (!parts.is_empty()).then_some(PropertyPath::Alternative(parts));
    }
    if let Some(inner) = op("zeroOrMorePath") {
        return parse_property_path_with(&inner, objects)
            .map(|p| PropertyPath::ZeroOrMore(Box::new(p)));
    }
    if let Some(inner) = op("oneOrMorePath") {
        return parse_property_path_with(&inner, objects)
            .map(|p| PropertyPath::OneOrMore(Box::new(p)));
    }
    if let Some(inner) = op("zeroOrOnePath") {
        return parse_property_path_with(&inner, objects)
            .map(|p| PropertyPath::ZeroOrOne(Box::new(p)));
    }
    None
}

/// Walk the RDF list whose head is `head`, returning each member's lexical node form
/// (IRI, `_:label`, or literal value). Empty if `head` is not a list.
fn rdf_list_elements(head: &str, objects: &dyn Fn(&str, &str) -> Vec<String>) -> Vec<String> {
    const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
    const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
    const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
    let mut out = Vec::new();
    let mut current = head.to_string();
    for _ in 0..10_000 {
        if current == RDF_NIL {
            break;
        }
        match objects(&current, RDF_FIRST).into_iter().next() {
            Some(first) => out.push(first),
            None => break,
        }
        match objects(&current, RDF_REST).into_iter().next() {
            Some(rest) => current = rest,
            None => break,
        }
    }
    out
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
        /// The `sh:sparql` node (an IRI, or `_:label`): `sh:sourceConstraint`.
        node: String,
        select: String,
        message: Option<String>,
        severity: Option<String>,
        /// The `sh:resultAnnotation`s of the `sh:sparql` node (SHACL-AF §4).
        annotations: Vec<ResultAnnotation>,
    },

    // SHACL-AF §6: an instance of a constraint component declared in the shapes
    // graph (`sh:ConstraintComponent` + `sh:parameter` + a validator), created
    // for a shape that carries the component's parameter predicates.
    Custom(Box<CustomConstraint>),

    // SHACL-AF §7: sh:expression — a node expression that must produce exactly
    // `{ true }` with each value node as its focus node. `message` is the
    // expression node's sh:message; `node` the expression node itself, every
    // result's `sh:sourceConstraint`.
    Expression {
        node: Term,
        expr: super::node_expr::NodeExpr,
        message: Option<String>,
    },
}

/// A result annotation (SHACL-AF §4), declared with `sh:resultAnnotation` on
/// the node that carries a SPARQL constraint's or validator's `sh:select` /
/// `sh:ask`: each result that query produces gets `property` set to the
/// solution's binding of `var_name`, or to `defaults` when it has none.
#[derive(Debug, Clone)]
pub struct ResultAnnotation {
    /// `sh:annotationProperty`.
    pub property: oxigraph::model::NamedNode,
    /// `sh:annotationVarName`, else the local name of `property`.
    pub var_name: String,
    /// `sh:annotationValue`.
    pub defaults: Vec<Term>,
}

/// A validator of a SHACL-AF constraint component: an ASK evaluated once per
/// value node (`false` = violation) or a SELECT whose rows are the violations.
#[derive(Debug, Clone)]
pub enum CustomValidator {
    Ask(String),
    Select(String),
}

/// An instantiated constraint component (SHACL-AF §6): the component's IRI,
/// the parameter values the shape supplies (by the parameter's local name,
/// the SPARQL variable the validator sees), the validator picked for the
/// shape's kind and the validator's `sh:message`.
#[derive(Debug, Clone)]
pub struct CustomConstraint {
    pub component: String,
    pub params: Vec<(String, Term)>,
    pub validator: CustomValidator,
    pub message: Option<String>,
    /// The validator's `sh:resultAnnotation`s (SHACL-AF §4).
    pub annotations: Vec<ResultAnnotation>,
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
