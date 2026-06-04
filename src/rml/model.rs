//! RML data model types.
//!
//! Represents parsed RML mapping documents as Rust structs.
//! See: <https://rml.io/specs/rml/>

/// A complete RML mapping document containing one or more TriplesMap entries.
#[derive(Debug, Clone)]
pub struct RmlMapping {
    pub triples_maps: Vec<TriplesMap>,
}

/// An rml:TriplesMap — the unit of mapping from a logical source to RDF triples.
#[derive(Debug, Clone)]
pub struct TriplesMap {
    pub iri: String,
    pub logical_source: LogicalSource,
    pub subject_map: SubjectMap,
    pub predicate_object_maps: Vec<PredicateObjectMap>,
    pub graph_map: Option<TermMap>,
}

/// rml:LogicalSource — describes the source data (file, inline, etc.).
#[derive(Debug, Clone)]
pub struct LogicalSource {
    pub source: SourceRef,
    pub reference_formulation: ReferenceFormulation,
    pub iterator: Option<String>,
}

/// The actual data source reference.
#[derive(Debug, Clone)]
pub enum SourceRef {
    /// File path or URL string
    File(String),
    /// Inline data
    Inline(String),
}

/// rml:referenceFormulation — how to interpret references in the source.
#[derive(Debug, Clone, PartialEq)]
pub enum ReferenceFormulation {
    Csv,
    JsonPath,
    XPath,
    /// Unknown / custom formulation IRI
    Other(String),
}

impl ReferenceFormulation {
    pub fn from_iri(iri: &str) -> Self {
        match iri {
            "http://semweb.mmlab.be/ns/ql#CSV" => Self::Csv,
            "http://semweb.mmlab.be/ns/ql#JSONPath" => Self::JsonPath,
            "http://semweb.mmlab.be/ns/ql#XPath" => Self::XPath,
            other => Self::Other(other.to_string()),
        }
    }
}

/// rr:SubjectMap — maps source rows to RDF subjects.
#[derive(Debug, Clone)]
pub struct SubjectMap {
    pub term_map: TermMap,
    /// rr:class — rdf:type assertions added to every generated subject
    pub classes: Vec<String>,
}

/// rr:PredicateObjectMap — maps source rows to predicate-object pairs.
#[derive(Debug, Clone)]
pub struct PredicateObjectMap {
    pub predicate_map: TermMap,
    pub object_map: TermMap,
    pub graph_map: Option<TermMap>,
}

/// A term map: constant, template, or column/reference.
#[derive(Debug, Clone)]
pub struct TermMap {
    pub kind: TermMapKind,
    pub term_type: TermType,
    /// Optional datatype IRI for literals
    pub datatype: Option<String>,
    /// Optional language tag for literals
    pub language: Option<String>,
}

/// How the term value is produced.
#[derive(Debug, Clone)]
pub enum TermMapKind {
    /// `rr:constant` — a fixed IRI or literal
    Constant(String),
    /// `rr:template` — e.g. `"http://example.org/{column}"`
    Template(String),
    /// `rml:reference` or `rr:column` — a direct column/JSONPath/XPath reference
    Reference(String),
}

/// `rr:termType` — the RDF term type to produce.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::upper_case_acronyms)]
pub enum TermType {
    IRI,
    BlankNode,
    Literal,
}
