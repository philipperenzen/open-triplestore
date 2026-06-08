//! AST types for ShEx schemas.
//!
//! Models the core constructs of the Shape Expressions Language:
//! shape expressions, triple expressions, node constraints, and cardinalities.
#![allow(dead_code)]

use std::collections::HashMap;

/// A complete ShEx schema containing prefixes and shape declarations.
#[derive(Debug, Clone)]
pub struct ShExSchema {
    /// Prefix declarations (label → IRI).
    pub prefixes: HashMap<String, String>,
    /// Base IRI for relative IRI resolution.
    pub base: Option<String>,
    /// Shape declarations keyed by shape IRI.
    pub shapes: Vec<ShapeDecl>,
    /// Start shape expression (entry point for validation).
    pub start: Option<Box<ShapeExpr>>,
}

impl ShExSchema {
    /// Look up a shape declaration by IRI.
    pub fn find_shape(&self, iri: &str) -> Option<&ShapeDecl> {
        self.shapes.iter().find(|s| s.id == iri)
    }

    /// Expand a prefixed name using the schema's prefix map.
    pub fn expand_prefixed(&self, prefixed: &str) -> Option<String> {
        let (prefix, local) = prefixed.split_once(':')?;
        let ns = self.prefixes.get(prefix)?;
        Some(format!("{}{}", ns, local))
    }
}

/// A named shape declaration.
#[derive(Debug, Clone)]
pub struct ShapeDecl {
    /// The IRI identifying this shape.
    pub id: String,
    /// The shape expression body.
    pub shape_expr: ShapeExpr,
}

/// A shape expression — the core validation construct.
#[derive(Debug, Clone)]
pub enum ShapeExpr {
    /// Node constraint (datatype, node kind, string/numeric facets).
    NodeConstraint(NodeConstraint),
    /// Shape with triple expression and optional CLOSED / EXTRA.
    Shape {
        expression: Option<TripleExpr>,
        closed: bool,
        extra: Vec<String>,
    },
    /// Conjunction: all sub-expressions must match.
    ShapeAnd(Vec<ShapeExpr>),
    /// Disjunction: at least one sub-expression must match.
    ShapeOr(Vec<ShapeExpr>),
    /// Negation: the sub-expression must NOT match.
    ShapeNot(Box<ShapeExpr>),
    /// Reference to another shape by IRI.
    ShapeRef(String),
    /// Matches any node (used as wildcard).
    NodeConstraintAny,
}

/// Constraints on individual RDF nodes.
#[derive(Debug, Clone, Default)]
pub struct NodeConstraint {
    /// Required node kind (IRI, BNode, Literal, NonLiteral).
    pub node_kind: Option<NodeKind>,
    /// Required datatype IRI.
    pub datatype: Option<String>,
    /// String facets (pattern, minlength, maxlength, length).
    pub string_facets: Vec<StringFacet>,
    /// Numeric facets (mininclusive, maxinclusive, etc.).
    pub numeric_facets: Vec<NumericFacet>,
    /// Enumerated values.
    pub values: Vec<String>,
}

/// RDF node kinds.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::upper_case_acronyms)] // IRI mirrors the SHACL/ShEx node-kind keyword
pub enum NodeKind {
    IRI,
    BNode,
    Literal,
    NonLiteral,
}

/// String-based facet constraints.
#[derive(Debug, Clone)]
pub enum StringFacet {
    Pattern(String, Option<String>), // (regex, flags)
    MinLength(usize),
    MaxLength(usize),
    Length(usize),
}

/// Numeric facet constraints.
#[derive(Debug, Clone)]
pub enum NumericFacet {
    MinInclusive(f64),
    MaxInclusive(f64),
    MinExclusive(f64),
    MaxExclusive(f64),
    TotalDigits(usize),
    FractionDigits(usize),
}

/// A triple expression — describes expected triples for a focus node.
#[derive(Debug, Clone)]
pub enum TripleExpr {
    /// A single triple constraint: predicate + optional value shape + cardinality.
    TripleConstraint {
        predicate: String,
        /// If `true`, this is an inverse constraint (object → subject).
        inverse: bool,
        value_expr: Option<Box<ShapeExpr>>,
        min: usize,
        max: Cardinality,
        /// Semantic actions (ignored for now, stored for round-tripping).
        annotations: Vec<Annotation>,
    },
    /// Ordered conjunction: all sub-expressions must match (ordered grouping).
    EachOf(Vec<TripleExpr>),
    /// Unordered disjunction: exactly one sub-expression must match.
    OneOf(Vec<TripleExpr>),
}

/// Cardinality upper bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cardinality {
    Exact(usize),
    Unbounded,
}

impl Cardinality {
    pub fn allows(&self, count: usize) -> bool {
        match self {
            Cardinality::Exact(n) => count <= *n,
            Cardinality::Unbounded => true,
        }
    }
}

/// An annotation on a triple expression.
#[derive(Debug, Clone)]
pub struct Annotation {
    pub predicate: String,
    pub object: String,
}
