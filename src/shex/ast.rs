//! The ShEx 2.1 abstract syntax, shaped after ShExJ
//! (<http://shex.io/shex-semantics/#shexj>).
//!
//! Every IRI here is absolute: the ShExC, ShExJ and ShExR readers resolve
//! relative IRIs and prefixed names against the document's base and prefixes
//! before building these values, so two representations of one schema build
//! equal trees (the `schemas` representation tests rely on that).

use std::fmt;

use super::xsd::Numeric;

pub const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
pub const RDF_LANG_STRING: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString";
pub const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";

/// A shape expression or triple expression label: an IRI or a blank node.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Label {
    Iri(String),
    BNode(String),
}

impl Label {
    /// `_:x` for a blank node, the bare IRI otherwise (the form reports use).
    pub fn as_display(&self) -> String {
        match self {
            Label::Iri(i) => i.clone(),
            Label::BNode(b) => format!("_:{b}"),
        }
    }

    /// A label from its report/ShExJ form: `_:x` is a blank node, anything
    /// else an IRI.
    pub fn from_display(s: &str) -> Label {
        match s.strip_prefix("_:") {
            Some(b) => Label::BNode(b.to_string()),
            None => Label::Iri(s.to_string()),
        }
    }
}

impl fmt::Display for Label {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Label::Iri(i) => write!(f, "<{i}>"),
            Label::BNode(b) => write!(f, "_:{b}"),
        }
    }
}

/// A complete schema: `Schema { startActs start imports shapes }`.
#[derive(Debug, Clone, Default)]
pub struct Schema {
    pub start_acts: Vec<SemAct>,
    pub start: Option<ShapeExpr>,
    /// Absolute IRIs of imported schemas, in document order.
    pub imports: Vec<String>,
    /// Shape declarations in document order.
    pub shapes: Vec<ShapeDecl>,
    /// Prefixes declared by a ShExC document (prefix → namespace), kept so a
    /// ShapeMap can use the schema's prefixed names. Not part of the
    /// abstract syntax and ignored by equality.
    pub prefixes: Vec<(String, String)>,
    /// The base IRI the document ended with (ShExC `BASE`), likewise kept for
    /// ShapeMap resolution only.
    pub base: Option<String>,
}

/// A labelled shape expression (`ShapeDecl`, or a 2.1 shape expression with
/// an `id`).
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeDecl {
    pub label: Label,
    pub expr: ShapeExpr,
}

/// `shapeExpr = ShapeOr | ShapeAnd | ShapeNot | NodeConstraint | Shape |
/// ShapeExternal | shapeExprRef`.
#[derive(Debug, Clone, PartialEq)]
pub enum ShapeExpr {
    Or(Vec<ShapeExpr>),
    And(Vec<ShapeExpr>),
    Not(Box<ShapeExpr>),
    NodeConstraint(NodeConstraint),
    Shape(Box<Shape>),
    External,
    Ref(Label),
}

impl ShapeExpr {
    /// The empty shape `{}` — what ShExC's `.` stands for.
    pub fn empty_shape() -> ShapeExpr {
        ShapeExpr::Shape(Box::default())
    }

    pub fn is_empty_shape(&self) -> bool {
        matches!(self, ShapeExpr::Shape(s) if **s == Shape::default())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Iri,
    BNode,
    NonLiteral,
    Literal,
}

impl NodeKind {
    pub fn name(self) -> &'static str {
        match self {
            NodeKind::Iri => "iri",
            NodeKind::BNode => "bnode",
            NodeKind::NonLiteral => "nonliteral",
            NodeKind::Literal => "literal",
        }
    }
}

/// `NodeConstraint { nodeKind datatype xsFacet* values }`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NodeConstraint {
    pub node_kind: Option<NodeKind>,
    pub datatype: Option<String>,
    pub string_facets: Vec<StringFacet>,
    pub numeric_facets: Vec<NumericFacet>,
    pub values: Option<Vec<ValueSetValue>>,
    /// Semantic actions and annotations of a declared node constraint
    /// (`<S> [1 2] %<act>{ … %}`), as the test suite's ShExJ carries them.
    pub sem_acts: Vec<SemAct>,
    pub annotations: Vec<Annotation>,
}

impl NodeConstraint {
    pub fn is_empty(&self) -> bool {
        *self == NodeConstraint::default()
    }

    /// Facets in one fixed order (ShExJ's object keys carry none), so the
    /// ShExC and ShExJ forms of a constraint compare equal.
    pub fn normalize(&mut self) {
        self.string_facets.sort_by_key(|f| match f {
            StringFacet::Length(_) => 0,
            StringFacet::MinLength(_) => 1,
            StringFacet::MaxLength(_) => 2,
            StringFacet::Pattern(..) => 3,
        });
        self.numeric_facets.sort_by_key(|f| match f {
            NumericFacet::MinInclusive(_) => 0,
            NumericFacet::MinExclusive(_) => 1,
            NumericFacet::MaxInclusive(_) => 2,
            NumericFacet::MaxExclusive(_) => 3,
            NumericFacet::TotalDigits(_) => 4,
            NumericFacet::FractionDigits(_) => 5,
        });
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum StringFacet {
    Length(u64),
    MinLength(u64),
    MaxLength(u64),
    /// An XPath regular expression (already unescaped per the ShExC REGEXP
    /// rules) and its flags.
    Pattern(String, Option<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum NumericFacet {
    MinInclusive(Numeric),
    MinExclusive(Numeric),
    MaxInclusive(Numeric),
    MaxExclusive(Numeric),
    TotalDigits(u64),
    FractionDigits(u64),
}

/// An RDF literal as a value set member or annotation object.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObjectLiteral {
    pub value: String,
    pub language: Option<String>,
    /// The datatype IRI; `None` is `xsd:string` (or `rdf:langString` with a
    /// language).
    pub datatype: Option<String>,
}

impl ObjectLiteral {
    pub fn datatype_iri(&self) -> &str {
        match (&self.datatype, &self.language) {
            (Some(d), _) => d,
            (None, Some(_)) => RDF_LANG_STRING,
            (None, None) => XSD_STRING,
        }
    }
}

/// `objectValue = IRIREF | ObjectLiteral`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ObjectValue {
    Iri(String),
    Literal(ObjectLiteral),
}

/// A stem or the `.` wildcard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stem {
    Wildcard,
    Value(String),
}

/// An exclusion inside a stem range: one value, or a stem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exclusion {
    Value(String),
    Stem(String),
}

/// `valueSetValue`.
#[derive(Debug, Clone, PartialEq)]
pub enum ValueSetValue {
    Object(ObjectValue),
    IriStem(String),
    IriStemRange(Stem, Vec<Exclusion>),
    LiteralStem(String),
    LiteralStemRange(Stem, Vec<Exclusion>),
    Language(String),
    LanguageStem(String),
    LanguageStemRange(Stem, Vec<Exclusion>),
}

impl ValueSetValue {
    /// Language tags in lower case (they compare case-insensitively), as the
    /// ShExC reader produces them.
    pub fn lowercase_languages(self) -> ValueSetValue {
        let ex = |v: Vec<Exclusion>| {
            v.into_iter()
                .map(|e| match e {
                    Exclusion::Value(s) => Exclusion::Value(s.to_ascii_lowercase()),
                    Exclusion::Stem(s) => Exclusion::Stem(s.to_ascii_lowercase()),
                })
                .collect()
        };
        match self {
            ValueSetValue::Language(t) => ValueSetValue::Language(t.to_ascii_lowercase()),
            ValueSetValue::LanguageStem(t) => ValueSetValue::LanguageStem(t.to_ascii_lowercase()),
            ValueSetValue::LanguageStemRange(stem, e) => ValueSetValue::LanguageStemRange(
                match stem {
                    Stem::Value(s) => Stem::Value(s.to_ascii_lowercase()),
                    Stem::Wildcard => Stem::Wildcard,
                },
                ex(e),
            ),
            other => other,
        }
    }
}

/// `Shape { closed extra expression semActs annotations }`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Shape {
    pub closed: bool,
    pub extra: Vec<String>,
    pub expression: Option<TripleExpr>,
    pub sem_acts: Vec<SemAct>,
    pub annotations: Vec<Annotation>,
}

/// A cardinality's upper bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Max {
    Bounded(u32),
    Unbounded,
}

impl Max {
    pub fn allows(self, n: u32) -> bool {
        match self {
            Max::Bounded(m) => n <= m,
            Max::Unbounded => true,
        }
    }
}

/// `tripleExpr = EachOf | OneOf | TripleConstraint | tripleExprRef`.
#[derive(Debug, Clone, PartialEq)]
pub enum TripleExpr {
    EachOf(Group),
    OneOf(Group),
    TripleConstraint(TripleConstraint),
    /// An inclusion `&label`.
    Ref(Label),
}

impl TripleExpr {
    pub fn id(&self) -> Option<&Label> {
        match self {
            TripleExpr::EachOf(g) | TripleExpr::OneOf(g) => g.id.as_ref(),
            TripleExpr::TripleConstraint(tc) => tc.id.as_ref(),
            TripleExpr::Ref(_) => None,
        }
    }
}

/// The body shared by EachOf and OneOf.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub id: Option<Label>,
    pub expressions: Vec<TripleExpr>,
    pub min: u32,
    pub max: Max,
    pub sem_acts: Vec<SemAct>,
    pub annotations: Vec<Annotation>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TripleConstraint {
    pub id: Option<Label>,
    pub inverse: bool,
    pub predicate: String,
    pub value_expr: Option<Box<ShapeExpr>>,
    pub min: u32,
    pub max: Max,
    pub sem_acts: Vec<SemAct>,
    pub annotations: Vec<Annotation>,
}

/// `SemAct { name code? }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemAct {
    pub name: String,
    pub code: Option<String>,
}

/// `Annotation { predicate object }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Annotation {
    pub predicate: String,
    pub object: ObjectValue,
}

impl PartialEq for Schema {
    fn eq(&self, other: &Self) -> bool {
        self.start_acts == other.start_acts
            && self.start == other.start
            && self.imports == other.imports
            && self.shapes == other.shapes
    }
}
