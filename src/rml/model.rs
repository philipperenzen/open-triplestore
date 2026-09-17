//! RML data model types.
//!
//! Represents parsed RML mapping documents as Rust structs.
//! See: <https://rml.io/specs/rml/> and R2RML <https://www.w3.org/TR/r2rml/>.

use std::collections::BTreeMap;

/// A complete RML mapping document containing one or more TriplesMap entries.
#[derive(Debug, Clone)]
pub struct RmlMapping {
    pub triples_maps: Vec<TriplesMap>,
}

impl RmlMapping {
    pub fn find(&self, iri: &str) -> Option<&TriplesMap> {
        self.triples_maps.iter().find(|tm| tm.iri == iri)
    }

    /// Every datasource IRI (`urn:source:<id>`) the mapping reads from.
    pub fn datasources(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .triples_maps
            .iter()
            .filter_map(|tm| match &tm.logical_source.source {
                SourceRef::Datasource(iri) => Some(iri.clone()),
                _ => None,
            })
            .collect();
        out.sort();
        out.dedup();
        out
    }

    pub fn has_sql_source(&self) -> bool {
        self.triples_maps
            .iter()
            .any(|tm| matches!(tm.logical_source.source, SourceRef::Datasource(_)))
    }
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

/// rml:LogicalSource — describes the source data (file, inline, database).
#[derive(Debug, Clone)]
pub struct LogicalSource {
    pub source: SourceRef,
    pub reference_formulation: ReferenceFormulation,
    pub iterator: Option<String>,
    /// `rml:query` / `rr:sqlQuery` — the statement that selects the rows.
    pub query: Option<String>,
    /// `rr:tableName` — a whole table or view as the logical source.
    pub table_name: Option<String>,
}

impl LogicalSource {
    /// The SQL this logical source selects, with the table name quoted by the
    /// dialect's own rules. `None` when the source is not relational.
    pub fn sql(&self, quote: &dyn Fn(&str) -> String) -> Option<String> {
        if let Some(q) = &self.query {
            return Some(q.clone());
        }
        self.table_name
            .as_ref()
            .map(|t| format!("SELECT * FROM {}", quote(t)))
    }
}

/// The actual data source reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceRef {
    /// File path or URL string
    File(String),
    /// A registered datasource, by its `urn:source:<id>` IRI.
    Datasource(String),
}

/// rml:referenceFormulation — how to interpret references in the source.
#[derive(Debug, Clone, PartialEq)]
pub enum ReferenceFormulation {
    Csv,
    JsonPath,
    XPath,
    /// A relational source: references are column names.
    Sql,
    /// Unknown / custom formulation IRI
    Other(String),
}

impl ReferenceFormulation {
    pub fn from_iri(iri: &str) -> Self {
        match iri {
            "http://semweb.mmlab.be/ns/ql#CSV" => Self::Csv,
            "http://semweb.mmlab.be/ns/ql#JSONPath" => Self::JsonPath,
            "http://semweb.mmlab.be/ns/ql#XPath" => Self::XPath,
            // R2RML has no reference formulation; RML implementations spell a
            // relational source `ql:SQL2008`, and the modern RML-IO vocabulary
            // `rml:SQL2008`. Accept either, plus a bare local name.
            other
                if other.ends_with("#SQL2008")
                    || other.ends_with("/SQL2008")
                    || other.ends_with("#SQL")
                    || other == "SQL2008" =>
            {
                Self::Sql
            }
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
    pub object: ObjectMap,
    pub graph_map: Option<TermMap>,
}

/// How a predicate-object map produces its object.
#[derive(Debug, Clone)]
pub enum ObjectMap {
    /// A term built from this row alone.
    Term(TermMap),
    /// `rr:parentTriplesMap` with join conditions: the object is the subject
    /// another triples map generates for the joined row.
    Ref(RefObjectMap),
    /// `fnml:functionValue`: the object is computed by a declared function.
    Function(FunctionMap),
}

/// A referencing object map.
#[derive(Debug, Clone)]
pub struct RefObjectMap {
    /// IRI of the parent `rr:TriplesMap`.
    pub parent_triples_map: String,
    /// `rr:joinCondition` pairs. An empty list is a cross join, which R2RML
    /// permits only when both logical sources are identical.
    pub joins: Vec<JoinCondition>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinCondition {
    /// Column in the child (this triples map's) row.
    pub child: String,
    /// Column in the parent triples map's row.
    pub parent: String,
}

/// An FNML function call producing a term.
#[derive(Debug, Clone)]
pub struct FunctionMap {
    /// `fno:executes` — the function IRI.
    pub function: String,
    /// Parameters, keyed by their predicate IRI. A parameter is either a
    /// constant or a reference to a column in the current row.
    pub params: BTreeMap<String, Vec<FunctionArg>>,
    /// `rr:datatype` declared on the object map, applied when the function
    /// falls back to emitting a literal.
    pub datatype: Option<String>,
}

impl FunctionMap {
    pub fn first(&self, predicate: &str) -> Option<&FunctionArg> {
        self.params.get(predicate).and_then(|v| v.first())
    }
    pub fn all(&self, predicate: &str) -> &[FunctionArg] {
        self.params.get(predicate).map(Vec::as_slice).unwrap_or(&[])
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FunctionArg {
    Constant(String),
    Reference(String),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_reference_formulations_are_recognised_in_every_spelling() {
        for iri in [
            "http://semweb.mmlab.be/ns/ql#SQL2008",
            "http://w3id.org/rml/SQL2008",
            "http://semweb.mmlab.be/ns/ql#SQL",
        ] {
            assert_eq!(
                ReferenceFormulation::from_iri(iri),
                ReferenceFormulation::Sql,
                "{iri}"
            );
        }
        assert_eq!(
            ReferenceFormulation::from_iri("http://semweb.mmlab.be/ns/ql#CSV"),
            ReferenceFormulation::Csv
        );
        assert!(matches!(
            ReferenceFormulation::from_iri("http://example.org/Custom"),
            ReferenceFormulation::Other(_)
        ));
    }

    #[test]
    fn a_logical_source_renders_table_or_query_sql() {
        let quote = |t: &str| format!("\"{t}\"");
        let table = LogicalSource {
            source: SourceRef::Datasource("urn:source:s".into()),
            reference_formulation: ReferenceFormulation::Sql,
            iterator: None,
            query: None,
            table_name: Some("products".into()),
        };
        assert_eq!(table.sql(&quote).unwrap(), "SELECT * FROM \"products\"");
        let query = LogicalSource {
            query: Some("SELECT 1".into()),
            table_name: Some("ignored".into()),
            ..table.clone()
        };
        assert_eq!(
            query.sql(&quote).unwrap(),
            "SELECT 1",
            "an explicit query wins"
        );
        let file = LogicalSource {
            source: SourceRef::File("x.csv".into()),
            reference_formulation: ReferenceFormulation::Csv,
            iterator: None,
            query: None,
            table_name: None,
        };
        assert_eq!(file.sql(&quote), None);
    }
}
