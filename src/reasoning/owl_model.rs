//! The OWL 2 structural model (OWL 2 Structural Specification §5–§10), as far
//! as an external DL reasoner needs it.
//!
//! [`super::owl_mapping`] builds it from RDF triples (the reverse of the OWL 2
//! RDF mapping), [`super::owl_fs`] writes it as functional-style syntax and
//! [`super::owl_profile`] checks the OWL 2 DL global restrictions on it.

/// An individual: an IRI, or an anonymous individual (an RDF blank node).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Individual {
    Named(String),
    Anonymous(String),
}

/// A literal: lexical form plus datatype IRI or language tag.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Literal {
    pub lexical: String,
    /// Datatype IRI; `rdf:langString` (or `rdf:PlainLiteral`) when `lang` is set.
    pub datatype: String,
    pub lang: Option<String>,
}

/// An annotation value (§10.1).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AnnotationValue {
    Iri(String),
    Anonymous(String),
    Literal(Literal),
}

/// An annotation with its own (nested) annotations.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Annotation {
    pub property: String,
    pub value: AnnotationValue,
    pub annotations: Vec<Annotation>,
}

/// An annotation subject (§10.2.1).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AnnotationSubject {
    Iri(String),
    Anonymous(String),
}

/// An object property expression (§6.1).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ObjectProp {
    Named(String),
    Inverse(String),
}

impl ObjectProp {
    /// The property IRI, whichever direction.
    pub fn name(&self) -> &str {
        match self {
            ObjectProp::Named(p) | ObjectProp::Inverse(p) => p,
        }
    }

    pub fn inverse(&self) -> ObjectProp {
        match self {
            ObjectProp::Named(p) => ObjectProp::Inverse(p.clone()),
            ObjectProp::Inverse(p) => ObjectProp::Named(p.clone()),
        }
    }
}

/// A data range (§7).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DataRange {
    Datatype(String),
    IntersectionOf(Vec<DataRange>),
    UnionOf(Vec<DataRange>),
    ComplementOf(Box<DataRange>),
    OneOf(Vec<Literal>),
    /// `DatatypeRestriction(DT facet value …)`.
    Restriction(String, Vec<(String, Literal)>),
}

/// A class expression (§8).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ClassExpr {
    Class(String),
    IntersectionOf(Vec<ClassExpr>),
    UnionOf(Vec<ClassExpr>),
    ComplementOf(Box<ClassExpr>),
    OneOf(Vec<Individual>),
    SomeValuesFrom(ObjectProp, Box<ClassExpr>),
    AllValuesFrom(ObjectProp, Box<ClassExpr>),
    HasValue(ObjectProp, Individual),
    HasSelf(ObjectProp),
    MinCardinality(u32, ObjectProp, Option<Box<ClassExpr>>),
    MaxCardinality(u32, ObjectProp, Option<Box<ClassExpr>>),
    ExactCardinality(u32, ObjectProp, Option<Box<ClassExpr>>),
    /// `DataSomeValuesFrom(DPE+ DR)`.
    DataSomeValuesFrom(Vec<String>, DataRange),
    DataAllValuesFrom(Vec<String>, DataRange),
    DataHasValue(String, Literal),
    DataMinCardinality(u32, String, Option<DataRange>),
    DataMaxCardinality(u32, String, Option<DataRange>),
    DataExactCardinality(u32, String, Option<DataRange>),
}

/// The kind of an entity (§5.8 declarations).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EntityKind {
    Class,
    Datatype,
    ObjectProperty,
    DataProperty,
    AnnotationProperty,
    NamedIndividual,
}

impl EntityKind {
    pub fn fs_name(self) -> &'static str {
        match self {
            EntityKind::Class => "Class",
            EntityKind::Datatype => "Datatype",
            EntityKind::ObjectProperty => "ObjectProperty",
            EntityKind::DataProperty => "DataProperty",
            EntityKind::AnnotationProperty => "AnnotationProperty",
            EntityKind::NamedIndividual => "NamedIndividual",
        }
    }
}

/// An axiom (§9 and §10.2) without its annotations.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AxiomKind {
    Declaration(EntityKind, String),
    // Class expression axioms (§9.1)
    SubClassOf(ClassExpr, ClassExpr),
    EquivalentClasses(Vec<ClassExpr>),
    DisjointClasses(Vec<ClassExpr>),
    DisjointUnion(String, Vec<ClassExpr>),
    // Object property axioms (§9.2)
    SubObjectPropertyOf(ObjectProp, ObjectProp),
    /// `SubObjectPropertyOf(ObjectPropertyChain(…) super)`.
    SubObjectPropertyChain(Vec<ObjectProp>, ObjectProp),
    EquivalentObjectProperties(Vec<ObjectProp>),
    DisjointObjectProperties(Vec<ObjectProp>),
    InverseObjectProperties(ObjectProp, ObjectProp),
    ObjectPropertyDomain(ObjectProp, ClassExpr),
    ObjectPropertyRange(ObjectProp, ClassExpr),
    FunctionalObjectProperty(ObjectProp),
    InverseFunctionalObjectProperty(ObjectProp),
    ReflexiveObjectProperty(ObjectProp),
    IrreflexiveObjectProperty(ObjectProp),
    SymmetricObjectProperty(ObjectProp),
    AsymmetricObjectProperty(ObjectProp),
    TransitiveObjectProperty(ObjectProp),
    // Data property axioms (§9.3)
    SubDataPropertyOf(String, String),
    EquivalentDataProperties(Vec<String>),
    DisjointDataProperties(Vec<String>),
    DataPropertyDomain(String, ClassExpr),
    DataPropertyRange(String, DataRange),
    FunctionalDataProperty(String),
    // Datatype definitions (§9.4) and keys (§9.5)
    DatatypeDefinition(String, DataRange),
    HasKey(ClassExpr, Vec<ObjectProp>, Vec<String>),
    // Assertions (§9.6)
    SameIndividual(Vec<Individual>),
    DifferentIndividuals(Vec<Individual>),
    ClassAssertion(ClassExpr, Individual),
    ObjectPropertyAssertion(ObjectProp, Individual, Individual),
    NegativeObjectPropertyAssertion(ObjectProp, Individual, Individual),
    DataPropertyAssertion(String, Individual, Literal),
    NegativeDataPropertyAssertion(String, Individual, Literal),
    // Annotation axioms (§10.2)
    AnnotationAssertion(String, AnnotationSubject, AnnotationValue),
    SubAnnotationPropertyOf(String, String),
    AnnotationPropertyDomain(String, String),
    AnnotationPropertyRange(String, String),
}

/// An axiom with its axiom annotations.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Axiom {
    pub kind: AxiomKind,
    pub annotations: Vec<Annotation>,
}

impl Axiom {
    pub fn new(kind: AxiomKind) -> Self {
        Axiom {
            kind,
            annotations: Vec::new(),
        }
    }
}

/// An ontology (§3).
#[derive(Debug, Clone, Default)]
pub struct Ontology {
    pub iri: Option<String>,
    pub version_iri: Option<String>,
    /// `owl:imports` targets. They are recorded, never fetched: a run reasons
    /// over the graphs it was given.
    pub imports: Vec<String>,
    pub annotations: Vec<Annotation>,
    pub axioms: Vec<Axiom>,
}

impl Ontology {
    /// Every declared entity of `kind`.
    pub fn declared(&self, kind: EntityKind) -> impl Iterator<Item = &str> {
        self.axioms.iter().filter_map(move |a| match &a.kind {
            AxiomKind::Declaration(k, iri) if *k == kind => Some(iri.as_str()),
            _ => None,
        })
    }
}

// ─── Well-known IRIs ─────────────────────────────────────────────────────────

pub const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
pub const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";
pub const OWL: &str = "http://www.w3.org/2002/07/owl#";
pub const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

pub const OWL_THING: &str = "http://www.w3.org/2002/07/owl#Thing";
pub const OWL_NOTHING: &str = "http://www.w3.org/2002/07/owl#Nothing";
pub const OWL_TOP_OBJECT_PROPERTY: &str = "http://www.w3.org/2002/07/owl#topObjectProperty";
pub const OWL_BOTTOM_OBJECT_PROPERTY: &str = "http://www.w3.org/2002/07/owl#bottomObjectProperty";
pub const OWL_TOP_DATA_PROPERTY: &str = "http://www.w3.org/2002/07/owl#topDataProperty";
pub const OWL_BOTTOM_DATA_PROPERTY: &str = "http://www.w3.org/2002/07/owl#bottomDataProperty";
pub const RDFS_LITERAL: &str = "http://www.w3.org/2000/01/rdf-schema#Literal";

/// The built-in annotation properties of OWL 2 (§5.5).
pub const BUILTIN_ANNOTATION_PROPERTIES: &[&str] = &[
    "http://www.w3.org/2000/01/rdf-schema#label",
    "http://www.w3.org/2000/01/rdf-schema#comment",
    "http://www.w3.org/2000/01/rdf-schema#seeAlso",
    "http://www.w3.org/2000/01/rdf-schema#isDefinedBy",
    "http://www.w3.org/2002/07/owl#deprecated",
    "http://www.w3.org/2002/07/owl#versionInfo",
    "http://www.w3.org/2002/07/owl#priorVersion",
    "http://www.w3.org/2002/07/owl#backwardCompatibleWith",
    "http://www.w3.org/2002/07/owl#incompatibleWith",
];

/// The datatypes of the OWL 2 datatype map (§4), plus `rdfs:Literal`.
pub const DATATYPE_MAP: &[&str] = &[
    "http://www.w3.org/2000/01/rdf-schema#Literal",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#PlainLiteral",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#XMLLiteral",
    "http://www.w3.org/2002/07/owl#real",
    "http://www.w3.org/2002/07/owl#rational",
    "http://www.w3.org/2001/XMLSchema#decimal",
    "http://www.w3.org/2001/XMLSchema#integer",
    "http://www.w3.org/2001/XMLSchema#nonNegativeInteger",
    "http://www.w3.org/2001/XMLSchema#nonPositiveInteger",
    "http://www.w3.org/2001/XMLSchema#positiveInteger",
    "http://www.w3.org/2001/XMLSchema#negativeInteger",
    "http://www.w3.org/2001/XMLSchema#long",
    "http://www.w3.org/2001/XMLSchema#int",
    "http://www.w3.org/2001/XMLSchema#short",
    "http://www.w3.org/2001/XMLSchema#byte",
    "http://www.w3.org/2001/XMLSchema#unsignedLong",
    "http://www.w3.org/2001/XMLSchema#unsignedInt",
    "http://www.w3.org/2001/XMLSchema#unsignedShort",
    "http://www.w3.org/2001/XMLSchema#unsignedByte",
    "http://www.w3.org/2001/XMLSchema#double",
    "http://www.w3.org/2001/XMLSchema#float",
    "http://www.w3.org/2001/XMLSchema#string",
    "http://www.w3.org/2001/XMLSchema#normalizedString",
    "http://www.w3.org/2001/XMLSchema#token",
    "http://www.w3.org/2001/XMLSchema#language",
    "http://www.w3.org/2001/XMLSchema#Name",
    "http://www.w3.org/2001/XMLSchema#NCName",
    "http://www.w3.org/2001/XMLSchema#NMTOKEN",
    "http://www.w3.org/2001/XMLSchema#boolean",
    "http://www.w3.org/2001/XMLSchema#hexBinary",
    "http://www.w3.org/2001/XMLSchema#base64Binary",
    "http://www.w3.org/2001/XMLSchema#anyURI",
    "http://www.w3.org/2001/XMLSchema#dateTime",
    "http://www.w3.org/2001/XMLSchema#dateTimeStamp",
];

/// The constraining facets OWL 2 allows in a datatype restriction (§4).
pub const FACETS: &[&str] = &[
    "http://www.w3.org/2001/XMLSchema#minInclusive",
    "http://www.w3.org/2001/XMLSchema#maxInclusive",
    "http://www.w3.org/2001/XMLSchema#minExclusive",
    "http://www.w3.org/2001/XMLSchema#maxExclusive",
    "http://www.w3.org/2001/XMLSchema#length",
    "http://www.w3.org/2001/XMLSchema#minLength",
    "http://www.w3.org/2001/XMLSchema#maxLength",
    "http://www.w3.org/2001/XMLSchema#pattern",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#langRange",
];

/// Whether `iri` lies in a namespace OWL 2 reserves (§2.4).
pub fn is_reserved(iri: &str) -> bool {
    iri.starts_with(RDF) || iri.starts_with(RDFS) || iri.starts_with(OWL) || iri.starts_with(XSD)
}
